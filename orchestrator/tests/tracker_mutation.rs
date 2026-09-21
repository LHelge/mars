//! `TrackerMutation` against a real Postgres (`CLAUDE.md`, "Testing
//! expectations").
//!
//! The mutation context is the one code path every tracker writer goes
//! through, so what is asserted here is the set of promises the rest of the
//! tracker is allowed to rely on (`ARCHITECTURE.md`, "Task tracker" → "One
//! mutation at a time per project"; `docs/data-model.md`, "Tracker mutation
//! transactions").
//!
//! A committed mutation writes its events with the actor that made the change
//! and announces them exactly once, with the highest sequence of the batch. A
//! mutation that is dropped, or ended with `no_change`, writes nothing and
//! announces nothing: rollback exposes neither the change nor its events
//! (ADR 0028), which is what `SPEC.md`, "MCP tool contracts" promises for a
//! rejected or no-op tool call.
//!
//! The `task_sessions` link follows the actor and the commit. A session's
//! change creates it and a later one advances `last_touched_at` only; a user's
//! or the orchestrator's change creates nothing, because the table records
//! which *sessions* worked on a task (ADR 0030).
//!
//! Last, the lock. `begin` is the serialisation point, asserted the only way a
//! lock can be: a second mutation on the same project must wait for the first
//! to commit, while one on another project must not.
//!
//! Needs a container engine; see `tests/common/db.rs`.

mod common;

use std::time::Duration;

use axum::http::StatusCode;
use mars_orchestrator::events::{TaskActor, TaskEvent, TaskEventKind};
use mars_orchestrator::models::{NewEvent, TaskRef, TaskState};
use mars_orchestrator::prelude::*;
use mars_orchestrator::repositories::{SessionRepository, TaskRepository};
use mars_orchestrator::tracker::{Escalation, TaskDto, TrackerMutation, delete_task};
use serde_json::json;
use sqlx::Postgres;
use sqlx::postgres::PgListener;
use tokio::time::{sleep, timeout};
use uuid::Uuid;

/// Not a credential: an obviously fake stand-in for the Argon2id PHC string
/// the seeded user would carry (`CLAUDE.md`, rule 3).
const FAKE_PASSWORD_HASH: &str = "$argon2id$fake$hash";

/// Not a real image: the stub the session tests replay a fixture transcript
/// with.
const TEST_IMAGE: &str = "localhost/mars-session-stub:test";

/// How long a blocked transaction is given to prove it is blocked, and how
/// long a stream that should be silent is watched.
const BLOCKED_FOR: Duration = Duration::from_millis(400);

/// How long an unblocked transaction — or an expected notification — is given
/// once nothing should be in its way.
const UNBLOCKED_WITHIN: Duration = Duration::from_secs(10);

/// The rows a project's tracker needs before a mutation has anything to say.
///
/// Seeded with unchecked statements rather than through their repositories:
/// this file is about `TrackerMutation`, and the foreign keys are all it needs
/// from the others.
struct Fixture {
    project_id: Uuid,
    user_id: Uuid,
    session_id: Uuid,
    task_id: Uuid,
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
    let session_id = seed_session(pool, project_id, profile_id).await;

    seed_default_states(pool, project_id).await;
    let state_id = state_id(pool, project_id, "backlog").await;
    let task_id = seed_task(pool, project_id, state_id, 1).await;

    Fixture {
        project_id,
        user_id,
        session_id,
        task_id,
    }
}

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

    project_id
}

async fn seed_profile(pool: &PgPool, project_id: Uuid) -> Uuid {
    let profile_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO agent_profiles (id, project_id, name, image, partial_messages)
         VALUES ($1, $2, 'default', $3, TRUE)",
    )
    .bind(profile_id)
    .bind(project_id)
    .bind(TEST_IMAGE)
    .execute(pool)
    .await
    .expect("the profile seeds");

    profile_id
}

async fn seed_session(pool: &PgPool, project_id: Uuid, profile_id: Uuid) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO sessions (id, project_id, profile_id, kind, base_ref, branch, mcp_token_hash)
        VALUES ($1, $2, $3, 'conversational'::profile_kind, 'main', $4, $5)
        "#,
    )
    .bind(id)
    .bind(project_id)
    .bind(profile_id)
    .bind(format!("session/{id}"))
    // Not a credential: the column stores a hash, and this is an obviously
    // fake one (`CLAUDE.md`, rule 3).
    .bind(format!("fake-token-hash-{id}"))
    .execute(pool)
    .await
    .expect("the session seeds");

    id
}

async fn seed_task(pool: &PgPool, project_id: Uuid, state_id: Uuid, number: i32) -> Uuid {
    let task_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO tasks (id, project_id, number, title, state_id) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(task_id)
    .bind(project_id)
    .bind(number)
    .bind(format!("task {number}"))
    .bind(state_id)
    .execute(pool)
    .await
    .expect("the task seeds");

    task_id
}

/// Create the default state set in its own committed mutation.
async fn seed_default_states(pool: &PgPool, project_id: Uuid) {
    let repository = TaskRepository::new(pool);
    let mut mutation = TrackerMutation::begin(pool, project_id, TaskActor::System)
        .await
        .expect("the mutation opens");
    repository
        .insert_default_states(mutation.conn(), project_id)
        .await
        .expect("the default states insert");
    mutation.commit().await.expect("the mutation commits");
}

async fn state_id(pool: &PgPool, project_id: Uuid, name: &str) -> Uuid {
    TaskRepository::new(pool)
        .find_state_by_name(project_id, name)
        .await
        .unwrap()
        .unwrap_or_else(|| panic!("the project has a state named {name}"))
        .id
}

/// The task as an event payload carries it, read through an open mutation.
async fn task_dto(mutation: &mut TrackerMutation<'_>, pool: &PgPool, task_id: Uuid) -> TaskDto {
    let project_id = mutation.project_id();

    TaskRepository::new(pool)
        .load_task_dto_in(mutation.conn(), project_id, task_id)
        .await
        .expect("the task loads")
        .expect("the task is in this project")
}

/// Every committed event of the project, oldest first.
async fn events(pool: &PgPool, project_id: Uuid) -> Vec<TaskEvent> {
    TaskRepository::new(pool)
        .list_task_events_after(project_id, 0, 100)
        .await
        .expect("the events read")
        .into_iter()
        .map(|row| TaskEvent::from_row(row).expect("the row is a documented event"))
        .collect()
}

async fn session_links(
    pool: &PgPool,
    task_id: Uuid,
) -> Vec<mars_orchestrator::tracker::TaskSessionLinkDto> {
    TaskRepository::new(pool)
        .list_task_sessions(task_id)
        .await
        .expect("the links read")
}

/// The project's states, for a `states_changed` payload.
async fn states(pool: &PgPool, project_id: Uuid) -> Vec<TaskState> {
    TaskRepository::new(pool)
        .list_states(project_id)
        .await
        .expect("the states read")
}

/// Nothing arrives on the channel for as long as a blocked transaction is
/// given to prove itself blocked.
async fn assert_silent(listener: &mut PgListener) {
    assert!(
        timeout(BLOCKED_FOR, listener.recv()).await.is_err(),
        "an unexpected notification arrived on task_events",
    );
}

/// Take the project lock and release it, which cannot return until every
/// earlier transaction on that project has finished rolling back.
async fn settle(pool: &PgPool, project_id: Uuid) {
    let mutation = TrackerMutation::begin(pool, project_id, TaskActor::System)
        .await
        .expect("the lock is free");
    mutation
        .no_change()
        .await
        .expect("the empty mutation rolls back");
}

#[tokio::test]
async fn a_committed_event_carries_its_actor_and_is_announced_once() {
    let (_postgres, pool) = common::db::test_pool_with(6).await;
    let fixture = seed(&pool).await;
    let actor = TaskActor::User {
        user_id: fixture.user_id,
    };

    let mut listener = PgListener::connect_with(&pool).await.unwrap();
    listener.listen("task_events").await.unwrap();

    let mut mutation = TrackerMutation::begin(&pool, fixture.project_id, actor)
        .await
        .expect("the mutation opens");
    assert_eq!(mutation.actor(), actor);
    assert_eq!(mutation.project_id(), fixture.project_id);
    // The locked row, not a second read: `max_attempts` is the project's
    // default until somebody changes it under this very lock.
    assert_eq!(mutation.project().max_attempts, 3);

    let task = task_dto(&mut mutation, &pool, fixture.task_id).await;
    mutation
        .emit_task(TaskEventKind::Created, &task)
        .expect("the event is emitted");

    let outcome = mutation.commit().await.expect("the mutation commits");
    assert_eq!(outcome.seqs, vec![1]);
    assert_eq!(outcome.last_seq(), Some(1));
    assert!(outcome.escalations.is_empty());

    let stored = events(&pool, fixture.project_id).await;
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].kind, TaskEventKind::Created);
    assert_eq!(stored[0].actor, actor);
    assert_eq!(stored[0].task_id, Some(fixture.task_id));
    assert_eq!(stored[0].task.as_ref().map(|t| t.id), Some(fixture.task_id));
    assert_eq!(
        stored[0].task.as_ref().map(|t| t.state.as_str()),
        Some("backlog")
    );

    let notification = timeout(UNBLOCKED_WITHIN, listener.recv())
        .await
        .expect("a committed batch notifies")
        .unwrap();
    assert_eq!(notification.channel(), "task_events");
    assert_eq!(notification.payload(), format!("{}:1", fixture.project_id));
    assert_silent(&mut listener).await;
}

#[tokio::test]
async fn a_batch_takes_consecutive_sequences_and_notifies_once() {
    let (_postgres, pool) = common::db::test_pool_with(6).await;
    let fixture = seed(&pool).await;

    let mut listener = PgListener::connect_with(&pool).await.unwrap();
    listener.listen("task_events").await.unwrap();

    let mut mutation = TrackerMutation::begin(
        &pool,
        fixture.project_id,
        TaskActor::Session {
            session_id: fixture.session_id,
        },
    )
    .await
    .expect("the mutation opens");

    let task = task_dto(&mut mutation, &pool, fixture.task_id).await;
    let board = states(&pool, fixture.project_id).await;

    // Emission order is stream order, so the sequences follow these calls.
    mutation
        .emit_state(
            TaskEventKind::StateChanged,
            &task,
            "backlog",
            "ready",
            Some("planned"),
        )
        .expect("the state event is emitted");
    mutation
        .emit_states_changed(&board)
        .expect("the board event is emitted");

    let outcome = mutation.commit().await.expect("the mutation commits");
    assert_eq!(outcome.seqs, vec![1, 2]);

    let stored = events(&pool, fixture.project_id).await;
    assert_eq!(stored.len(), 2);
    assert_eq!(stored[0].kind, TaskEventKind::StateChanged);
    assert_eq!(stored[0].from.as_deref(), Some("backlog"));
    assert_eq!(stored[0].to.as_deref(), Some("ready"));
    assert_eq!(stored[0].reason.as_deref(), Some("planned"));
    assert_eq!(stored[1].kind, TaskEventKind::StatesChanged);
    // A project-wide event names no task.
    assert_eq!(stored[1].task_id, None);
    assert_eq!(stored[1].states.as_ref().map(Vec::len), Some(board.len()));

    // One notification for the batch, carrying its highest sequence.
    let notification = timeout(UNBLOCKED_WITHIN, listener.recv())
        .await
        .expect("a committed batch notifies")
        .unwrap();
    assert_eq!(notification.payload(), format!("{}:2", fixture.project_id));
    assert_silent(&mut listener).await;
}

#[tokio::test]
async fn dropping_a_mutation_writes_nothing_and_announces_nothing() {
    let (_postgres, pool) = common::db::test_pool_with(6).await;
    let fixture = seed(&pool).await;

    let mut listener = PgListener::connect_with(&pool).await.unwrap();
    listener.listen("task_events").await.unwrap();

    {
        let mut mutation = TrackerMutation::begin(
            &pool,
            fixture.project_id,
            TaskActor::Session {
                session_id: fixture.session_id,
            },
        )
        .await
        .expect("the mutation opens");

        let task = task_dto(&mut mutation, &pool, fixture.task_id).await;
        mutation
            .emit_task(TaskEventKind::Claimed, &task)
            .expect("the event is emitted");
        mutation.touch_actor(fixture.task_id);
    }

    // Taking the lock again cannot succeed until the dropped transaction has
    // finished rolling back, so this is the synchronisation point.
    settle(&pool, fixture.project_id).await;

    assert!(events(&pool, fixture.project_id).await.is_empty());
    assert!(session_links(&pool, fixture.task_id).await.is_empty());
    assert_silent(&mut listener).await;
}

#[tokio::test]
async fn no_change_rolls_back_explicitly() {
    let (_postgres, pool) = common::db::test_pool_with(6).await;
    let fixture = seed(&pool).await;

    let mut listener = PgListener::connect_with(&pool).await.unwrap();
    listener.listen("task_events").await.unwrap();

    let mut mutation = TrackerMutation::begin(
        &pool,
        fixture.project_id,
        TaskActor::Session {
            session_id: fixture.session_id,
        },
    )
    .await
    .expect("the mutation opens");

    let task = task_dto(&mut mutation, &pool, fixture.task_id).await;
    mutation
        .emit_task(TaskEventKind::Released, &task)
        .expect("the event is emitted");
    mutation.touch_actor(fixture.task_id);
    mutation.record_escalation(escalation(&fixture));

    mutation.no_change().await.expect("the mutation rolls back");

    assert!(events(&pool, fixture.project_id).await.is_empty());
    assert!(session_links(&pool, fixture.task_id).await.is_empty());
    assert_silent(&mut listener).await;
}

#[tokio::test]
async fn a_session_actor_is_linked_once_and_a_later_change_only_advances_the_last_touch() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let actor = TaskActor::Session {
        session_id: fixture.session_id,
    };

    let mut mutation = TrackerMutation::begin(&pool, fixture.project_id, actor)
        .await
        .expect("the mutation opens");
    let task = task_dto(&mut mutation, &pool, fixture.task_id).await;
    mutation
        .emit_task(TaskEventKind::Claimed, &task)
        .expect("the event is emitted");
    // Twice in one mutation, and once more by hand for the same pair: all one
    // upsert.
    mutation.touch_actor(fixture.task_id);
    mutation.touch_actor(fixture.task_id);
    mutation.touch(fixture.task_id, fixture.session_id);
    mutation.commit().await.expect("the mutation commits");

    let first = session_links(&pool, fixture.task_id).await;
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].session_id, fixture.session_id);
    // Both timestamps are the transaction's `NOW()`.
    assert_eq!(first[0].first_touched_at, first[0].last_touched_at);

    let mut mutation = TrackerMutation::begin(&pool, fixture.project_id, actor)
        .await
        .expect("the second mutation opens");
    let task = task_dto(&mut mutation, &pool, fixture.task_id).await;
    mutation
        .emit_task(TaskEventKind::Updated, &task)
        .expect("the event is emitted");
    mutation.touch_actor(fixture.task_id);
    mutation
        .commit()
        .await
        .expect("the second mutation commits");

    let second = session_links(&pool, fixture.task_id).await;
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].first_touched_at, first[0].first_touched_at);
    assert!(
        second[0].last_touched_at > first[0].last_touched_at,
        "a repeated touch must advance only last_touched_at",
    );
}

#[tokio::test]
async fn a_user_or_system_actor_is_never_linked_and_an_eventless_mutation_touches_nothing() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;

    for actor in [
        TaskActor::User {
            user_id: fixture.user_id,
        },
        TaskActor::System,
    ] {
        let mut mutation = TrackerMutation::begin(&pool, fixture.project_id, actor)
            .await
            .expect("the mutation opens");
        let task = task_dto(&mut mutation, &pool, fixture.task_id).await;
        mutation
            .emit_task(TaskEventKind::Updated, &task)
            .expect("the event is emitted");
        mutation.touch_actor(fixture.task_id);
        mutation.commit().await.expect("the mutation commits");

        assert!(
            session_links(&pool, fixture.task_id).await.is_empty(),
            "{actor:?} must not create a task_sessions row",
        );
    }

    // A mutation with no events commits, but a link records a change it never
    // announced, so it is not written either.
    let mut mutation = TrackerMutation::begin(
        &pool,
        fixture.project_id,
        TaskActor::Session {
            session_id: fixture.session_id,
        },
    )
    .await
    .expect("the eventless mutation opens");
    mutation.touch_actor(fixture.task_id);
    let outcome = mutation.commit().await.expect("it still commits");

    assert!(outcome.seqs.is_empty());
    assert!(session_links(&pool, fixture.task_id).await.is_empty());
}

#[tokio::test]
async fn an_escalation_comes_back_with_the_commit() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;

    let mut mutation = TrackerMutation::begin(&pool, fixture.project_id, TaskActor::System)
        .await
        .expect("the mutation opens");
    let task = task_dto(&mut mutation, &pool, fixture.task_id).await;
    mutation
        .emit_state(
            TaskEventKind::Escalated,
            &task,
            "ready",
            "needs_human",
            Some("stalled"),
        )
        .expect("the event is emitted");
    mutation.record_escalation(escalation(&fixture));

    let outcome = mutation.commit().await.expect("the mutation commits");
    assert_eq!(outcome.escalations, vec![escalation(&fixture)]);
    // The email is the caller's to send, after the transaction is closed.
    assert_eq!(outcome.seqs.len(), 1);
}

#[tokio::test]
async fn an_unknown_project_is_not_found() {
    let (_postgres, pool) = common::db::test_pool().await;
    seed(&pool).await;

    // Mapped to `()` because a `TrackerMutation` is not `Debug`: it owns an
    // open transaction, which is not a thing to print.
    let error = TrackerMutation::begin(&pool, Uuid::new_v4(), TaskActor::System)
        .await
        .map(|_| ())
        .expect_err("there is no such project");
    assert_eq!(error.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn mutations_serialise_per_project_and_not_across_projects() {
    // Three mutations at once, plus the connections the helpers take.
    let (_postgres, pool) = common::db::test_pool_with(6).await;
    let fixture = seed(&pool).await;
    let other_project = seed_project(&pool).await;

    // The first mutation takes the lock and keeps it.
    let first = TrackerMutation::begin(&pool, fixture.project_id, TaskActor::System)
        .await
        .expect("the first mutation opens");

    // The second wants the same project and must wait at the lock.
    let second_pool = pool.clone();
    let project_id = fixture.project_id;
    let second = tokio::spawn(async move {
        let mutation = TrackerMutation::begin(&second_pool, project_id, TaskActor::System)
            .await
            .unwrap();
        mutation.commit().await.unwrap();
    });

    // The third wants a different project row and must not.
    let third_pool = pool.clone();
    let third = tokio::spawn(async move {
        let mutation = TrackerMutation::begin(&third_pool, other_project, TaskActor::System)
            .await
            .unwrap();
        mutation.commit().await.unwrap();
    });

    timeout(UNBLOCKED_WITHIN, third)
        .await
        .expect("another project's mutation is not blocked")
        .expect("the third mutation does not panic");

    sleep(BLOCKED_FOR).await;
    assert!(
        !second.is_finished(),
        "the second mutation did not wait for the project lock",
    );

    first.commit().await.expect("the first mutation commits");

    timeout(UNBLOCKED_WITHIN, second)
        .await
        .expect("the second mutation proceeds once the lock is free")
        .expect("the second mutation does not panic");
}

/// A session-only writer, as the session owner is for a `result` line: the
/// session row lock, an event batch, and then the usage counters — a second
/// `UPDATE` of a row this transaction already rewrote, which is what makes
/// Postgres check the row's foreign keys again and take `FOR KEY SHARE` on the
/// project, the task and the hand-off it names (`ARCHITECTURE.md`, "Task
/// tracker", the lock strength).
async fn open_session_write(
    pool: &PgPool,
    session_id: Uuid,
) -> sqlx::Transaction<'static, Postgres> {
    let repository = SessionRepository::new(pool);
    let mut tx = pool.begin().await.expect("the session writer opens");
    repository
        .append_events(
            &mut tx,
            session_id,
            &[NewEvent::now(
                "assistant_message",
                json!({ "text": "fake" }),
            )],
        )
        .await
        .expect("the batch is appended under the session row lock");

    tx
}

async fn finish_session_write(
    pool: &PgPool,
    mut tx: sqlx::Transaction<'static, Postgres>,
    session_id: Uuid,
) -> Result<()> {
    SessionRepository::new(pool)
        .add_usage(&mut tx, session_id, 0.01, 1, 1)
        .await?;
    tx.commit().await?;

    Ok(())
}

#[tokio::test]
async fn a_session_writer_and_a_mutation_naming_its_session_do_not_deadlock() {
    let (_postgres, pool) = common::db::test_pool_with(6).await;
    let fixture = seed(&pool).await;

    // The owner's half first: the session row is held.
    let writer = open_session_write(&pool, fixture.session_id).await;

    // The tracker's half: the project row is held, and the commit will write
    // a session link and an event whose foreign keys name that session.
    let mut mutation = TrackerMutation::begin(
        &pool,
        fixture.project_id,
        TaskActor::Session {
            session_id: fixture.session_id,
        },
    )
    .await
    .expect("the mutation opens");
    let task = task_dto(&mut mutation, &pool, fixture.task_id).await;
    mutation
        .emit_task(TaskEventKind::Updated, &task)
        .expect("the event is emitted");
    mutation.touch_actor(fixture.task_id);

    // With the project row held `FOR UPDATE` these two waited for each other
    // until Postgres broke the cycle with 40P01 (kb48s).
    let (written, committed) = timeout(UNBLOCKED_WITHIN, async {
        tokio::join!(
            finish_session_write(&pool, writer, fixture.session_id),
            mutation.commit(),
        )
    })
    .await
    .expect("neither transaction waits for the other indefinitely");

    written.expect("the session writer commits");
    committed.expect("the mutation commits");
}

#[tokio::test]
async fn deleting_a_task_waits_for_its_session_instead_of_deadlocking_with_it() {
    let (_postgres, pool) = common::db::test_pool_with(6).await;
    let fixture = seed(&pool).await;
    // A row-level fact no verb arranges without a launch: the session is the
    // one launched for this task.
    sqlx::query("UPDATE sessions SET task_id = $2 WHERE id = $1")
        .bind(fixture.session_id)
        .bind(fixture.task_id)
        .execute(&pool)
        .await
        .expect("the session is linked to the task");

    let writer = open_session_write(&pool, fixture.session_id).await;

    let deletion_pool = pool.clone();
    let (project_id, task_id) = (fixture.project_id, fixture.task_id);
    let deletion = tokio::spawn(async move {
        let mut mutation =
            TrackerMutation::begin(&deletion_pool, project_id, TaskActor::System).await?;
        let task = TaskRepository::new(&deletion_pool)
            .find_task_for_update(mutation.conn(), project_id, TaskRef::Id(task_id))
            .await?
            .ok_or(Error::NotFound)?;
        delete_task(&mut mutation, &task).await?;
        mutation.commit().await?;

        Ok::<(), Error>(())
    });

    // The deletion has to clear `sessions.task_id`, so it waits for the
    // writer — holding nothing the writer's foreign-key re-check needs.
    sleep(BLOCKED_FOR).await;
    assert!(
        !deletion.is_finished(),
        "the deletion did not wait for the session row",
    );

    timeout(
        UNBLOCKED_WITHIN,
        finish_session_write(&pool, writer, fixture.session_id),
    )
    .await
    .expect("the session writer is not blocked by the deletion")
    .expect("the session writer commits");

    timeout(UNBLOCKED_WITHIN, deletion)
        .await
        .expect("the deletion proceeds once the session row is free")
        .expect("the deletion does not panic")
        .expect("the deletion commits");
}

fn escalation(fixture: &Fixture) -> Escalation {
    Escalation {
        project_id: fixture.project_id,
        project_name: format!("project-{}", fixture.project_id),
        task_id: fixture.task_id,
        task_number: 1,
        task_title: "task 1".to_string(),
        assignee_user_id: Some(fixture.user_id),
        reason: "stalled".to_string(),
    }
}
