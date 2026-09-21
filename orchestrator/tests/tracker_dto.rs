//! `TaskRepository`'s read-only DTO loaders against a real Postgres
//! (`CLAUDE.md`, "Testing expectations").
//!
//! These assemble `SPEC.md`'s `Task` and `TaskDetail` out of five tables, and
//! what is asserted here is everything the `tasks` row alone cannot answer:
//!
//! - `state` is the state's **name**, not its id;
//! - `depends_on` carries every outgoing edge *with its kind*, so a pair
//!   holding both a `blocks` and a `discovered_from` edge appears twice;
//! - `blocks` is the reverse direction and only the `blocks` kind;
//! - `handoff` is the record `current_handoff_id` names, and `null` when it
//!   names nothing;
//! - `TaskDetail` orders comments oldest first, hand-offs oldest first,
//!   children by priority then number, and session links by first touch;
//! - a per-project number addresses a task exactly like its UUID, and an
//!   unknown one is `None` rather than an error;
//! - the batch loader is a fixed number of queries, so a board of fifty tasks
//!   costs the same four as one task;
//! - the `_in` variant sees the uncommitted state, which is the whole reason
//!   event payloads can be built inside a mutation.
//!
//! Scope is asserted by asking a second project for the first project's task.
//!
//! Needs a container engine; see `tests/common/db.rs`.

mod common;

use std::time::Duration;

use mars_orchestrator::events::TaskActor;
use mars_orchestrator::models::{
    HandoffCaller, NewSession, NewTask, NewTaskComment, NewTaskHandoff, Priority, ProfileKind,
    ReviewStatus, Task, TaskDependencyKind, TaskRef, TaskUpdate,
};
use mars_orchestrator::prelude::*;
use mars_orchestrator::repositories::{SessionRepository, TaskFilter, TaskRepository};
use mars_orchestrator::tracker::state::{StateChangeOptions, change_state};
use mars_orchestrator::tracker::{
    CommentAuthor, Locked, ReviewCarry, TrackerMutation, add_comment,
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
/// two mutations, short enough not to drag the suite out.
const BETWEEN_WRITES: Duration = Duration::from_millis(50);

/// One project with its default states, a profile and the user everything is
/// attributed to.
struct Fixture {
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

/// A titled task on this project, inserted and committed.
async fn insert_titled(pool: &PgPool, project_id: Uuid, title: &str) -> Task {
    let task = NewTask::new(project_id, title).expect("the title parses");
    let repository = TaskRepository::new(pool);
    let mut mutation = TrackerMutation::begin(pool, project_id, TaskActor::System)
        .await
        .expect("the mutation opens");
    let inserted = repository
        .insert_task(mutation.conn(), project_id, &task)
        .await
        .expect("the task inserts");
    mutation.commit().await.expect("the mutation commits");

    inserted
}

/// Run `body` inside a committed tracker mutation.
async fn in_mutation<T, F>(pool: &PgPool, project_id: Uuid, body: F) -> T
where
    F: AsyncFnOnce(&TaskRepository<'_>, Locked<'_>) -> T,
{
    let repository = TaskRepository::new(pool);
    let mut mutation = TrackerMutation::begin(pool, project_id, TaskActor::System)
        .await
        .expect("the mutation opens");
    let outcome = body(&repository, mutation.conn()).await;
    mutation.commit().await.expect("the mutation commits");

    outcome
}

#[tokio::test]
async fn a_task_carries_its_state_name_its_edges_and_no_handoff() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let repository = TaskRepository::new(&pool);

    let subject = insert_titled(&pool, project_id, "subject").await;
    let prerequisite = insert_titled(&pool, project_id, "prerequisite").await;
    let dependant = insert_titled(&pool, project_id, "dependant").await;

    // The same pair carries both kinds, which is the case `SPEC.md` calls out
    // and the one a naive `depends_on` collapses.
    in_mutation(&pool, project_id, async |repository, mut tx| {
        for kind in [
            TaskDependencyKind::Blocks,
            TaskDependencyKind::DiscoveredFrom,
        ] {
            repository
                .insert_dependency(tx.reborrow(), project_id, subject.id, prerequisite.id, kind)
                .await
                .expect("the edge inserts");
        }
        repository
            .insert_dependency(
                tx,
                project_id,
                dependant.id,
                subject.id,
                TaskDependencyKind::Blocks,
            )
            .await
            .expect("the reverse edge inserts");
    })
    .await;

    let dto = repository
        .load_task_dto(project_id, subject.id)
        .await
        .expect("the task loads")
        .expect("the task is there");

    assert_eq!(dto.id, subject.id);
    assert_eq!(dto.state, "backlog");
    assert_eq!(dto.priority, 2);
    assert!(dto.handoff.is_none());
    assert_eq!(
        dto.depends_on
            .iter()
            .map(|edge| (edge.task_id, edge.kind))
            .collect::<Vec<_>>(),
        vec![
            (prerequisite.id, TaskDependencyKind::Blocks),
            (prerequisite.id, TaskDependencyKind::DiscoveredFrom),
        ]
    );
    assert_eq!(dto.blocks, vec![dependant.id]);

    // The informational kind never appears in the reverse direction.
    let prerequisite_dto = repository
        .load_task_dto(project_id, prerequisite.id)
        .await
        .expect("the task loads")
        .expect("the task is there");
    assert_eq!(prerequisite_dto.blocks, vec![subject.id]);
    assert!(prerequisite_dto.depends_on.is_empty());
}

#[tokio::test]
async fn a_task_carries_the_handoff_its_column_names() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let session_id = seed_session(&pool, project_id, fixture.profile_id).await;
    let repository = TaskRepository::new(&pool);

    let task = insert_titled(&pool, project_id, "handed off").await;

    // A record of this task that the column does not name. `handoff` is the
    // pointer's, not the newest row's, so a task with history and no pointer
    // carries none — which is also what the detail's `handoffs` list is for.
    let historical = in_mutation(&pool, project_id, async |repository, mut tx| {
        let comment = NewTaskComment::from_session(task.id, session_id, "an earlier look");
        let comment = repository
            .insert_comment(tx.reborrow(), project_id, &comment)
            .await
            .expect("the comment inserts");

        let mut handoff = NewTaskHandoff::new(task.id, "session/earlier", COMMIT, comment.id);
        handoff.source_session_id = Some(session_id);
        handoff.created_by_session_id = Some(session_id);
        repository
            .insert_handoff(tx, project_id, &handoff)
            .await
            .expect("the hand-off inserts")
            .id
    })
    .await;

    let dto = repository
        .load_task_dto(project_id, task.id)
        .await
        .expect("the task loads")
        .expect("the task is there");
    assert!(dto.handoff.is_none(), "the column names nothing yet");

    // Now a publication, which is the one thing that writes the column
    // (`tracker::handoffs::publish_in_transaction`).
    common::tracker::hold(&pool, project_id, task.id, session_id).await;
    let (_, handoff_id) = common::tracker::handoff_in_place(
        &pool,
        project_id,
        task.id,
        common::tracker::Handoff {
            source_session_id: Some(session_id),
            source_branch: "session/branch",
            commit: COMMIT,
            comment: "have a look",
            target_state: "",
            caller: HandoffCaller::Session { session_id },
            review: ReviewCarry::Fresh,
        },
    )
    .await;

    let dto = repository
        .load_task_dto(project_id, task.id)
        .await
        .expect("the task loads")
        .expect("the task is there");

    let handoff = dto.handoff.expect("the hand-off is there");
    assert_eq!(handoff.id, handoff_id);
    assert_eq!(handoff.task_id, task.id);
    assert_eq!(handoff.commit, COMMIT);
    assert_eq!(handoff.source_branch, "session/branch");
    assert_eq!(handoff.review_status, ReviewStatus::Unreviewed);
    assert_eq!(handoff.source_session_id, Some(session_id));

    // Both records are history; only one of them is current.
    let detail = repository
        .load_task_detail(project_id, TaskRef::Id(task.id))
        .await
        .expect("the detail loads")
        .expect("the task is there");
    assert_eq!(
        detail
            .handoffs
            .iter()
            .map(|record| record.id)
            .collect::<Vec<_>>(),
        vec![historical, handoff_id],
        "oldest first",
    );
    assert_eq!(detail.task.handoff.expect("the current one").id, handoff_id);
}

#[tokio::test]
async fn a_detail_carries_its_comments_children_and_sessions_in_order() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let session_id = seed_session(&pool, project_id, fixture.profile_id).await;
    let repository = TaskRepository::new(&pool);

    let parent = insert_titled(&pool, project_id, "epic").await;
    let first_child = insert_titled(&pool, project_id, "first child").await;
    let second_child = insert_titled(&pool, project_id, "second child").await;

    // The second child is more urgent, so board order — priority, then
    // number — puts it first, which insertion order alone would not.
    in_mutation(&pool, project_id, async |repository, mut tx| {
        for (child, priority) in [
            (&first_child, Priority::default()),
            (&second_child, Priority::CRITICAL),
        ] {
            repository
                .update_task(
                    tx.reborrow(),
                    project_id,
                    child.id,
                    &TaskUpdate {
                        parent_id: Some(Some(parent.id)),
                        priority: Some(priority),
                        ..Default::default()
                    },
                )
                .await
                .expect("the child is re-parented");
        }
    })
    .await;

    // One mutation each, with a pause between: `created_at` is the
    // transaction's `NOW()`, so two comments written in one transaction share
    // a timestamp and fall back to the id tie-break, which says nothing about
    // the order the loader produces.
    //
    // Written through `tracker::add_comment` as the session, which is also
    // what puts the `task_sessions` link there: a session that comments has
    // worked on the task (ADR 0030).
    for body in ["first word", "second word"] {
        common::tracker::in_mutation(
            &pool,
            project_id,
            TaskActor::Session { session_id },
            async |m| {
                let locked = common::tracker::locked(m, parent.id).await?;
                add_comment(m, &locked, CommentAuthor::Session(session_id), body).await
            },
        )
        .await
        .expect("the comment is written");
        sleep(BETWEEN_WRITES).await;
    }

    let detail = repository
        .load_task_detail(project_id, TaskRef::Id(parent.id))
        .await
        .expect("the detail loads")
        .expect("the task is there");

    assert_eq!(detail.task.id, parent.id);
    assert_eq!(detail.task.state, "backlog");
    assert_eq!(
        detail
            .comments
            .iter()
            .map(|comment| comment.body.as_str())
            .collect::<Vec<_>>(),
        vec!["first word", "second word"]
    );
    assert!(detail.comments.iter().all(|comment| !comment.system));
    assert_eq!(
        detail
            .children
            .iter()
            .map(|child| child.id)
            .collect::<Vec<_>>(),
        vec![second_child.id, first_child.id]
    );
    assert!(detail.children.iter().all(|child| child.state == "backlog"));
    assert_eq!(detail.sessions.len(), 1);
    assert_eq!(detail.sessions[0].session_id, session_id);
    // Two comments in two mutations: the first inserted the link, the second
    // advanced `last_touched_at` and left `first_touched_at` alone.
    assert!(detail.sessions[0].first_touched_at < detail.sessions[0].last_touched_at);
    assert!(detail.handoffs.is_empty());
}

#[tokio::test]
async fn a_detail_is_addressed_by_number_as_well_as_by_uuid() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let repository = TaskRepository::new(&pool);

    let task = insert_titled(&pool, project_id, "addressable").await;

    let by_number = repository
        .load_task_detail(project_id, TaskRef::Number(task.number))
        .await
        .expect("the detail loads")
        .expect("the task is there");
    assert_eq!(by_number.task.id, task.id);

    // A number nobody carries answers like an unknown UUID, never a parse
    // error.
    assert!(
        repository
            .load_task_detail(project_id, TaskRef::Number(task.number + 1_000))
            .await
            .expect("the detail loads")
            .is_none()
    );
    assert!(
        repository
            .load_task_detail(project_id, TaskRef::Id(Uuid::new_v4()))
            .await
            .expect("the detail loads")
            .is_none()
    );
}

#[tokio::test]
async fn another_projects_task_is_not_visible() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let other_project_id = seed_project(&pool).await;
    let repository = TaskRepository::new(&pool);

    let task = insert_titled(&pool, fixture.project_id, "ours").await;

    assert!(
        repository
            .load_task_dto(other_project_id, task.id)
            .await
            .expect("the task loads")
            .is_none()
    );
    assert!(
        repository
            .load_task_detail(other_project_id, TaskRef::Id(task.id))
            .await
            .expect("the detail loads")
            .is_none()
    );
}

#[tokio::test]
async fn a_board_of_fifty_tasks_loads_in_board_order_and_a_bounded_number_of_queries() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let repository = TaskRepository::new(&pool);

    let mut inserted = Vec::new();
    for index in 0..50i16 {
        let mut task =
            NewTask::new(project_id, &format!("task {index}")).expect("the title parses");
        task.priority = Priority::try_from(index % 4).expect("a valid priority");
        let mut mutation = TrackerMutation::begin(&pool, project_id, TaskActor::System)
            .await
            .expect("the mutation opens");
        inserted.push(
            repository
                .insert_task(mutation.conn(), project_id, &task)
                .await
                .expect("the task inserts"),
        );
        mutation.commit().await.expect("the mutation commits");
    }
    assert_eq!(inserted.len(), 50);

    let tasks = repository
        .list_tasks(project_id, &TaskFilter::default())
        .await
        .expect("the board lists");
    assert_eq!(tasks.len(), 50);

    // The loader is four statements whatever the number of tasks. Counting
    // them would need `pg_stat_statements`, which the plain `postgres` image
    // does not preload, so what is asserted here is the observable half: the
    // whole board comes back, in board order, from one call — and, below,
    // that an empty batch issues nothing at all, which an N+1 written as a
    // loop over the input would also satisfy but a loop over *all* tasks
    // would not.
    let dtos = repository
        .load_task_dtos(project_id, &tasks)
        .await
        .expect("the board loads");
    assert_eq!(dtos.len(), 50);

    // The input order is kept: the row query already ordered by priority, then
    // number, and nothing in the assembly reorders it.
    assert_eq!(
        dtos.iter().map(|dto| dto.id).collect::<Vec<_>>(),
        tasks.iter().map(|task| task.id).collect::<Vec<_>>(),
    );
    let mut expected: Vec<(i16, i32)> = tasks.iter().map(|t| (t.priority, t.number)).collect();
    expected.sort_unstable();
    assert_eq!(
        dtos.iter()
            .map(|dto| (dto.priority, dto.number))
            .collect::<Vec<_>>(),
        expected,
    );
    assert!(dtos.iter().all(|dto| dto.state == "backlog"));

    // An empty batch costs nothing at all.
    assert!(
        repository
            .load_task_dtos(project_id, &[])
            .await
            .expect("the empty batch loads")
            .is_empty()
    );
}

#[tokio::test]
async fn the_dashboard_loader_spans_projects() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let other_project_id = seed_project(&pool).await;
    let repository = TaskRepository::new(&pool);

    let ours = insert_titled(&pool, fixture.project_id, "ours").await;
    let theirs = insert_titled(&pool, other_project_id, "theirs").await;

    let dtos = repository
        .load_task_dtos_any_project(&[ours.clone(), theirs.clone()])
        .await
        .expect("both tasks load");

    assert_eq!(
        dtos.iter().map(|dto| dto.id).collect::<Vec<_>>(),
        vec![ours.id, theirs.id]
    );
    assert!(dtos.iter().all(|dto| dto.state == "backlog"));
    assert_eq!(dtos[0].project_id, fixture.project_id);
    assert_eq!(dtos[1].project_id, other_project_id);

    // The scoped loader is the one that refuses: a foreign task has no state
    // in this project, so it cannot be assembled at all.
    let scoped = repository
        .load_task_dtos(fixture.project_id, &[theirs])
        .await;
    assert!(matches!(scoped, Err(Error::Internal(_))));
}

#[tokio::test]
async fn the_in_transaction_loader_sees_the_uncommitted_change() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let repository = TaskRepository::new(&pool);

    let task = insert_titled(&pool, project_id, "moving").await;
    let review = repository
        .find_state_by_name(project_id, "review")
        .await
        .expect("the state reads")
        .expect("the state exists");

    let mut mutation = TrackerMutation::begin(&pool, project_id, TaskActor::System)
        .await
        .expect("the mutation opens");
    let locked = common::tracker::locked(&mut mutation, task.id)
        .await
        .expect("the row reads");
    change_state(
        &mut mutation,
        &locked,
        &review,
        StateChangeOptions::default(),
    )
    .await
    .expect("the state moves");

    // What an event payload for this move would carry: the task as it is
    // inside the transaction.
    let inside = repository
        .load_task_dto_in(mutation.conn(), project_id, task.id)
        .await
        .expect("the task loads")
        .expect("the task is there");
    assert_eq!(inside.state, "review");

    // The pool is still on the committed row.
    let outside = repository
        .load_task_dto(project_id, task.id)
        .await
        .expect("the task loads")
        .expect("the task is there");
    assert_eq!(outside.state, "backlog");

    mutation.commit().await.expect("the mutation commits");

    let after = repository
        .load_task_dto(project_id, task.id)
        .await
        .expect("the task loads")
        .expect("the task is there");
    assert_eq!(after.state, "review");

    // An unknown task inside a mutation is `None`, as it is on the pool.
    let mut mutation = TrackerMutation::begin(&pool, project_id, TaskActor::System)
        .await
        .expect("the mutation opens");
    assert!(
        repository
            .load_task_dto_in(mutation.conn(), project_id, Uuid::new_v4())
            .await
            .expect("the task loads")
            .is_none()
    );
}
