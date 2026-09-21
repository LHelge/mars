//! Task rows, edges, comments, hand-offs and session links against a real
//! Postgres (`CLAUDE.md`, "Testing expectations").
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
//! Everything that writes a task's columns is driven through the tracker verbs
//! — `create_task`, `update_task`, `delete_task` and the lease's `claim` — and
//! the session links through the [`TrackerMutation`] seam that writes them on
//! commit: the row helpers behind `tracker/` are the tracker's alone
//! (`ARCHITECTURE.md`, "Task tracker"). What stays at the repository is what
//! has no verb of its own: the edge, comment and hand-off inserts, whose
//! cross-table scope rules are the thing under test, and the reads.
//!
//! The state columns themselves — the lease, `attempts`, `closed_at` and the
//! closure the terminal line causes — belong to `tests/tracker_state.rs` and
//! `tests/tracker_leases.rs`, which assert them through `change_state` and the
//! lease verbs; nothing here repeats them.
//!
//! Scope is asserted throughout by giving each helper an id from a second
//! project and requiring the documented refusal, never a leaked row.
//!
//! Needs a container engine; see `tests/common/db.rs`.

mod common;

use std::str::FromStr;
use std::time::Duration;

use axum::http::StatusCode;
use mars_orchestrator::events::TaskActor;
use mars_orchestrator::models::{
    NewSession, NewTaskComment, NewTaskHandoff, Priority, ProfileKind, ReviewStatus, Task,
    TaskDependencyKind, TaskRef, TaskStateKind,
};
use mars_orchestrator::prelude::*;
use mars_orchestrator::repositories::{SessionRepository, TaskFilter, TaskRepository};
use mars_orchestrator::tracker::{
    CreateTaskInput, CreatedBy, TaskDto, TrackerMutation, UpdateTaskInput, create_task,
    delete_task, update_task,
};
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
    let mut mutation = TrackerMutation::begin(pool, project_id, TaskActor::System)
        .await
        .expect("the mutation opens");
    repository
        .insert_default_states(mutation.conn(), project_id)
        .await
        .expect("the default states insert");
    mutation.commit().await.expect("the mutation commits");

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

/// Run `body` against an open mutation on `project_id`, committing it when the
/// verb succeeded and rolling it back when it did not.
///
/// The pairing every transport makes (`tracker::commit_and_notify`), without
/// the email side.
async fn in_mutation<T, F>(pool: &PgPool, project_id: Uuid, actor: TaskActor, body: F) -> Result<T>
where
    F: AsyncFnOnce(&mut TrackerMutation<'_>) -> Result<T>,
{
    let mut mutation = TrackerMutation::begin(pool, project_id, actor).await?;
    match body(&mut mutation).await {
        Ok(outcome) => {
            mutation.commit().await?;
            Ok(outcome)
        }
        Err(error) => {
            mutation.no_change().await?;
            Err(error)
        }
    }
}

/// Create a task in its own committed mutation, as `user_id` would.
async fn create(
    pool: &PgPool,
    project_id: Uuid,
    user_id: Uuid,
    input: CreateTaskInput,
) -> Result<TaskDto> {
    in_mutation(pool, project_id, TaskActor::User { user_id }, async |m| {
        create_task(m, input).await
    })
    .await
}

/// The input a plain titled creation is.
fn new_task(title: &str, user_id: Uuid) -> CreateTaskInput {
    CreateTaskInput {
        title: title.to_string(),
        description: None,
        state: None,
        priority: None,
        labels: Vec::new(),
        parent: None,
        depends_on: Vec::new(),
        discovered_from: None,
        created_by: CreatedBy::User(user_id),
    }
}

/// A titled task on this project, created and committed.
async fn create_titled(pool: &PgPool, project_id: Uuid, user_id: Uuid, title: &str) -> TaskDto {
    create(pool, project_id, user_id, new_task(title, user_id))
        .await
        .expect("the task is created")
}

/// Apply an update to a task in its own committed mutation, reading the row
/// under the lock the way every transport does.
async fn update(
    pool: &PgPool,
    project_id: Uuid,
    actor: TaskActor,
    task_id: Uuid,
    input: UpdateTaskInput,
) -> Result<(TaskDto, bool)> {
    in_mutation(pool, project_id, actor, async |m| {
        let task = locked(m, project_id, task_id).await?;
        let outcome = update_task(m, &task, input).await?;
        Ok((outcome.task, outcome.changed))
    })
    .await
}

/// Delete a task in its own committed mutation.
async fn delete(pool: &PgPool, project_id: Uuid, task_id: Uuid) -> Result<()> {
    in_mutation(pool, project_id, TaskActor::System, async |m| {
        let task = locked(m, project_id, task_id).await?;
        delete_task(m, &task).await
    })
    .await
}

/// The task row as it stands under this mutation's lock.
async fn locked(m: &mut TrackerMutation<'_>, project_id: Uuid, task_id: Uuid) -> Result<Task> {
    TaskRepository::new(m.pool())
        .find_task_for_update(m.conn(), project_id, TaskRef::Id(task_id))
        .await?
        .ok_or(Error::NotFound)
}

#[tokio::test]
async fn numbers_run_from_one_and_are_never_reused() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let user_id = fixture.user_id;
    let repository = TaskRepository::new(&pool);

    let first = create_titled(&pool, project_id, user_id, "first").await;
    let second = create_titled(&pool, project_id, user_id, "second").await;
    let third = create_titled(&pool, project_id, user_id, "third").await;
    assert_eq!([first.number, second.number, third.number], [1, 2, 3]);

    // The counter is the project's, not a count of its rows: deleting the
    // middle task leaves the next number at 4 (`SPEC.md`, "Tasks": "`number`
    // is assigned from the project's counter and never reused").
    delete(&pool, project_id, second.id).await.unwrap();

    let fourth = create_titled(&pool, project_id, user_id, "fourth").await;
    assert_eq!(fourth.number, 4);

    // A second project numbers from 1 of its own.
    let other_project = seed_project(&pool).await;
    assert_eq!(
        create_titled(&pool, other_project, user_id, "theirs")
            .await
            .number,
        1
    );

    // Deleting something that is not this project's task is not found, and
    // certainly not a deletion.
    let error = delete(&pool, project_id, Uuid::new_v4())
        .await
        .expect_err("there is no such task");
    assert_eq!(error.status(), StatusCode::NOT_FOUND);
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
    let user_id = fixture.user_id;
    let repository = TaskRepository::new(&pool);

    // No state given: the queue state with the lowest position.
    let defaulted = create_titled(&pool, project_id, user_id, "defaulted").await;
    assert_eq!(defaulted.state, "backlog");
    assert_eq!(defaulted.priority, Priority::MEDIUM.get());
    assert!(!defaulted.blocked);
    assert!(defaulted.labels.is_empty());
    assert!(defaulted.closed_at.is_none());
    assert!(defaulted.lease_holder_session_id.is_none());

    let row = repository
        .find_task(project_id, TaskRef::Id(defaulted.id))
        .await
        .unwrap()
        .expect("the row is there");
    assert_eq!(row.state_id, state_id(&pool, project_id, "backlog").await);
    assert_eq!(row.attempts, 0);

    // An explicit state of this project is taken as given, with everything
    // else the input carries.
    let explicit = create(
        &pool,
        project_id,
        user_id,
        CreateTaskInput {
            state: Some("review".into()),
            priority: Some(Priority::CRITICAL.get()),
            description: Some("# plan\n\nwrite it".into()),
            labels: vec!["backend".into(), "p0".into()],
            ..new_task("explicit", user_id)
        },
    )
    .await
    .unwrap();
    assert_eq!(explicit.state, "review");
    assert_eq!(explicit.priority, 0);
    assert_eq!(explicit.labels, ["backend", "p0"]);
    assert_eq!(
        repository
            .find_task(project_id, TaskRef::Id(explicit.id))
            .await
            .unwrap()
            .unwrap()
            .created_by_user_id,
        Some(user_id),
    );

    // A state this project does not have is not a state this task can be in,
    // and the refusal names the ones it does.
    let error = create(
        &pool,
        project_id,
        user_id,
        CreateTaskInput {
            state: Some("nowhere".into()),
            ..new_task("foreign state", user_id)
        },
    )
    .await
    .expect_err("the project has no such state");
    assert_eq!(error.status(), StatusCode::BAD_REQUEST);
    assert!(
        error.to_string().contains("backlog"),
        "the refusal did not list the project's states: {error}",
    );
}

#[tokio::test]
async fn a_project_with_no_queue_state_cannot_take_a_task() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let user_id = fixture.user_id;

    // `delete_state` refuses to remove the last queue state, so this is the
    // defensive path: the rows are taken out from under the repository.
    sqlx::query("DELETE FROM task_states WHERE project_id = $1 AND kind = 'queue'")
        .bind(project_id)
        .execute(&pool)
        .await
        .expect("the queue states delete");

    let error = create(
        &pool,
        project_id,
        user_id,
        new_task("nowhere to go", user_id),
    )
    .await
    .expect_err("there is no state to land in");
    assert_eq!(error.status(), StatusCode::CONFLICT);
    assert_eq!(error.to_string(), "project has no queue state");

    // Nothing was allocated on the way out: the next successful creation on a
    // repaired project still gets number 1.
    let repaired = create(
        &pool,
        project_id,
        user_id,
        CreateTaskInput {
            state: Some("needs_human".into()),
            ..new_task("somewhere to go", user_id)
        },
    )
    .await
    .unwrap();
    assert_eq!(repaired.number, 1);
}

#[tokio::test]
async fn nesting_is_one_level_deep_on_creation_and_on_re_parenting() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let user_id = fixture.user_id;
    let actor = TaskActor::User { user_id };

    let epic = create_titled(&pool, project_id, user_id, "the epic").await;

    // The allowed case: a top-level task of this project may be a parent.
    let child = create(
        &pool,
        project_id,
        user_id,
        CreateTaskInput {
            parent: Some(epic.id),
            ..new_task("a child", user_id)
        },
    )
    .await
    .unwrap();
    assert_eq!(child.parent_id, Some(epic.id));

    // A parent of another project is not a parent at all.
    let other_project = seed_project(&pool).await;
    let theirs = create_titled(&pool, other_project, user_id, "their epic").await;
    let error = create(
        &pool,
        project_id,
        user_id,
        CreateTaskInput {
            parent: Some(theirs.id),
            ..new_task("cross-project child", user_id)
        },
    )
    .await
    .expect_err("the parent belongs to another project");
    assert_eq!(error.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        error.to_string(),
        "parent must be a top-level task of the same project"
    );

    // A task that already has a parent cannot receive children.
    let error = create(
        &pool,
        project_id,
        user_id,
        CreateTaskInput {
            parent: Some(child.id),
            ..new_task("a grandchild", user_id)
        },
    )
    .await
    .expect_err("the parent already has one");
    assert_eq!(error.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        error.to_string(),
        "parent must be a top-level task of the same project"
    );

    // Re-parenting enforces the same rule from the other end.
    let loose = create_titled(&pool, project_id, user_id, "loose").await;
    let error = update(
        &pool,
        project_id,
        actor,
        loose.id,
        UpdateTaskInput {
            parent: Some(Some(TaskRef::Id(child.id))),
            ..UpdateTaskInput::default()
        },
    )
    .await
    .expect_err("a task with a parent cannot receive children");
    assert_eq!(error.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        error.to_string(),
        "a task with a parent cannot receive children"
    );

    // And an epic cannot become somebody's child.
    let other_epic = create_titled(&pool, project_id, user_id, "another epic").await;
    let error = update(
        &pool,
        project_id,
        actor,
        epic.id,
        UpdateTaskInput {
            parent: Some(Some(TaskRef::Id(other_epic.id))),
            ..UpdateTaskInput::default()
        },
    )
    .await
    .expect_err("a task with children cannot get a parent");
    assert_eq!(error.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        error.to_string(),
        "a task with children cannot get a parent"
    );

    // A task is never its own parent.
    let error = update(
        &pool,
        project_id,
        actor,
        loose.id,
        UpdateTaskInput {
            parent: Some(Some(TaskRef::Id(loose.id))),
            ..UpdateTaskInput::default()
        },
    )
    .await
    .expect_err("a task cannot be its own parent");
    assert_eq!(error.status(), StatusCode::BAD_REQUEST);
    assert_eq!(error.to_string(), "a task cannot be its own parent");

    // A reference naming no task of this project is the same refusal as a
    // parent that is not top-level: it is not a parent this project has.
    let error = update(
        &pool,
        project_id,
        actor,
        loose.id,
        UpdateTaskInput {
            parent: Some(Some(TaskRef::Id(theirs.id))),
            ..UpdateTaskInput::default()
        },
    )
    .await
    .expect_err("the parent belongs to another project");
    assert_eq!(error.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        error.to_string(),
        "parent must be a top-level task of the same project"
    );

    // Moving a child out to the top level is always allowed.
    let (detached, changed) = update(
        &pool,
        project_id,
        actor,
        child.id,
        UpdateTaskInput {
            parent: Some(None),
            ..UpdateTaskInput::default()
        },
    )
    .await
    .unwrap();
    assert!(changed, "detaching is a change");
    assert!(detached.parent_id.is_none());

    assert!(
        TaskRepository::new(&pool)
            .list_children(project_id, epic.id)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn an_update_that_changes_nothing_writes_nothing() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let user_id = fixture.user_id;
    let actor = TaskActor::User { user_id };
    let repository = TaskRepository::new(&pool);

    let task = create(
        &pool,
        project_id,
        user_id,
        CreateTaskInput {
            labels: vec!["backend".into()],
            ..new_task("as stored", user_id)
        },
    )
    .await
    .unwrap();
    let stored_at = repository
        .find_task(project_id, TaskRef::Id(task.id))
        .await
        .unwrap()
        .unwrap()
        .updated_at;

    // Every supplied value already equals the stored one, so the row is not
    // written and `updated_at` does not move (ADR 0030).
    let (_, changed) = update(
        &pool,
        project_id,
        actor,
        task.id,
        UpdateTaskInput {
            title: Some("as stored".into()),
            description: Some(String::new()),
            priority: Some(Priority::MEDIUM.get()),
            labels: Some(vec!["backend".into()]),
            assignee_user_id: Some(None),
            parent: Some(None),
            state: Some("backlog".into()),
            ..UpdateTaskInput::default()
        },
    )
    .await
    .unwrap();
    assert!(!changed, "an identical update is not a change");
    assert_eq!(
        repository
            .find_task(project_id, TaskRef::Id(task.id))
            .await
            .unwrap()
            .unwrap()
            .updated_at,
        stored_at,
    );

    // An empty update is the same answer by a shorter route.
    let (_, changed) = update(
        &pool,
        project_id,
        actor,
        task.id,
        UpdateTaskInput::default(),
    )
    .await
    .unwrap();
    assert!(!changed);

    // One differing field is enough to make it a change, and only the fields
    // that were mentioned move.
    let (updated, changed) = update(
        &pool,
        project_id,
        actor,
        task.id,
        UpdateTaskInput {
            priority: Some(Priority::HIGH.get()),
            assignee_user_id: Some(Some(user_id)),
            ..UpdateTaskInput::default()
        },
    )
    .await
    .unwrap();
    assert!(changed, "a differing field is a change");
    assert_eq!(updated.priority, Priority::HIGH.get());
    assert_eq!(updated.assignee_user_id, Some(user_id));
    assert_eq!(updated.title, "as stored");
    assert_eq!(updated.labels, ["backend"]);
    assert!(
        repository
            .find_task(project_id, TaskRef::Id(task.id))
            .await
            .unwrap()
            .unwrap()
            .updated_at
            > stored_at
    );

    // Clearing an optional column is a change; asking for it twice is not.
    let (cleared, changed) = update(
        &pool,
        project_id,
        actor,
        task.id,
        UpdateTaskInput {
            assignee_user_id: Some(None),
            labels: Some(Vec::new()),
            ..UpdateTaskInput::default()
        },
    )
    .await
    .unwrap();
    assert!(changed, "clearing is a change");
    assert!(cleared.assignee_user_id.is_none());
    assert!(cleared.labels.is_empty());

    // An unknown task is 404, whatever the update says.
    let error = update(
        &pool,
        project_id,
        actor,
        Uuid::new_v4(),
        UpdateTaskInput::default(),
    )
    .await
    .expect_err("there is no such task");
    assert_eq!(error.status(), StatusCode::NOT_FOUND);

    // So is a task of another project, which is what keeps the scope honest.
    let other_project = seed_project(&pool).await;
    let theirs = create_titled(&pool, other_project, user_id, "theirs").await;
    let error = update(
        &pool,
        project_id,
        actor,
        theirs.id,
        UpdateTaskInput {
            priority: Some(Priority::LOW.get()),
            ..UpdateTaskInput::default()
        },
    )
    .await
    .expect_err("the task belongs to another project");
    assert_eq!(error.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_task_is_found_by_its_uuid_or_its_number_and_only_in_its_project() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let user_id = fixture.user_id;
    let repository = TaskRepository::new(&pool);

    let task = create_titled(&pool, project_id, user_id, "addressable").await;

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
    let theirs = create_titled(&pool, other_project, user_id, "theirs").await;
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
    let mut mutation = TrackerMutation::begin(&pool, project_id, TaskActor::System)
        .await
        .unwrap();
    assert_eq!(
        repository
            .find_task_for_update(mutation.conn(), project_id, TaskRef::Number(task.number))
            .await
            .unwrap()
            .unwrap()
            .id,
        task.id,
    );
    assert!(
        repository
            .find_task_for_update(mutation.conn(), project_id, TaskRef::Id(theirs.id))
            .await
            .unwrap()
            .is_none()
    );
    mutation.no_change().await.unwrap();
}

#[tokio::test]
async fn the_board_reads_filter_and_order_as_documented() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let user_id = fixture.user_id;
    let actor = TaskActor::User { user_id };
    let repository = TaskRepository::new(&pool);
    let session_id = seed_session(&pool, project_id, fixture.profile_id).await;

    let review = state_id(&pool, project_id, "review").await;

    // Created in an order that is not the order they come back in.
    let low = create(
        &pool,
        project_id,
        user_id,
        CreateTaskInput {
            priority: Some(Priority::LOW.get()),
            labels: vec!["backend".into()],
            ..new_task("low", user_id)
        },
    )
    .await
    .unwrap();

    let critical = create(
        &pool,
        project_id,
        user_id,
        CreateTaskInput {
            priority: Some(Priority::CRITICAL.get()),
            state: Some("review".into()),
            ..new_task("critical", user_id)
        },
    )
    .await
    .unwrap();

    let also_low = create(
        &pool,
        project_id,
        user_id,
        CreateTaskInput {
            priority: Some(Priority::LOW.get()),
            labels: vec!["backend".into(), "infra".into()],
            ..new_task("also low", user_id)
        },
    )
    .await
    .unwrap();

    let escalated = create(
        &pool,
        project_id,
        user_id,
        CreateTaskInput {
            state: Some("needs_human".into()),
            ..new_task("escalated", user_id)
        },
    )
    .await
    .unwrap();

    let ids = |tasks: Vec<Task>| tasks.into_iter().map(|task| task.id).collect::<Vec<_>>();

    // Priority first, then number: `low` was created before `also low`.
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

    // `held` splits the board by whether anybody has a lease. The lease is the
    // repository's `claim` statement, which is what the lease verbs compose
    // (`tests/tracker_leases.rs` asserts their rules).
    in_mutation(&pool, project_id, TaskActor::System, async |m| {
        TaskRepository::new(m.pool())
            .claim(m.conn(), project_id, critical.id, session_id, &[review])
            .await?
            .ok_or_else(|| Error::Internal("the claim found nothing to take".into()))
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
    assert_eq!(
        repository
            .list_by_lease_holder(session_id)
            .await
            .unwrap()
            .into_iter()
            .map(|held| held.id)
            .collect::<Vec<_>>(),
        [critical.id],
    );

    // `parent` filters to an epic's children, which is also what
    // `list_children` answers.
    update(
        &pool,
        project_id,
        actor,
        also_low.id,
        UpdateTaskInput {
            parent: Some(Some(TaskRef::Id(low.id))),
            ..UpdateTaskInput::default()
        },
    )
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
    let theirs = create(
        &pool,
        other_project,
        user_id,
        CreateTaskInput {
            state: Some("needs_human".into()),
            priority: Some(Priority::CRITICAL.get()),
            ..new_task("theirs, escalated", user_id)
        },
    )
    .await
    .unwrap();

    let waiting_ids = ids(repository
        .list_tasks_by_state_kind(TaskStateKind::Human)
        .await
        .unwrap());
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
    let user_id = fixture.user_id;
    let repository = TaskRepository::new(&pool);

    let blocked = create_titled(&pool, project_id, user_id, "waits").await;
    let blocker = create_titled(&pool, project_id, user_id, "must finish first").await;

    // The edge inserts are the repository's: an edge has no verb of its own
    // that can name its kind, and `kind` being part of the primary key is the
    // rule under test here.
    let insert = async |task_id: Uuid, depends_on: Uuid, kind: TaskDependencyKind| {
        in_mutation(&pool, project_id, TaskActor::System, async |m| {
            TaskRepository::new(m.pool())
                .insert_dependency(m.conn(), project_id, task_id, depends_on, kind)
                .await
        })
        .await
    };

    let edge = insert(blocked.id, blocker.id, TaskDependencyKind::Blocks)
        .await
        .unwrap();
    assert_eq!(edge.kind, TaskDependencyKind::Blocks);

    // The same pair with a different kind is a different edge.
    insert(blocked.id, blocker.id, TaskDependencyKind::DiscoveredFrom)
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
    let error = insert(blocked.id, blocker.id, TaskDependencyKind::Blocks)
        .await
        .expect_err("the edge is already there");
    assert_eq!(error.status(), StatusCode::CONFLICT);
    assert_eq!(error.to_string(), "dependency already exists");

    // A task cannot depend on itself.
    let error = insert(blocked.id, blocked.id, TaskDependencyKind::Blocks)
        .await
        .expect_err("a task cannot depend on itself");
    assert_eq!(error.status(), StatusCode::BAD_REQUEST);
    assert_eq!(error.to_string(), "a task cannot depend on itself");

    // Neither end may be another project's task, in either position.
    let other_project = seed_project(&pool).await;
    let theirs = create_titled(&pool, other_project, user_id, "theirs").await;
    for (task_id, depends_on) in [(blocked.id, theirs.id), (theirs.id, blocker.id)] {
        let error = insert(task_id, depends_on, TaskDependencyKind::Related)
            .await
            .expect_err("one end belongs to another project");
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
    let in_tx = in_mutation(&pool, project_id, TaskActor::System, async |m| {
        TaskRepository::new(m.pool())
            .list_dependants_in_tx(m.conn(), project_id, blocker.id, TaskDependencyKind::Blocks)
            .await
    })
    .await
    .unwrap();
    assert_eq!(in_tx, [blocked.id]);

    // Removing one kind leaves the other standing.
    let remove = async |project: Uuid, task_id: Uuid, depends_on: Uuid| {
        in_mutation(&pool, project, TaskActor::System, async |m| {
            TaskRepository::new(m.pool())
                .delete_dependency(
                    m.conn(),
                    project,
                    task_id,
                    depends_on,
                    TaskDependencyKind::Blocks,
                )
                .await
        })
        .await
    };

    assert!(remove(project_id, blocked.id, blocker.id).await.unwrap());

    let edges = repository
        .list_dependencies(project_id, blocked.id)
        .await
        .unwrap();
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].kind, TaskDependencyKind::DiscoveredFrom);

    // Removing it again, or removing another project's edge, is `false`.
    for project in [project_id, other_project] {
        assert!(!remove(project, blocked.id, blocker.id).await.unwrap());
    }

    // Deleting a prerequisite takes its edges with it.
    delete(&pool, project_id, blocker.id).await.unwrap();
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
    let user_id = fixture.user_id;
    let repository = TaskRepository::new(&pool);
    let session_id = seed_session(&pool, project_id, fixture.profile_id).await;

    let task = create_titled(&pool, project_id, user_id, "discussed").await;

    // The author rules belong to the insert: `tracker::add_comment` cannot
    // express a comment with two authors or none, which is what makes these
    // the repository's to refuse.
    let insert = async |comment: &NewTaskComment| {
        in_mutation(&pool, project_id, TaskActor::System, async |m| {
            TaskRepository::new(m.pool())
                .insert_comment(m.conn(), project_id, comment)
                .await
        })
        .await
    };

    let from_user = NewTaskComment::from_user(task.id, user_id, "looks right to me");
    let from_session = NewTaskComment::from_session(task.id, session_id, "pushed a fix");
    let from_system = NewTaskComment::from_system(task.id, "released: the session ended");

    for comment in [&from_user, &from_session, &from_system] {
        insert(comment).await.unwrap();
    }

    let comments = repository.list_comments(project_id, task.id).await.unwrap();
    assert_eq!(
        comments.iter().map(|c| c.id).collect::<Vec<_>>(),
        [from_user.id, from_session.id, from_system.id],
    );
    assert_eq!(comments[0].author_user_id, Some(user_id));
    assert_eq!(comments[1].author_session_id, Some(session_id));
    assert!(comments[2].system);
    assert!(comments[2].author_user_id.is_none());
    assert!(comments[2].author_session_id.is_none());

    // Two authors, no author, and an author on a system comment.
    let mut two_authors = NewTaskComment::from_user(task.id, user_id, "who wrote this?");
    two_authors.author_session_id = Some(session_id);

    let mut no_author = NewTaskComment::from_user(task.id, user_id, "nobody wrote this");
    no_author.author_user_id = None;

    let mut authored_system = NewTaskComment::from_system(task.id, "escalated");
    authored_system.author_user_id = Some(user_id);

    for comment in [&two_authors, &no_author, &authored_system] {
        let error = insert(comment).await.expect_err("the authorship is wrong");
        assert_eq!(error.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            error.to_string(),
            "a comment has exactly one author, and a system comment has none"
        );
    }

    // An empty body is rejected before any row is written.
    let empty = NewTaskComment::from_user(task.id, user_id, "   \n ");
    let error = insert(&empty).await.expect_err("the body is empty");
    assert_eq!(error.status(), StatusCode::BAD_REQUEST);
    assert_eq!(error.to_string(), "comment body must not be empty");

    // A task outside this project is 404, whichever way round it is asked.
    let other_project = seed_project(&pool).await;
    let theirs = create_titled(&pool, other_project, user_id, "theirs").await;
    let crossing = NewTaskComment::from_user(theirs.id, user_id, "not mine to comment on");
    let error = insert(&crossing)
        .await
        .expect_err("the task belongs to another project");
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

#[tokio::test]
async fn a_handoff_is_validated_against_its_task_comment_and_sessions() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let user_id = fixture.user_id;
    let repository = TaskRepository::new(&pool);
    let session_id = seed_session(&pool, project_id, fixture.profile_id).await;

    let task = create_titled(&pool, project_id, user_id, "handed off").await;

    // A record and its comment commit together, as they do in the real
    // publication path (`tracker::handoffs::publish_in_transaction`, whose
    // git-backed scenarios are `tests/tracker_handoffs.rs` and
    // `tests/handoffs_service.rs`). What is under test here is the insert's
    // own cross-table validation, which that path can only ever satisfy.
    let publish = async |comment: &NewTaskComment, handoff: &NewTaskHandoff| {
        in_mutation(&pool, project_id, TaskActor::System, async |m| {
            let repository = TaskRepository::new(m.pool());
            repository
                .insert_comment(m.conn(), project_id, comment)
                .await?;
            repository
                .insert_handoff(m.conn(), project_id, handoff)
                .await
        })
        .await
    };

    let comment = NewTaskComment::from_session(task.id, session_id, "implemented and pushed");
    let mut handoff =
        NewTaskHandoff::new(task.id, format!("session/{session_id}"), COMMIT, comment.id);
    handoff.source_session_id = Some(session_id);
    handoff.created_by_session_id = Some(session_id);
    let published = publish(&comment, &handoff).await.unwrap().id;

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
    let review_comment = NewTaskComment::from_user(task.id, user_id, "approved");
    let mut forwarded = NewTaskHandoff::new(
        task.id,
        stored.source_branch.clone(),
        stored.commit.clone(),
        review_comment.id,
    );
    forwarded.source_session_id = stored.source_session_id;
    forwarded.created_by_user_id = Some(user_id);
    forwarded.review_status = ReviewStatus::Approved;
    forwarded.reviewed_by_user_id = Some(user_id);
    forwarded.reviewed_at = Some(chrono::Utc::now());
    let forwarded_id = publish(&review_comment, &forwarded).await.unwrap().id;

    let stored_forward = repository
        .find_handoff(project_id, forwarded_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored_forward.review_status, ReviewStatus::Approved);
    assert_eq!(stored_forward.reviewed_by_user_id, Some(user_id));
    assert!(stored_forward.reviewed_at.is_some());

    // Oldest first.
    let history = repository.list_handoffs(project_id, task.id).await.unwrap();
    assert_eq!(
        history.iter().map(|h| h.id).collect::<Vec<_>>(),
        [published, forwarded_id],
    );

    // The model's own rules still run: an abbreviated commit is not a commit.
    let comment = NewTaskComment::from_user(task.id, user_id, "half a commit");
    let mut abbreviated = NewTaskHandoff::new(task.id, "session/x", &COMMIT[..7], comment.id);
    abbreviated.created_by_user_id = Some(user_id);
    let error = publish(&comment, &abbreviated)
        .await
        .expect_err("the commit is abbreviated");
    assert_eq!(error.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        error.to_string(),
        "commit must be a full lowercase hexadecimal git object id"
    );

    // A comment of another task is not this hand-off's comment.
    let other_task = create_titled(&pool, project_id, user_id, "somewhere else").await;
    let elsewhere = NewTaskComment::from_user(other_task.id, user_id, "about the other task");
    let mut wrong_comment = NewTaskHandoff::new(task.id, "session/x", COMMIT, elsewhere.id);
    wrong_comment.created_by_user_id = Some(user_id);
    let error = publish(&elsewhere, &wrong_comment)
        .await
        .expect_err("the comment is on another task");
    assert_eq!(error.status(), StatusCode::BAD_REQUEST);
    assert_eq!(error.to_string(), "comment must belong to this task");

    // A session of another project cannot be the source of this project's
    // work.
    let other_project = seed_project(&pool).await;
    let their_profile = seed_profile(&pool, other_project).await;
    let their_session = seed_session(&pool, other_project, their_profile).await;

    let comment = NewTaskComment::from_user(task.id, user_id, "whose branch is this?");
    let mut foreign_session = NewTaskHandoff::new(task.id, "session/x", COMMIT, comment.id);
    foreign_session.source_session_id = Some(their_session);
    foreign_session.created_by_user_id = Some(user_id);
    let error = publish(&comment, &foreign_session)
        .await
        .expect_err("the source session belongs to another project");
    assert_eq!(error.status(), StatusCode::BAD_REQUEST);
    assert_eq!(error.to_string(), "sessions must belong to this project");

    // A task of another project is 404, and the record is not visible from
    // one either.
    let theirs = create_titled(&pool, other_project, user_id, "theirs").await;
    let comment = NewTaskComment::from_user(theirs.id, user_id, "not mine");
    let mut crossing = NewTaskHandoff::new(theirs.id, "session/x", COMMIT, comment.id);
    crossing.created_by_user_id = Some(user_id);
    let error = in_mutation(&pool, project_id, TaskActor::System, async |m| {
        TaskRepository::new(m.pool())
            .insert_handoff(m.conn(), project_id, &crossing)
            .await
    })
    .await
    .expect_err("the task belongs to another project");
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
    let user_id = fixture.user_id;
    let repository = TaskRepository::new(&pool);
    let session_id = seed_session(&pool, project_id, fixture.profile_id).await;
    let other_session = seed_session(&pool, project_id, fixture.profile_id).await;

    let task = create_titled(&pool, project_id, user_id, "worked on").await;
    let other_task = create_titled(&pool, project_id, user_id, "also worked on").await;

    // A session's change writes the link on commit, once per pair however
    // often the mutation records it (`TrackerMutation::touch`, ADR 0030).
    let touch = async |actor_session: Uuid, task_id: Uuid, title: &str| {
        update(
            &pool,
            project_id,
            TaskActor::Session {
                session_id: actor_session,
            },
            task_id,
            UpdateTaskInput {
                title: Some(title.to_string()),
                ..UpdateTaskInput::default()
            },
        )
        .await
        .expect("the update applies");
    };

    touch(session_id, task.id, "worked on once").await;
    let first = repository
        .list_task_sessions(task.id)
        .await
        .unwrap()
        .into_iter()
        .next()
        .expect("the link is there");
    assert_eq!(first.session_id, session_id);
    assert_eq!(first.first_touched_at, first.last_touched_at);

    sleep(BETWEEN_TOUCHES).await;

    // A later transaction moves `last_touched_at` and leaves the first alone.
    touch(session_id, task.id, "worked on twice").await;
    let later = repository
        .list_task_sessions(task.id)
        .await
        .unwrap()
        .into_iter()
        .next()
        .expect("the link is still there");
    assert_eq!(later.first_touched_at, first.first_touched_at);
    assert!(later.last_touched_at > first.last_touched_at);

    // A second session on the same task is its own link.
    touch(other_session, task.id, "and by somebody else").await;

    let links = repository.list_task_sessions(task.id).await.unwrap();
    assert_eq!(
        links.iter().map(|link| link.session_id).collect::<Vec<_>>(),
        [session_id, other_session],
    );
    assert_eq!(links[0].first_touched_at, first.first_touched_at);

    sleep(BETWEEN_TOUCHES).await;

    // The other direction: what did this session work on, most recent first.
    touch(session_id, other_task.id, "also worked on, later").await;

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

    // A no-op change records nothing: the link is history of changes made
    // (ADR 0030).
    let (_, changed) = update(
        &pool,
        project_id,
        TaskActor::Session {
            session_id: other_session,
        },
        other_task.id,
        UpdateTaskInput {
            title: Some("also worked on, later".into()),
            ..UpdateTaskInput::default()
        },
    )
    .await
    .unwrap();
    assert!(!changed);
    assert_eq!(
        repository
            .list_task_sessions(other_task.id)
            .await
            .unwrap()
            .len(),
        1,
    );

    // Deleting the task takes its links with it.
    delete(&pool, project_id, task.id).await.unwrap();
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
