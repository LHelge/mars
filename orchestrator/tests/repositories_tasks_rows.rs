//! `TaskRepository`'s row helpers — tasks, dependencies, comments, hand-offs
//! and session links — against a real Postgres (`CLAUDE.md`, "Testing
//! expectations").
//!
//! These are the rules `docs/data-model.md` hands to the repository because
//! the schema cannot carry them, so they are asserted here rather than trusted
//! to a constraint:
//!
//! - numbers come from `projects.next_task_number` and are never reused, and a
//!   task with no state given lands in the project's default queue state;
//! - nesting is one level deep, on creation and on re-parenting, including
//!   terminal children (`ARCHITECTURE.md`, "Task tracker", "Parents");
//! - an update whose values already match the stored row changes nothing and
//!   reports it, so its caller writes no event and no session link (ADR 0030);
//! - both ends of a dependency belong to one project, and the edge's identity
//!   includes its kind, so `blocks` and `discovered_from` coexist for a pair;
//! - a comment has exactly one author, or none when it is a system comment;
//! - a hand-off's task, comment and sessions all belong where they claim to;
//! - a session link preserves `first_touched_at` and advances
//!   `last_touched_at` only when a later transaction touches it again.
//!
//! Scope is asserted throughout by giving each helper an id from a second
//! project and requiring the documented refusal, never a leaked row.
//!
//! Needs a container engine; see `tests/common/db.rs`.

mod common;

use std::str::FromStr;
use std::time::Duration;

use axum::http::StatusCode;
use mars_orchestrator::models::{
    Label, NewSession, NewTask, NewTaskComment, NewTaskHandoff, Priority, ProfileKind,
    ReviewStatus, Task, TaskDependencyKind, TaskRef, TaskStateKind, TaskTitle, TaskUpdate,
};
use mars_orchestrator::prelude::*;
use mars_orchestrator::repositories::{SessionRepository, StateFields, TaskFilter, TaskRepository};
use tokio::time::sleep;
use uuid::Uuid;

/// Not a credential: an obviously fake stand-in for the Argon2id PHC string
/// the seeded user would carry (`CLAUDE.md`, rule 3).
const FAKE_PASSWORD_HASH: &str = "$argon2id$fake$hash";

/// Not a real image: the stub the session tests replay a fixture transcript
/// with.
const TEST_IMAGE: &str = "localhost/mars-session-stub:test";

/// An obviously fake but well-formed SHA-1 object id (rule 3).
const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

/// Long enough that `NOW()` — the transaction's start time — differs between
/// two transactions, short enough not to drag the suite out.
const BETWEEN_TOUCHES: Duration = Duration::from_millis(50);

/// One project with its states and a profile, plus the user everything is
/// attributed to.
struct Fixture {
    user_id: Uuid,
    project_id: Uuid,
    profile_id: Uuid,
}

async fn seed(pool: &PgPool) -> Fixture {
    let user_id = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, username, email, password_hash) VALUES ($1, $2, $3, $4)")
        .bind(user_id)
        .bind(format!("user-{}", &user_id.simple().to_string()[..8]))
        .bind(format!("{user_id}@example.test"))
        .bind(FAKE_PASSWORD_HASH)
        .execute(pool)
        .await
        .expect("the user seeds");

    let project_id = seed_project(pool).await;
    let profile_id = seed_profile(pool, project_id).await;

    Fixture {
        user_id,
        project_id,
        profile_id,
    }
}

/// A project with the documented default state set already created.
async fn seed_project(pool: &PgPool) -> Uuid {
    let project_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id, name, remote_url) VALUES ($1, $2, $3)")
        .bind(project_id)
        .bind(format!("project-{project_id}"))
        // `.invalid` can never resolve (`CLAUDE.md`, rule 3).
        .bind("https://example.invalid/org/repo.git")
        .execute(pool)
        .await
        .expect("the project seeds");

    let repository = TaskRepository::new(pool);
    let mut tx = repository
        .begin_mutation(project_id)
        .await
        .expect("the mutation opens");
    repository
        .insert_default_states(&mut tx, project_id)
        .await
        .expect("the default states insert");
    tx.commit().await.expect("the transaction commits");

    project_id
}

async fn seed_profile(pool: &PgPool, project_id: Uuid) -> Uuid {
    let profile_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO agent_profiles (id, project_id, name, image, partial_messages)
         VALUES ($1, $2, $3, $4, TRUE)",
    )
    .bind(profile_id)
    .bind(project_id)
    .bind("default")
    .bind(TEST_IMAGE)
    .execute(pool)
    .await
    .expect("the profile seeds");

    profile_id
}

/// A session of this project, through the repository that owns `sessions`.
async fn seed_session(pool: &PgPool, project_id: Uuid, profile_id: Uuid) -> Uuid {
    let session = NewSession::new(
        project_id,
        profile_id,
        ProfileKind::Conversational,
        "main",
        // Not a credential: a fake stand-in for the hashed MCP token (rule 3).
        format!("fake-mcp-token-hash-{}", Uuid::new_v4()),
    );

    let mut tx = pool.begin().await.expect("a transaction begins");
    let inserted = SessionRepository::new(pool)
        .insert(&mut tx, &session)
        .await
        .expect("the session inserts");
    tx.commit().await.expect("the transaction commits");

    inserted.id
}

/// The id of a project's state by name.
async fn state_id(pool: &PgPool, project_id: Uuid, name: &str) -> Uuid {
    TaskRepository::new(pool)
        .find_state_by_name(project_id, name)
        .await
        .expect("the state reads")
        .expect("the state exists")
        .id
}

/// Insert `task` in its own committed tracker mutation.
async fn insert(pool: &PgPool, project_id: Uuid, task: &NewTask) -> Result<Task> {
    let repository = TaskRepository::new(pool);
    let mut tx = repository.begin_mutation(project_id).await?;
    let inserted = repository.insert_task(&mut tx, project_id, task).await?;
    tx.commit().await?;

    Ok(inserted)
}

/// A titled task on this project, inserted and committed.
async fn insert_titled(pool: &PgPool, project_id: Uuid, title: &str) -> Task {
    let task = NewTask::new(project_id, title).expect("the title parses");
    insert(pool, project_id, &task)
        .await
        .expect("the task inserts")
}

/// Run `body` inside a committed tracker mutation.
async fn in_mutation<T, F>(pool: &PgPool, project_id: Uuid, body: F) -> Result<T>
where
    F: AsyncFnOnce(&TaskRepository<'_>, &mut sqlx::PgConnection) -> Result<T>,
{
    let repository = TaskRepository::new(pool);
    let mut tx = repository.begin_mutation(project_id).await?;
    let outcome = body(&repository, &mut tx).await?;
    tx.commit().await?;

    Ok(outcome)
}

#[tokio::test]
async fn numbers_run_from_one_and_are_never_reused() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let repository = TaskRepository::new(&pool);

    let first = insert_titled(&pool, project_id, "first").await;
    let second = insert_titled(&pool, project_id, "second").await;
    let third = insert_titled(&pool, project_id, "third").await;
    assert_eq!([first.number, second.number, third.number], [1, 2, 3]);

    // The counter is the project's, not a count of its rows: deleting the
    // middle task leaves the next number at 4 (`SPEC.md`, "Tasks": "`number`
    // is assigned from the project's counter and never reused").
    let deleted = in_mutation(&pool, project_id, async |repository, tx| {
        repository.delete_task(tx, project_id, second.id).await
    })
    .await
    .unwrap();
    assert!(deleted);

    let fourth = insert_titled(&pool, project_id, "fourth").await;
    assert_eq!(fourth.number, 4);

    // A second project numbers from 1 of its own.
    let other_project = seed_project(&pool).await;
    assert_eq!(
        insert_titled(&pool, other_project, "theirs").await.number,
        1
    );

    // Deleting something that is not this project's task is `false`, not an
    // error and certainly not a deletion.
    let untouched = in_mutation(&pool, project_id, async |repository, tx| {
        repository.delete_task(tx, project_id, Uuid::new_v4()).await
    })
    .await
    .unwrap();
    assert!(!untouched);
    assert!(
        repository
            .find_task(project_id, TaskRef::Id(first.id))
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        repository
            .find_task(project_id, TaskRef::Id(second.id))
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn a_new_task_lands_in_the_default_queue_state() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;

    let backlog = state_id(&pool, project_id, "backlog").await;
    let review = state_id(&pool, project_id, "review").await;

    // No state given: the queue state with the lowest position.
    let defaulted = insert_titled(&pool, project_id, "defaulted").await;
    assert_eq!(defaulted.state_id, backlog);
    assert_eq!(defaulted.priority, Priority::MEDIUM.get());
    assert!(!defaulted.blocked);
    assert_eq!(defaulted.attempts, 0);
    assert!(defaulted.labels.is_empty());
    assert!(defaulted.closed_at.is_none());
    assert!(defaulted.lease_holder_session_id.is_none());

    // An explicit state of this project is taken as given.
    let mut explicit = NewTask::new(project_id, "explicit").unwrap();
    explicit.state_id = Some(review);
    explicit.priority = Priority::CRITICAL;
    explicit.description = "# plan\n\nwrite it".to_string();
    explicit.labels = Label::parse_list(&["backend", "p0"]).unwrap();
    explicit.created_by_user_id = Some(fixture.user_id);
    let explicit = insert(&pool, project_id, &explicit).await.unwrap();
    assert_eq!(explicit.state_id, review);
    assert_eq!(explicit.priority, 0);
    assert_eq!(explicit.labels, ["backend", "p0"]);
    assert_eq!(explicit.created_by_user_id, Some(fixture.user_id));

    // A state of another project is not a state this task can be in.
    let other_project = seed_project(&pool).await;
    let their_backlog = state_id(&pool, other_project, "backlog").await;
    let mut foreign = NewTask::new(project_id, "foreign state").unwrap();
    foreign.state_id = Some(their_backlog);
    let error = insert(&pool, project_id, &foreign).await.unwrap_err();
    assert_eq!(error.status(), StatusCode::BAD_REQUEST);
    assert_eq!(error.to_string(), "state must belong to this project");
}

#[tokio::test]
async fn a_project_with_no_queue_state_cannot_take_a_task() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;

    // `delete_state` refuses to remove the last queue state, so this is the
    // defensive path: the rows are taken out from under the repository.
    sqlx::query("DELETE FROM task_states WHERE project_id = $1 AND kind = 'queue'")
        .bind(project_id)
        .execute(&pool)
        .await
        .expect("the queue states delete");

    let task = NewTask::new(project_id, "nowhere to go").unwrap();
    let error = insert(&pool, project_id, &task).await.unwrap_err();
    assert_eq!(error.status(), StatusCode::CONFLICT);
    assert_eq!(error.to_string(), "project has no queue state");

    // Nothing was allocated on the way out: the next successful insert on a
    // repaired project still gets number 1.
    let human = state_id(&pool, project_id, "needs_human").await;
    let mut repaired = NewTask::new(project_id, "somewhere to go").unwrap();
    repaired.state_id = Some(human);
    assert_eq!(
        insert(&pool, project_id, &repaired).await.unwrap().number,
        1
    );
}

#[tokio::test]
async fn nesting_is_one_level_deep_on_creation_and_on_re_parenting() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;

    let epic = insert_titled(&pool, project_id, "the epic").await;

    // The allowed case: a top-level task of this project may be a parent.
    let mut child = NewTask::new(project_id, "a child").unwrap();
    child.parent_id = Some(epic.id);
    let child = insert(&pool, project_id, &child).await.unwrap();
    assert_eq!(child.parent_id, Some(epic.id));

    // A parent of another project is not a parent at all.
    let other_project = seed_project(&pool).await;
    let theirs = insert_titled(&pool, other_project, "their epic").await;
    let mut foreign = NewTask::new(project_id, "cross-project child").unwrap();
    foreign.parent_id = Some(theirs.id);
    let error = insert(&pool, project_id, &foreign).await.unwrap_err();
    assert_eq!(error.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        error.to_string(),
        "parent must be a top-level task of the same project"
    );

    // A task that already has a parent cannot receive children.
    let mut grandchild = NewTask::new(project_id, "a grandchild").unwrap();
    grandchild.parent_id = Some(child.id);
    let error = insert(&pool, project_id, &grandchild).await.unwrap_err();
    assert_eq!(error.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        error.to_string(),
        "parent must be a top-level task of the same project"
    );

    // Re-parenting enforces the same rule from the other end.
    let loose = insert_titled(&pool, project_id, "loose").await;
    let error = in_mutation(&pool, project_id, async |repository, tx| {
        repository
            .update_task(
                tx,
                project_id,
                loose.id,
                &TaskUpdate {
                    parent_id: Some(Some(child.id)),
                    ..TaskUpdate::default()
                },
            )
            .await
    })
    .await
    .unwrap_err();
    assert_eq!(error.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        error.to_string(),
        "a task with a parent cannot receive children"
    );

    // And an epic cannot become somebody's child, terminal children included.
    let other_epic = insert_titled(&pool, project_id, "another epic").await;
    let error = in_mutation(&pool, project_id, async |repository, tx| {
        repository
            .update_task(
                tx,
                project_id,
                epic.id,
                &TaskUpdate {
                    parent_id: Some(Some(other_epic.id)),
                    ..TaskUpdate::default()
                },
            )
            .await
    })
    .await
    .unwrap_err();
    assert_eq!(error.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        error.to_string(),
        "a task with children cannot get a parent"
    );

    // A task is never its own parent.
    let error = in_mutation(&pool, project_id, async |repository, tx| {
        repository
            .update_task(
                tx,
                project_id,
                loose.id,
                &TaskUpdate {
                    parent_id: Some(Some(loose.id)),
                    ..TaskUpdate::default()
                },
            )
            .await
    })
    .await
    .unwrap_err();
    assert_eq!(error.status(), StatusCode::BAD_REQUEST);
    assert_eq!(error.to_string(), "a task cannot be its own parent");

    // Moving a child out to the top level is always allowed.
    let detached = in_mutation(&pool, project_id, async |repository, tx| {
        repository
            .update_task(
                tx,
                project_id,
                child.id,
                &TaskUpdate {
                    parent_id: Some(None),
                    ..TaskUpdate::default()
                },
            )
            .await
    })
    .await
    .unwrap()
    .expect("detaching is a change");
    assert!(detached.parent_id.is_none());

    let children = TaskRepository::new(&pool)
        .list_children(project_id, epic.id)
        .await
        .unwrap();
    assert!(children.is_empty());
}

#[tokio::test]
async fn an_update_that_changes_nothing_writes_nothing() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let repository = TaskRepository::new(&pool);

    let mut new = NewTask::new(project_id, "as stored").unwrap();
    new.labels = Label::parse_list(&["backend"]).unwrap();
    let task = insert(&pool, project_id, &new).await.unwrap();

    // Every supplied value already equals the stored one, so the row is not
    // written and `updated_at` does not move (ADR 0030).
    let identical = TaskUpdate {
        title: Some(TaskTitle::parse("as stored").unwrap()),
        description: Some(String::new()),
        priority: Some(Priority::MEDIUM),
        labels: Some(Label::parse_list(&["backend"]).unwrap()),
        assignee_user_id: Some(None),
        parent_id: Some(None),
    };
    assert!(!identical.is_empty());
    let unchanged = in_mutation(&pool, project_id, async |repository, tx| {
        repository
            .update_task(tx, project_id, task.id, &identical)
            .await
    })
    .await
    .unwrap();
    assert!(unchanged.is_none(), "an identical update is not a change");
    assert_eq!(
        repository
            .find_task(project_id, TaskRef::Id(task.id))
            .await
            .unwrap()
            .unwrap()
            .updated_at,
        task.updated_at,
    );

    // An empty update is the same answer by a shorter route.
    let empty = in_mutation(&pool, project_id, async |repository, tx| {
        repository
            .update_task(tx, project_id, task.id, &TaskUpdate::default())
            .await
    })
    .await
    .unwrap();
    assert!(empty.is_none());

    // One differing field is enough to make it a change, and only the fields
    // that were mentioned move.
    let changed = in_mutation(&pool, project_id, async |repository, tx| {
        repository
            .update_task(
                tx,
                project_id,
                task.id,
                &TaskUpdate {
                    priority: Some(Priority::HIGH),
                    assignee_user_id: Some(Some(fixture.user_id)),
                    ..TaskUpdate::default()
                },
            )
            .await
    })
    .await
    .unwrap()
    .expect("a differing field is a change");
    assert_eq!(changed.priority, Priority::HIGH.get());
    assert_eq!(changed.assignee_user_id, Some(fixture.user_id));
    assert_eq!(changed.title, "as stored");
    assert_eq!(changed.labels, ["backend"]);
    assert!(changed.updated_at > task.updated_at);

    // Clearing an optional column is a change; asking for it twice is not.
    let cleared = in_mutation(&pool, project_id, async |repository, tx| {
        repository
            .update_task(
                tx,
                project_id,
                task.id,
                &TaskUpdate {
                    assignee_user_id: Some(None),
                    labels: Some(Vec::new()),
                    ..TaskUpdate::default()
                },
            )
            .await
    })
    .await
    .unwrap()
    .expect("clearing is a change");
    assert!(cleared.assignee_user_id.is_none());
    assert!(cleared.labels.is_empty());

    // An unknown task is 404, whatever the update says.
    let error = in_mutation(&pool, project_id, async |repository, tx| {
        repository
            .update_task(tx, project_id, Uuid::new_v4(), &TaskUpdate::default())
            .await
    })
    .await
    .unwrap_err();
    assert_eq!(error.status(), StatusCode::NOT_FOUND);

    // So is a task of another project, which is what keeps the scope honest.
    let other_project = seed_project(&pool).await;
    let theirs = insert_titled(&pool, other_project, "theirs").await;
    let error = in_mutation(&pool, project_id, async |repository, tx| {
        repository
            .update_task(
                tx,
                project_id,
                theirs.id,
                &TaskUpdate {
                    priority: Some(Priority::LOW),
                    ..TaskUpdate::default()
                },
            )
            .await
    })
    .await
    .unwrap_err();
    assert_eq!(error.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_state_fields_move_together_and_stay_in_scope() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let session_id = seed_session(&pool, project_id, fixture.profile_id).await;
    let done = state_id(&pool, project_id, "done").await;

    let task = insert_titled(&pool, project_id, "moves about").await;
    let claimed_at = chrono::Utc::now();

    // A claim: the lease, the attempt count, nothing else.
    let claimed = in_mutation(&pool, project_id, async |repository, tx| {
        repository
            .set_task_state_fields(
                tx,
                project_id,
                task.id,
                &StateFields {
                    lease: Some(Some((session_id, claimed_at))),
                    attempts: Some(1),
                    ..StateFields::default()
                },
            )
            .await
    })
    .await
    .unwrap();
    assert_eq!(claimed.lease_holder_session_id, Some(session_id));
    assert!(claimed.lease_since.is_some());
    assert_eq!(claimed.attempts, 1);
    assert_eq!(claimed.state_id, task.state_id);

    assert_eq!(
        TaskRepository::new(&pool)
            .list_by_lease_holder(session_id)
            .await
            .unwrap()
            .into_iter()
            .map(|held| held.id)
            .collect::<Vec<_>>(),
        [task.id],
    );

    // A terminal move: the state and `closed_at`, and the lease cleared. Both
    // lease columns go together, which is what the table's `CHECK` demands.
    let closed = in_mutation(&pool, project_id, async |repository, tx| {
        repository
            .set_task_state_fields(
                tx,
                project_id,
                task.id,
                &StateFields {
                    state_id: Some(done),
                    lease: Some(None),
                    attempts: Some(0),
                    closed_at: Some(Some(chrono::Utc::now())),
                    blocked: Some(true),
                    needs_human_reason: Some(Some("ran out of attempts".into())),
                    ..StateFields::default()
                },
            )
            .await
    })
    .await
    .unwrap();
    assert_eq!(closed.state_id, done);
    assert!(closed.lease_holder_session_id.is_none());
    assert!(closed.lease_since.is_none());
    assert_eq!(closed.attempts, 0);
    assert!(closed.closed_at.is_some());
    assert!(closed.blocked);
    assert_eq!(
        closed.needs_human_reason.as_deref(),
        Some("ran out of attempts")
    );

    assert!(
        TaskRepository::new(&pool)
            .list_by_lease_holder(session_id)
            .await
            .unwrap()
            .is_empty()
    );

    // Reopening clears what closing set, and leaves everything unmentioned.
    let reopened = in_mutation(&pool, project_id, async |repository, tx| {
        repository
            .set_task_state_fields(
                tx,
                project_id,
                task.id,
                &StateFields {
                    state_id: Some(task.state_id),
                    closed_at: Some(None),
                    needs_human_reason: Some(None),
                    blocked: Some(false),
                    ..StateFields::default()
                },
            )
            .await
    })
    .await
    .unwrap();
    assert!(reopened.closed_at.is_none());
    assert!(reopened.needs_human_reason.is_none());
    assert!(!reopened.blocked);
    assert_eq!(reopened.attempts, 0);

    // A state of another project would take the task off this board.
    let other_project = seed_project(&pool).await;
    let their_done = state_id(&pool, other_project, "done").await;
    let error = in_mutation(&pool, project_id, async |repository, tx| {
        repository
            .set_task_state_fields(
                tx,
                project_id,
                task.id,
                &StateFields {
                    state_id: Some(their_done),
                    ..StateFields::default()
                },
            )
            .await
    })
    .await
    .unwrap_err();
    assert_eq!(error.status(), StatusCode::BAD_REQUEST);
    assert_eq!(error.to_string(), "state must belong to this project");

    // A hand-off of another task is not this task's current record.
    let other_task = insert_titled(&pool, project_id, "the other task").await;
    let handoff_id = publish_handoff(&pool, project_id, other_task.id, session_id, COMMIT).await;
    let error = in_mutation(&pool, project_id, async |repository, tx| {
        repository
            .set_task_state_fields(
                tx,
                project_id,
                task.id,
                &StateFields {
                    current_handoff_id: Some(Some(handoff_id)),
                    ..StateFields::default()
                },
            )
            .await
    })
    .await
    .unwrap_err();
    assert_eq!(error.status(), StatusCode::BAD_REQUEST);
    assert_eq!(error.to_string(), "hand-off must belong to this task");

    // And an unknown task is 404 rather than a silent no-op.
    let error = in_mutation(&pool, project_id, async |repository, tx| {
        repository
            .set_task_state_fields(
                tx,
                project_id,
                Uuid::new_v4(),
                &StateFields {
                    blocked: Some(true),
                    ..StateFields::default()
                },
            )
            .await
    })
    .await
    .unwrap_err();
    assert_eq!(error.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_task_is_found_by_its_uuid_or_its_number_and_only_in_its_project() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let repository = TaskRepository::new(&pool);

    let task = insert_titled(&pool, project_id, "addressable").await;

    for task_ref in [TaskRef::Id(task.id), TaskRef::Number(task.number)] {
        let found = repository
            .find_task(project_id, task_ref)
            .await
            .unwrap()
            .expect("the task is found");
        assert_eq!(found.id, task.id);
    }

    // Both forms parse out of a path segment.
    assert_eq!(
        TaskRef::from_str(&task.id.to_string()).unwrap(),
        TaskRef::Id(task.id)
    );
    assert_eq!(
        TaskRef::from_str(&task.number.to_string()).unwrap(),
        TaskRef::Number(task.number)
    );

    // A number nobody has is `None`, never an error about parsing a UUID.
    assert!(
        repository
            .find_task(project_id, TaskRef::Number(9_999))
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        repository
            .find_task(project_id, TaskRef::Id(Uuid::new_v4()))
            .await
            .unwrap()
            .is_none()
    );

    // The number is only unique within a project: another project's task 1 is
    // not this project's, by either reference.
    let other_project = seed_project(&pool).await;
    let theirs = insert_titled(&pool, other_project, "theirs").await;
    assert_eq!(theirs.number, task.number);
    assert_eq!(
        repository
            .find_task(project_id, TaskRef::Number(theirs.number))
            .await
            .unwrap()
            .unwrap()
            .id,
        task.id,
    );
    assert!(
        repository
            .find_task(project_id, TaskRef::Id(theirs.id))
            .await
            .unwrap()
            .is_none()
    );

    // The locking read answers the same questions.
    let mut tx = repository.begin_mutation(project_id).await.unwrap();
    assert_eq!(
        repository
            .find_task_for_update(&mut tx, project_id, TaskRef::Number(task.number))
            .await
            .unwrap()
            .unwrap()
            .id,
        task.id,
    );
    assert!(
        repository
            .find_task_for_update(&mut tx, project_id, TaskRef::Id(theirs.id))
            .await
            .unwrap()
            .is_none()
    );
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn the_board_reads_filter_and_order_as_documented() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let repository = TaskRepository::new(&pool);
    let session_id = seed_session(&pool, project_id, fixture.profile_id).await;

    let review = state_id(&pool, project_id, "review").await;
    let human = state_id(&pool, project_id, "needs_human").await;

    // Inserted in an order that is not the order they come back in.
    let mut low = NewTask::new(project_id, "low").unwrap();
    low.priority = Priority::LOW;
    low.labels = Label::parse_list(&["backend"]).unwrap();
    let low = insert(&pool, project_id, &low).await.unwrap();

    let mut critical = NewTask::new(project_id, "critical").unwrap();
    critical.priority = Priority::CRITICAL;
    critical.state_id = Some(review);
    let critical = insert(&pool, project_id, &critical).await.unwrap();

    let mut also_low = NewTask::new(project_id, "also low").unwrap();
    also_low.priority = Priority::LOW;
    also_low.labels = Label::parse_list(&["backend", "infra"]).unwrap();
    let also_low = insert(&pool, project_id, &also_low).await.unwrap();

    let mut escalated = NewTask::new(project_id, "escalated").unwrap();
    escalated.state_id = Some(human);
    let escalated = insert(&pool, project_id, &escalated).await.unwrap();

    let ids = |tasks: Vec<Task>| tasks.into_iter().map(|task| task.id).collect::<Vec<_>>();

    // Priority first, then number: `low` was inserted before `also low`.
    assert_eq!(
        ids(repository
            .list_tasks(project_id, &TaskFilter::default())
            .await
            .unwrap()),
        [critical.id, escalated.id, low.id, also_low.id],
    );

    assert_eq!(
        ids(repository
            .list_tasks(
                project_id,
                &TaskFilter {
                    state_id: Some(review),
                    ..TaskFilter::default()
                }
            )
            .await
            .unwrap()),
        [critical.id],
    );

    assert_eq!(
        ids(repository
            .list_tasks(
                project_id,
                &TaskFilter {
                    label: Some("infra".into()),
                    ..TaskFilter::default()
                }
            )
            .await
            .unwrap()),
        [also_low.id],
    );
    assert_eq!(
        ids(repository
            .list_tasks(
                project_id,
                &TaskFilter {
                    label: Some("backend".into()),
                    ..TaskFilter::default()
                }
            )
            .await
            .unwrap()),
        [low.id, also_low.id],
    );
    // An unknown label matches nothing rather than failing.
    assert!(
        repository
            .list_tasks(
                project_id,
                &TaskFilter {
                    label: Some("nonexistent".into()),
                    ..TaskFilter::default()
                }
            )
            .await
            .unwrap()
            .is_empty()
    );

    assert_eq!(
        ids(repository
            .list_tasks(
                project_id,
                &TaskFilter {
                    priority: Some(Priority::LOW.get()),
                    ..TaskFilter::default()
                }
            )
            .await
            .unwrap()),
        [low.id, also_low.id],
    );

    // `held` splits the board by whether anybody has a lease.
    in_mutation(&pool, project_id, async |repository, tx| {
        repository
            .set_task_state_fields(
                tx,
                project_id,
                critical.id,
                &StateFields {
                    lease: Some(Some((session_id, chrono::Utc::now()))),
                    ..StateFields::default()
                },
            )
            .await
    })
    .await
    .unwrap();

    assert_eq!(
        ids(repository
            .list_tasks(
                project_id,
                &TaskFilter {
                    held: Some(true),
                    ..TaskFilter::default()
                }
            )
            .await
            .unwrap()),
        [critical.id],
    );
    assert_eq!(
        ids(repository
            .list_tasks(
                project_id,
                &TaskFilter {
                    held: Some(false),
                    ..TaskFilter::default()
                }
            )
            .await
            .unwrap()),
        [escalated.id, low.id, also_low.id],
    );

    // `parent` filters to an epic's children, which is also what
    // `list_children` answers.
    in_mutation(&pool, project_id, async |repository, tx| {
        repository
            .update_task(
                tx,
                project_id,
                also_low.id,
                &TaskUpdate {
                    parent_id: Some(Some(low.id)),
                    ..TaskUpdate::default()
                },
            )
            .await
    })
    .await
    .unwrap();
    assert_eq!(
        ids(repository
            .list_tasks(
                project_id,
                &TaskFilter {
                    parent_id: Some(low.id),
                    ..TaskFilter::default()
                }
            )
            .await
            .unwrap()),
        [also_low.id],
    );
    assert_eq!(
        ids(repository.list_children(project_id, low.id).await.unwrap()),
        [also_low.id],
    );

    // The dashboard read crosses projects, and only picks up human states.
    let other_project = seed_project(&pool).await;
    let their_human = state_id(&pool, other_project, "needs_human").await;
    let mut theirs = NewTask::new(other_project, "theirs, escalated").unwrap();
    theirs.state_id = Some(their_human);
    theirs.priority = Priority::CRITICAL;
    let theirs = insert(&pool, other_project, &theirs).await.unwrap();

    let waiting = repository
        .list_tasks_by_state_kind(TaskStateKind::Human)
        .await
        .unwrap();
    let waiting_ids = ids(waiting);
    assert_eq!(waiting_ids.len(), 2);
    // Priority orders the dashboard, so the critical one leads.
    assert_eq!(waiting_ids[0], theirs.id);
    assert!(waiting_ids.contains(&escalated.id));
    assert!(!waiting_ids.contains(&critical.id));
}

#[tokio::test]
async fn dependency_edges_are_scoped_keyed_by_kind_and_inserted_once() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let repository = TaskRepository::new(&pool);

    let blocked = insert_titled(&pool, project_id, "waits").await;
    let blocker = insert_titled(&pool, project_id, "must finish first").await;

    let edge = in_mutation(&pool, project_id, async |repository, tx| {
        repository
            .insert_dependency(
                tx,
                project_id,
                blocked.id,
                blocker.id,
                TaskDependencyKind::Blocks,
            )
            .await
    })
    .await
    .unwrap();
    assert_eq!(edge.kind, TaskDependencyKind::Blocks);

    // The same pair with a different kind is a different edge, because `kind`
    // is part of the primary key.
    in_mutation(&pool, project_id, async |repository, tx| {
        repository
            .insert_dependency(
                tx,
                project_id,
                blocked.id,
                blocker.id,
                TaskDependencyKind::DiscoveredFrom,
            )
            .await
    })
    .await
    .unwrap();

    let edges = repository
        .list_dependencies(project_id, blocked.id)
        .await
        .unwrap();
    assert_eq!(edges.len(), 2);
    assert!(
        edges
            .iter()
            .all(|edge| edge.depends_on_task_id == blocker.id)
    );

    // The same edge twice is a conflict.
    let error = in_mutation(&pool, project_id, async |repository, tx| {
        repository
            .insert_dependency(
                tx,
                project_id,
                blocked.id,
                blocker.id,
                TaskDependencyKind::Blocks,
            )
            .await
    })
    .await
    .unwrap_err();
    assert_eq!(error.status(), StatusCode::CONFLICT);
    assert_eq!(error.to_string(), "dependency already exists");

    // A task cannot depend on itself.
    let error = in_mutation(&pool, project_id, async |repository, tx| {
        repository
            .insert_dependency(
                tx,
                project_id,
                blocked.id,
                blocked.id,
                TaskDependencyKind::Blocks,
            )
            .await
    })
    .await
    .unwrap_err();
    assert_eq!(error.status(), StatusCode::BAD_REQUEST);
    assert_eq!(error.to_string(), "a task cannot depend on itself");

    // Neither end may be another project's task, in either position.
    let other_project = seed_project(&pool).await;
    let theirs = insert_titled(&pool, other_project, "theirs").await;
    for (task_id, depends_on) in [(blocked.id, theirs.id), (theirs.id, blocker.id)] {
        let error = in_mutation(&pool, project_id, async |repository, tx| {
            repository
                .insert_dependency(
                    tx,
                    project_id,
                    task_id,
                    depends_on,
                    TaskDependencyKind::Related,
                )
                .await
        })
        .await
        .unwrap_err();
        assert_eq!(error.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            error.to_string(),
            "dependency must reference tasks of the same project"
        );
    }

    // "Who is waiting on me", per kind.
    assert_eq!(
        repository
            .list_dependants(project_id, blocker.id, TaskDependencyKind::Blocks)
            .await
            .unwrap(),
        [blocked.id],
    );
    assert!(
        repository
            .list_dependants(project_id, blocker.id, TaskDependencyKind::Related)
            .await
            .unwrap()
            .is_empty()
    );
    // The same answer from inside a mutation, which is where a recomputation
    // or a deletion has to read it.
    let in_tx = in_mutation(&pool, project_id, async |repository, tx| {
        repository
            .list_dependants_in_tx(tx, project_id, blocker.id, TaskDependencyKind::Blocks)
            .await
    })
    .await
    .unwrap();
    assert_eq!(in_tx, [blocked.id]);

    // Removing one kind leaves the other standing.
    let removed = in_mutation(&pool, project_id, async |repository, tx| {
        repository
            .delete_dependency(
                tx,
                project_id,
                blocked.id,
                blocker.id,
                TaskDependencyKind::Blocks,
            )
            .await
    })
    .await
    .unwrap();
    assert!(removed);

    let edges = repository
        .list_dependencies(project_id, blocked.id)
        .await
        .unwrap();
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].kind, TaskDependencyKind::DiscoveredFrom);

    // Removing it again, or removing another project's edge, is `false`.
    for project in [project_id, other_project] {
        let removed = in_mutation(&pool, project, async |repository, tx| {
            repository
                .delete_dependency(
                    tx,
                    project,
                    blocked.id,
                    blocker.id,
                    TaskDependencyKind::Blocks,
                )
                .await
        })
        .await
        .unwrap();
        assert!(!removed);
    }

    // Deleting a prerequisite takes its edges with it.
    in_mutation(&pool, project_id, async |repository, tx| {
        repository.delete_task(tx, project_id, blocker.id).await
    })
    .await
    .unwrap();
    assert!(
        repository
            .list_dependencies(project_id, blocked.id)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn a_comment_has_one_author_or_none_and_belongs_to_this_project() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let repository = TaskRepository::new(&pool);
    let session_id = seed_session(&pool, project_id, fixture.profile_id).await;

    let task = insert_titled(&pool, project_id, "discussed").await;

    let from_user = NewTaskComment::from_user(task.id, fixture.user_id, "looks right to me");
    let from_session = NewTaskComment::from_session(task.id, session_id, "pushed a fix");
    let from_system = NewTaskComment::from_system(task.id, "released: the session ended");

    for comment in [&from_user, &from_session, &from_system] {
        in_mutation(&pool, project_id, async |repository, tx| {
            repository.insert_comment(tx, project_id, comment).await
        })
        .await
        .unwrap();
    }

    let comments = repository.list_comments(project_id, task.id).await.unwrap();
    assert_eq!(
        comments.iter().map(|c| c.id).collect::<Vec<_>>(),
        [from_user.id, from_session.id, from_system.id],
    );
    assert_eq!(comments[0].author_user_id, Some(fixture.user_id));
    assert_eq!(comments[1].author_session_id, Some(session_id));
    assert!(comments[2].system);
    assert!(comments[2].author_user_id.is_none());
    assert!(comments[2].author_session_id.is_none());

    // Two authors, no author, and an author on a system comment.
    let mut two_authors = NewTaskComment::from_user(task.id, fixture.user_id, "who wrote this?");
    two_authors.author_session_id = Some(session_id);

    let mut no_author = NewTaskComment::from_user(task.id, fixture.user_id, "nobody wrote this");
    no_author.author_user_id = None;

    let mut authored_system = NewTaskComment::from_system(task.id, "escalated");
    authored_system.author_user_id = Some(fixture.user_id);

    for comment in [&two_authors, &no_author, &authored_system] {
        let error = in_mutation(&pool, project_id, async |repository, tx| {
            repository.insert_comment(tx, project_id, comment).await
        })
        .await
        .unwrap_err();
        assert_eq!(error.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            error.to_string(),
            "a comment has exactly one author, and a system comment has none"
        );
    }

    // An empty body is rejected before any row is written.
    let empty = NewTaskComment::from_user(task.id, fixture.user_id, "   \n ");
    let error = in_mutation(&pool, project_id, async |repository, tx| {
        repository.insert_comment(tx, project_id, &empty).await
    })
    .await
    .unwrap_err();
    assert_eq!(error.status(), StatusCode::BAD_REQUEST);
    assert_eq!(error.to_string(), "comment body must not be empty");

    // A task outside this project is 404, whichever way round it is asked.
    let other_project = seed_project(&pool).await;
    let theirs = insert_titled(&pool, other_project, "theirs").await;
    let crossing = NewTaskComment::from_user(theirs.id, fixture.user_id, "not mine to comment on");
    let error = in_mutation(&pool, project_id, async |repository, tx| {
        repository.insert_comment(tx, project_id, &crossing).await
    })
    .await
    .unwrap_err();
    assert_eq!(error.status(), StatusCode::NOT_FOUND);

    assert!(
        repository
            .list_comments(other_project, task.id)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        repository
            .list_comments(project_id, task.id)
            .await
            .unwrap()
            .len(),
        3,
    );
}

/// Publish an unreviewed revision on `task_id`, with its required comment.
///
/// Returns the hand-off's id. The comment and the record commit together, as
/// they do in the real publication path.
async fn publish_handoff(
    pool: &PgPool,
    project_id: Uuid,
    task_id: Uuid,
    session_id: Uuid,
    commit: &str,
) -> Uuid {
    let comment = NewTaskComment::from_session(task_id, session_id, "implemented and pushed");
    let mut handoff =
        NewTaskHandoff::new(task_id, format!("session/{session_id}"), commit, comment.id);
    handoff.source_session_id = Some(session_id);
    handoff.created_by_session_id = Some(session_id);

    in_mutation(pool, project_id, async |repository, tx| {
        repository
            .insert_comment(&mut *tx, project_id, &comment)
            .await?;
        repository.insert_handoff(tx, project_id, &handoff).await
    })
    .await
    .expect("the hand-off publishes")
    .id
}

#[tokio::test]
async fn a_handoff_is_validated_against_its_task_comment_and_sessions() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let repository = TaskRepository::new(&pool);
    let session_id = seed_session(&pool, project_id, fixture.profile_id).await;

    let task = insert_titled(&pool, project_id, "handed off").await;

    let published = publish_handoff(&pool, project_id, task.id, session_id, COMMIT).await;
    let stored = repository
        .find_handoff(project_id, published)
        .await
        .unwrap()
        .expect("the record is there");
    assert_eq!(stored.task_id, task.id);
    assert_eq!(stored.commit, COMMIT);
    assert_eq!(stored.review_status, ReviewStatus::Unreviewed);
    assert_eq!(stored.source_session_id, Some(session_id));
    assert!(stored.reviewed_at.is_none());

    // A forward carrying an explicit decision round-trips its review columns,
    // which is the one `TEXT`-with-a-`CHECK` enum in the schema.
    let review_comment = NewTaskComment::from_user(task.id, fixture.user_id, "approved");
    let mut forwarded = NewTaskHandoff::new(
        task.id,
        stored.source_branch.clone(),
        stored.commit.clone(),
        review_comment.id,
    );
    forwarded.source_session_id = stored.source_session_id;
    forwarded.created_by_user_id = Some(fixture.user_id);
    forwarded.review_status = ReviewStatus::Approved;
    forwarded.reviewed_by_user_id = Some(fixture.user_id);
    forwarded.reviewed_at = Some(chrono::Utc::now());

    let forwarded_id = in_mutation(&pool, project_id, async |repository, tx| {
        repository
            .insert_comment(&mut *tx, project_id, &review_comment)
            .await?;
        repository.insert_handoff(tx, project_id, &forwarded).await
    })
    .await
    .unwrap()
    .id;

    let stored_forward = repository
        .find_handoff(project_id, forwarded_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored_forward.review_status, ReviewStatus::Approved);
    assert_eq!(stored_forward.reviewed_by_user_id, Some(fixture.user_id));
    assert!(stored_forward.reviewed_at.is_some());

    // Oldest first, and the current record is whichever the task names.
    let history = repository.list_handoffs(project_id, task.id).await.unwrap();
    assert_eq!(
        history.iter().map(|h| h.id).collect::<Vec<_>>(),
        [published, forwarded_id],
    );

    let current = in_mutation(&pool, project_id, async |repository, tx| {
        repository
            .set_task_state_fields(
                tx,
                project_id,
                task.id,
                &StateFields {
                    current_handoff_id: Some(Some(forwarded_id)),
                    ..StateFields::default()
                },
            )
            .await
    })
    .await
    .unwrap();
    assert_eq!(current.current_handoff_id, Some(forwarded_id));

    // The model's own rules still run: an abbreviated commit is not a commit.
    let comment = NewTaskComment::from_user(task.id, fixture.user_id, "half a commit");
    let mut abbreviated = NewTaskHandoff::new(task.id, "session/x", &COMMIT[..7], comment.id);
    abbreviated.created_by_user_id = Some(fixture.user_id);
    let error = in_mutation(&pool, project_id, async |repository, tx| {
        repository
            .insert_comment(&mut *tx, project_id, &comment)
            .await?;
        repository
            .insert_handoff(tx, project_id, &abbreviated)
            .await
    })
    .await
    .unwrap_err();
    assert_eq!(error.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        error.to_string(),
        "commit must be a full lowercase hexadecimal git object id"
    );

    // A comment of another task is not this hand-off's comment.
    let other_task = insert_titled(&pool, project_id, "somewhere else").await;
    let elsewhere =
        NewTaskComment::from_user(other_task.id, fixture.user_id, "about the other task");
    let mut wrong_comment = NewTaskHandoff::new(task.id, "session/x", COMMIT, elsewhere.id);
    wrong_comment.created_by_user_id = Some(fixture.user_id);
    let error = in_mutation(&pool, project_id, async |repository, tx| {
        repository
            .insert_comment(&mut *tx, project_id, &elsewhere)
            .await?;
        repository
            .insert_handoff(tx, project_id, &wrong_comment)
            .await
    })
    .await
    .unwrap_err();
    assert_eq!(error.status(), StatusCode::BAD_REQUEST);
    assert_eq!(error.to_string(), "comment must belong to this task");

    // A session of another project cannot be the source of this project's
    // work.
    let other_project = seed_project(&pool).await;
    let their_profile = seed_profile(&pool, other_project).await;
    let their_session = seed_session(&pool, other_project, their_profile).await;

    let comment = NewTaskComment::from_user(task.id, fixture.user_id, "whose branch is this?");
    let mut foreign_session = NewTaskHandoff::new(task.id, "session/x", COMMIT, comment.id);
    foreign_session.source_session_id = Some(their_session);
    foreign_session.created_by_user_id = Some(fixture.user_id);
    let error = in_mutation(&pool, project_id, async |repository, tx| {
        repository
            .insert_comment(&mut *tx, project_id, &comment)
            .await?;
        repository
            .insert_handoff(tx, project_id, &foreign_session)
            .await
    })
    .await
    .unwrap_err();
    assert_eq!(error.status(), StatusCode::BAD_REQUEST);
    assert_eq!(error.to_string(), "sessions must belong to this project");

    // A task of another project is 404, and the record is not visible from
    // one either.
    let theirs = insert_titled(&pool, other_project, "theirs").await;
    let comment = NewTaskComment::from_user(theirs.id, fixture.user_id, "not mine");
    let mut crossing = NewTaskHandoff::new(theirs.id, "session/x", COMMIT, comment.id);
    crossing.created_by_user_id = Some(fixture.user_id);
    let error = in_mutation(&pool, project_id, async |repository, tx| {
        repository.insert_handoff(tx, project_id, &crossing).await
    })
    .await
    .unwrap_err();
    assert_eq!(error.status(), StatusCode::NOT_FOUND);

    assert!(
        repository
            .find_handoff(other_project, published)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        repository
            .list_handoffs(other_project, task.id)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn a_session_link_keeps_its_first_touch_and_advances_the_last() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let repository = TaskRepository::new(&pool);
    let session_id = seed_session(&pool, project_id, fixture.profile_id).await;
    let other_session = seed_session(&pool, project_id, fixture.profile_id).await;

    let task = insert_titled(&pool, project_id, "worked on").await;
    let other_task = insert_titled(&pool, project_id, "also worked on").await;

    // Twice in one transaction is one touch: `NOW()` is the transaction's
    // start time, so nothing moves the second time either.
    let (first, again) = in_mutation(&pool, project_id, async |repository, tx| {
        let first = repository
            .touch_task_session(&mut *tx, task.id, session_id)
            .await?;
        let again = repository
            .touch_task_session(tx, task.id, session_id)
            .await?;
        Ok((first, again))
    })
    .await
    .unwrap();
    assert_eq!(first.first_touched_at, again.first_touched_at);
    assert_eq!(first.last_touched_at, again.last_touched_at);
    assert_eq!(first.first_touched_at, first.last_touched_at);

    sleep(BETWEEN_TOUCHES).await;

    // A later transaction moves `last_touched_at` and leaves the first alone.
    let later = in_mutation(&pool, project_id, async |repository, tx| {
        repository.touch_task_session(tx, task.id, session_id).await
    })
    .await
    .unwrap();
    assert_eq!(later.first_touched_at, first.first_touched_at);
    assert!(later.last_touched_at > first.last_touched_at);

    // A second session on the same task is its own link.
    in_mutation(&pool, project_id, async |repository, tx| {
        repository
            .touch_task_session(tx, task.id, other_session)
            .await
    })
    .await
    .unwrap();

    let links = repository.list_task_sessions(task.id).await.unwrap();
    assert_eq!(
        links.iter().map(|link| link.session_id).collect::<Vec<_>>(),
        [session_id, other_session],
    );
    assert_eq!(links[0].first_touched_at, first.first_touched_at);

    sleep(BETWEEN_TOUCHES).await;

    // The other direction: what did this session work on, most recent first.
    in_mutation(&pool, project_id, async |repository, tx| {
        repository
            .touch_task_session(tx, other_task.id, session_id)
            .await
    })
    .await
    .unwrap();

    assert_eq!(
        repository
            .list_tasks_for_session(session_id)
            .await
            .unwrap()
            .into_iter()
            .map(|task| task.id)
            .collect::<Vec<_>>(),
        [other_task.id, task.id],
    );
    assert_eq!(
        repository
            .list_tasks_for_session(other_session)
            .await
            .unwrap()
            .into_iter()
            .map(|task| task.id)
            .collect::<Vec<_>>(),
        [task.id],
    );

    // Deleting the task takes its links with it.
    in_mutation(&pool, project_id, async |repository, tx| {
        repository.delete_task(tx, project_id, task.id).await
    })
    .await
    .unwrap();
    assert!(
        repository
            .list_task_sessions(task.id)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        repository
            .list_tasks_for_session(session_id)
            .await
            .unwrap()
            .len(),
        1,
    );
}
