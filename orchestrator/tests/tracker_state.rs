//! `tracker::state::change_state` against a real Postgres (`CLAUDE.md`,
//! "Testing expectations").
//!
//! One function carries every move a task can make, so what is asserted here
//! is the hand-off contract the routes, the MCP tools, the reaper and the
//! escalation paths are all allowed to assume (`ARCHITECTURE.md`, "Task
//! tracker" → "The lease is the worker", "Parents" and "Attempts and
//! escalation"; `SPEC.md`, "Tasks"; `docs/data-model.md`, `tasks`):
//!
//! - a different state is a hand-off — the lease goes, whoever held it,
//!   `attempts` returns to zero and exactly one `state_changed` is written;
//! - the current state is a no-op that writes nothing and emits nothing,
//!   leaving the lease, the counter and `closed_at` where they were;
//! - entering a terminal state closes the task and unblocks its dependants,
//!   and leaving one reopens it and blocks them again;
//! - the last open child closes its parent, in the same transaction, with
//!   actor `system`, into the terminal state with the lowest position — and
//!   reopening that child never reopens the parent;
//! - an unknown state name is a 400 that names the valid ones.
//!
//! Driven through `TrackerMutation` directly rather than over HTTP: the rules
//! are the tracker's, and the routes that will call them are other tasks'.
//!
//! Needs a container engine; see `tests/common/db.rs`.

mod common;

use axum::http::StatusCode;
use mars_orchestrator::events::{TaskActor, TaskEvent, TaskEventKind};
use mars_orchestrator::models::{
    NewSession, NewTask, ProfileKind, Task, TaskDependencyKind, TaskRef,
};
use mars_orchestrator::prelude::*;
use mars_orchestrator::repositories::{SessionRepository, TaskRepository};
use mars_orchestrator::tracker::graph::recompute_blocked;
use mars_orchestrator::tracker::state::{
    StateChangeOptions, StateChangeResult, StateEventKind, change_state, resolve_state,
    resolve_state_in_pool,
};
use mars_orchestrator::tracker::{TaskDto, TrackerMutation};
use uuid::Uuid;

/// Not a credential: an obviously fake stand-in for the Argon2id PHC string
/// the seeded user would carry (`CLAUDE.md`, rule 3).
const FAKE_PASSWORD_HASH: &str = "$argon2id$fake$hash";

/// Not a real image: the stub the session tests replay a fixture transcript
/// with.
const TEST_IMAGE: &str = "localhost/mars-session-stub:test";

/// The message an unknown state name answers with, for the default state set.
const UNKNOWN_STATE: &str = "unknown state \"shipped\"; valid states are: \
     backlog, ready, review, merge, needs_human, done, cancelled";

/// A project with the documented default states, a profile to hang sessions
/// off, and the user every change is attributed to.
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

    let project_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id, name, remote_url) VALUES ($1, $2, $3)")
        .bind(project_id)
        .bind(format!("project-{project_id}"))
        // `.invalid` can never resolve (rule 3).
        .bind("https://example.invalid/org/repo.git")
        .execute(pool)
        .await
        .expect("the project seeds");

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

    let repository = TaskRepository::new(pool);
    let mut mutation = TrackerMutation::begin(pool, project_id, TaskActor::System)
        .await
        .expect("the mutation opens");
    repository
        .insert_default_states(mutation.conn(), project_id)
        .await
        .expect("the default states insert");
    mutation.commit().await.expect("the mutation commits");

    Fixture {
        user_id,
        project_id,
        profile_id,
    }
}

/// A session, so a lease has something to point at.
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

/// A task of this project in the given state, optionally under a parent.
async fn task(pool: &PgPool, project_id: Uuid, title: &str, state: &str) -> Task {
    task_under(pool, project_id, title, state, None).await
}

async fn task_under(
    pool: &PgPool,
    project_id: Uuid,
    title: &str,
    state: &str,
    parent_id: Option<Uuid>,
) -> Task {
    let mut new = NewTask::new(project_id, title).expect("the title parses");
    new.state_id = Some(state_id(pool, project_id, state).await);
    new.parent_id = parent_id;

    let repository = TaskRepository::new(pool);
    let mut mutation = TrackerMutation::begin(pool, project_id, TaskActor::System)
        .await
        .expect("the mutation opens");
    let inserted = repository
        .insert_task(mutation.conn(), project_id, &new)
        .await
        .expect("the task inserts");
    mutation.commit().await.expect("the mutation commits");

    inserted
}

/// The id of a project's state by name.
async fn state_id(pool: &PgPool, project_id: Uuid, name: &str) -> Uuid {
    TaskRepository::new(pool)
        .find_state_by_name(project_id, name)
        .await
        .expect("the state reads")
        .expect("the project has this state")
        .id
}

/// Put a lease and an attempt count on a task, the way a claim would.
async fn claim(pool: &PgPool, project_id: Uuid, task_id: Uuid, session_id: Uuid, attempts: i16) {
    common::tracker::hold_with_attempts(pool, project_id, task_id, session_id, attempts).await;
}

/// Add one `blocks` edge and store the flag it implies.
async fn blocks(pool: &PgPool, project_id: Uuid, dependant: Uuid, prerequisite: Uuid) {
    let repository = TaskRepository::new(pool);
    let mut mutation = TrackerMutation::begin(pool, project_id, TaskActor::System)
        .await
        .expect("the mutation opens");
    repository
        .insert_dependency(
            mutation.conn(),
            project_id,
            dependant,
            prerequisite,
            TaskDependencyKind::Blocks,
        )
        .await
        .expect("the edge inserts");
    recompute_blocked(&mut mutation, &[dependant])
        .await
        .expect("the flag is recomputed");
    mutation.commit().await.expect("the mutation commits");
}

/// Store the `blocked` flag these tasks' graphs imply, in a mutation of its
/// own — how a parent comes to be blocked by children created before it was
/// anybody's business.
async fn recompute(pool: &PgPool, project_id: Uuid, task_ids: &[Uuid]) {
    let mut mutation = TrackerMutation::begin(pool, project_id, TaskActor::System)
        .await
        .expect("the mutation opens");
    recompute_blocked(&mut mutation, task_ids)
        .await
        .expect("the flags are recomputed");
    mutation.commit().await.expect("the mutation commits");
}

/// Move a task the way a caller would: under the project lock, on the row as
/// it is there, committing only when the change succeeded.
async fn change(
    pool: &PgPool,
    project_id: Uuid,
    actor: TaskActor,
    task_id: Uuid,
    target: &str,
    opts: StateChangeOptions,
) -> Result<StateChangeResult> {
    let repository = TaskRepository::new(pool);
    let mut mutation = TrackerMutation::begin(pool, project_id, actor)
        .await
        .expect("the mutation opens");

    let moved = async {
        let task = repository
            .find_task_for_update(mutation.conn(), project_id, TaskRef::Id(task_id))
            .await?
            .ok_or(Error::NotFound)?;
        let state = resolve_state(&mut mutation, target).await?;

        change_state(&mut mutation, &task, &state, opts).await
    }
    .await;

    match moved {
        Ok(result) => {
            mutation.commit().await.expect("the mutation commits");
            Ok(result)
        }
        Err(error) => {
            mutation.no_change().await.expect("the mutation rolls back");
            Err(error)
        }
    }
}

/// The plain hand-off every test but the escalation one makes.
fn plain() -> StateChangeOptions {
    StateChangeOptions::default()
}

/// The task as it is committed.
async fn read(pool: &PgPool, project_id: Uuid, task_id: Uuid) -> TaskDto {
    TaskRepository::new(pool)
        .load_task_dto(project_id, task_id)
        .await
        .expect("the task reads")
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

/// The events written after `seq`, which is how each step of a test reads
/// only its own.
async fn events_after(pool: &PgPool, project_id: Uuid, seq: i64) -> Vec<TaskEvent> {
    events(pool, project_id)
        .await
        .into_iter()
        .filter(|event| event.seq > seq)
        .collect()
}

/// The highest sequence written so far.
async fn last_seq(pool: &PgPool, project_id: Uuid) -> i64 {
    events(pool, project_id)
        .await
        .last()
        .map(|event| event.seq)
        .unwrap_or(0)
}

#[tokio::test]
async fn a_hand_off_clears_the_lease_and_resets_the_attempts() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let session_id = seed_session(&pool, project_id, fixture.profile_id).await;

    let subject = task(&pool, project_id, "implement it", "ready").await;
    claim(&pool, project_id, subject.id, session_id, 2).await;
    // The arranging claims are behind the cursor; what follows is the move's
    // own.
    let seq = last_seq(&pool, project_id).await;

    // A user is not bound by anybody's lease: the hand-off clears it even
    // though the user is not the holder (`SPEC.md`, "Tasks").
    let actor = TaskActor::User {
        user_id: fixture.user_id,
    };
    let moved = change(&pool, project_id, actor, subject.id, "review", plain())
        .await
        .expect("the move succeeds");

    assert!(moved.changed);
    assert_eq!(moved.task.state, "review");
    assert_eq!(moved.task.lease_holder_session_id, None);
    assert_eq!(moved.task.lease_since, None);
    assert_eq!(moved.task.attempts, 0);
    assert_eq!(moved.task.closed_at, None);

    let stored = read(&pool, project_id, subject.id).await;
    assert_eq!(stored, moved.task);

    let written = events_after(&pool, project_id, seq).await;
    assert_eq!(written.len(), 1);
    assert_eq!(written[0].kind, TaskEventKind::StateChanged);
    assert_eq!(written[0].task_id, Some(subject.id));
    assert_eq!(written[0].actor, actor);
    assert_eq!(written[0].from.as_deref(), Some("ready"));
    assert_eq!(written[0].to.as_deref(), Some("review"));
    assert_eq!(written[0].reason, None);
    // The payload carries the task as it is after the move.
    assert_eq!(
        written[0].task.as_ref().map(|task| task.attempts),
        Some(0_i16)
    );
}

#[tokio::test]
async fn assigning_the_current_state_changes_nothing_at_all() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let session_id = seed_session(&pool, project_id, fixture.profile_id).await;

    let subject = task(&pool, project_id, "still mine", "ready").await;
    claim(&pool, project_id, subject.id, session_id, 2).await;
    // The arranging claims are behind the cursor; what follows is the move's
    // own.
    let seq = last_seq(&pool, project_id).await;
    let before = read(&pool, project_id, subject.id).await;

    let actor = TaskActor::Session { session_id };
    let moved = change(&pool, project_id, actor, subject.id, "ready", plain())
        .await
        .expect("the no-op succeeds");

    assert!(!moved.changed);
    assert_eq!(moved.task, before);

    // The lease, the counter and the row's own timestamp all survive.
    let after = read(&pool, project_id, subject.id).await;
    assert_eq!(after, before);
    assert_eq!(after.lease_holder_session_id, Some(session_id));
    assert_eq!(after.attempts, 2);

    assert!(events_after(&pool, project_id, seq).await.is_empty());
}

#[tokio::test]
async fn closing_a_task_unblocks_its_dependants_and_reopening_blocks_them_again() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let actor = TaskActor::User {
        user_id: fixture.user_id,
    };

    let prerequisite = task(&pool, project_id, "first this", "ready").await;
    let dependant = task(&pool, project_id, "then this", "ready").await;
    blocks(&pool, project_id, dependant.id, prerequisite.id).await;
    assert!(read(&pool, project_id, dependant.id).await.blocked);

    let seq = last_seq(&pool, project_id).await;
    let closed = change(&pool, project_id, actor, prerequisite.id, "done", plain())
        .await
        .expect("the closure succeeds");
    assert!(closed.task.closed_at.is_some());

    // The task's own event first, then the flip it caused.
    let after_close = events_after(&pool, project_id, seq).await;
    assert_eq!(after_close.len(), 2);
    assert_eq!(after_close[0].kind, TaskEventKind::StateChanged);
    assert_eq!(after_close[0].task_id, Some(prerequisite.id));
    assert_eq!(after_close[0].to.as_deref(), Some("done"));
    assert_eq!(after_close[1].kind, TaskEventKind::Unblocked);
    assert_eq!(after_close[1].task_id, Some(dependant.id));
    assert!(!read(&pool, project_id, dependant.id).await.blocked);

    // Reopening is the same move in the other direction.
    let seq = last_seq(&pool, project_id).await;
    let reopened = change(&pool, project_id, actor, prerequisite.id, "ready", plain())
        .await
        .expect("the reopen succeeds");
    assert_eq!(reopened.task.closed_at, None);

    let after_reopen = events_after(&pool, project_id, seq).await;
    assert_eq!(after_reopen.len(), 2);
    assert_eq!(after_reopen[0].kind, TaskEventKind::StateChanged);
    assert_eq!(after_reopen[0].from.as_deref(), Some("done"));
    assert_eq!(after_reopen[1].kind, TaskEventKind::Blocked);
    assert_eq!(after_reopen[1].task_id, Some(dependant.id));
    assert!(read(&pool, project_id, dependant.id).await.blocked);
}

#[tokio::test]
async fn a_terminal_move_between_terminal_states_only_refreshes_the_closure() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let actor = TaskActor::User {
        user_id: fixture.user_id,
    };

    let prerequisite = task(&pool, project_id, "first this", "ready").await;
    let dependant = task(&pool, project_id, "then this", "ready").await;
    blocks(&pool, project_id, dependant.id, prerequisite.id).await;

    change(&pool, project_id, actor, prerequisite.id, "done", plain())
        .await
        .expect("the closure succeeds");
    let closed_at = read(&pool, project_id, prerequisite.id)
        .await
        .closed_at
        .expect("a closed task carries the timestamp");

    let seq = last_seq(&pool, project_id).await;
    let cancelled = change(
        &pool,
        project_id,
        actor,
        prerequisite.id,
        "cancelled",
        plain(),
    )
    .await
    .expect("the second closure succeeds");

    // Terminal-ness did not change, so the dependant is not recomputed and
    // nothing but the state event is written; `closed_at` is stamped anew.
    let after = events_after(&pool, project_id, seq).await;
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].kind, TaskEventKind::StateChanged);
    assert_eq!(after[0].from.as_deref(), Some("done"));
    assert_eq!(after[0].to.as_deref(), Some("cancelled"));
    assert!(cancelled.task.closed_at.expect("still closed") >= closed_at);
    assert!(!read(&pool, project_id, dependant.id).await.blocked);
}

#[tokio::test]
async fn the_last_open_child_closes_its_parent_as_the_system() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let session_id = seed_session(&pool, project_id, fixture.profile_id).await;
    let actor = TaskActor::User {
        user_id: fixture.user_id,
    };

    // A parent may be held; its closure clears the lease like any hand-off.
    // The claim comes before the children, because a blocked task is not
    // claimable and an open child is what blocks one.
    let parent = task(&pool, project_id, "the epic", "ready").await;
    claim(&pool, project_id, parent.id, session_id, 1).await;

    let first = task_under(&pool, project_id, "one", "ready", Some(parent.id)).await;
    let second = task_under(&pool, project_id, "two", "ready", Some(parent.id)).await;
    recompute(&pool, project_id, &[parent.id]).await;
    assert!(read(&pool, project_id, parent.id).await.blocked);

    // One child of two: the parent has an open child left, so nothing about
    // it changes — not its state and not its flag.
    let seq = last_seq(&pool, project_id).await;
    change(&pool, project_id, actor, first.id, "done", plain())
        .await
        .expect("the first child closes");

    let after_first = events_after(&pool, project_id, seq).await;
    assert_eq!(after_first.len(), 1);
    assert_eq!(after_first[0].task_id, Some(first.id));
    let parent_now = read(&pool, project_id, parent.id).await;
    assert_eq!(parent_now.state, "ready");
    assert!(parent_now.blocked);

    // The last one: the child's event, the parent's flag, then the parent's
    // own move, by the system.
    let seq = last_seq(&pool, project_id).await;
    change(&pool, project_id, actor, second.id, "done", plain())
        .await
        .expect("the second child closes");

    let after_second = events_after(&pool, project_id, seq).await;
    assert_eq!(after_second.len(), 3);
    assert_eq!(after_second[0].kind, TaskEventKind::StateChanged);
    assert_eq!(after_second[0].task_id, Some(second.id));
    assert_eq!(after_second[0].actor, actor);
    assert_eq!(after_second[1].kind, TaskEventKind::Unblocked);
    assert_eq!(after_second[1].task_id, Some(parent.id));
    assert_eq!(after_second[2].kind, TaskEventKind::StateChanged);
    assert_eq!(after_second[2].task_id, Some(parent.id));
    assert_eq!(after_second[2].actor, TaskActor::System);
    assert_eq!(after_second[2].from.as_deref(), Some("ready"));
    assert_eq!(after_second[2].to.as_deref(), Some("done"));

    let closed_parent = read(&pool, project_id, parent.id).await;
    assert_eq!(closed_parent.state, "done");
    assert!(closed_parent.closed_at.is_some());
    assert_eq!(closed_parent.lease_holder_session_id, None);
    assert_eq!(closed_parent.attempts, 0);
    assert!(!closed_parent.blocked);

    // Reopening a child never reopens the parent: it only blocks it again.
    let seq = last_seq(&pool, project_id).await;
    change(&pool, project_id, actor, second.id, "ready", plain())
        .await
        .expect("the child reopens");

    let after_reopen = events_after(&pool, project_id, seq).await;
    assert_eq!(after_reopen.len(), 2);
    assert_eq!(after_reopen[0].task_id, Some(second.id));
    assert_eq!(after_reopen[1].kind, TaskEventKind::Blocked);
    assert_eq!(after_reopen[1].task_id, Some(parent.id));

    let still_closed = read(&pool, project_id, parent.id).await;
    assert_eq!(still_closed.state, "done");
    assert!(still_closed.closed_at.is_some());
    assert!(still_closed.blocked);
}

#[tokio::test]
async fn a_parent_closes_into_the_terminal_state_with_the_lowest_position() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let actor = TaskActor::User {
        user_id: fixture.user_id,
    };

    // "The project's first terminal state" is a position, not the name
    // `done`: put `cancelled` in front of it and the parent follows.
    let cancelled = state_id(&pool, project_id, "cancelled").await;
    let repository = TaskRepository::new(&pool);
    let mut mutation = TrackerMutation::begin(&pool, project_id, TaskActor::System)
        .await
        .expect("the mutation opens");
    repository
        .move_state(mutation.conn(), project_id, cancelled, 5)
        .await
        .expect("the state moves");
    mutation.commit().await.expect("the mutation commits");

    let parent = task(&pool, project_id, "the epic", "ready").await;
    let only = task_under(&pool, project_id, "the only one", "ready", Some(parent.id)).await;
    recompute(&pool, project_id, &[parent.id]).await;

    let seq = last_seq(&pool, project_id).await;
    change(&pool, project_id, actor, only.id, "done", plain())
        .await
        .expect("the child closes");

    let written = events_after(&pool, project_id, seq).await;
    let parent_event = written.last().expect("the parent's move is the last event");
    assert_eq!(parent_event.kind, TaskEventKind::StateChanged);
    assert_eq!(parent_event.task_id, Some(parent.id));
    assert_eq!(parent_event.actor, TaskActor::System);
    assert_eq!(parent_event.to.as_deref(), Some("cancelled"));
    assert_eq!(read(&pool, project_id, parent.id).await.state, "cancelled");
}

#[tokio::test]
async fn an_already_closed_parent_is_left_alone() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let actor = TaskActor::User {
        user_id: fixture.user_id,
    };

    let parent = task(&pool, project_id, "the epic", "done").await;
    let child = task_under(&pool, project_id, "the only one", "ready", Some(parent.id)).await;
    recompute(&pool, project_id, &[parent.id]).await;
    assert!(read(&pool, project_id, parent.id).await.blocked);

    let seq = last_seq(&pool, project_id).await;
    change(&pool, project_id, actor, child.id, "done", plain())
        .await
        .expect("the child closes");

    // The child's own event and the parent's flag, and nothing more: a
    // terminal parent is neither moved nor reclosed.
    let written = events_after(&pool, project_id, seq).await;
    assert_eq!(written.len(), 2);
    assert_eq!(written[0].task_id, Some(child.id));
    assert_eq!(written[1].kind, TaskEventKind::Unblocked);
    assert_eq!(written[1].task_id, Some(parent.id));
    assert_eq!(read(&pool, project_id, parent.id).await.state, "done");
}

#[tokio::test]
async fn an_escalation_carries_its_reason_and_an_ordinary_move_does_not() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let session_id = seed_session(&pool, project_id, fixture.profile_id).await;
    let subject = task(&pool, project_id, "stuck", "ready").await;
    claim(&pool, project_id, subject.id, session_id, 3).await;
    // The arranging claims are behind the cursor; what follows is the
    // escalation's own.
    let seq = last_seq(&pool, project_id).await;

    let escalated = change(
        &pool,
        project_id,
        TaskActor::Session { session_id },
        subject.id,
        "needs_human",
        StateChangeOptions {
            event: StateEventKind::Escalated {
                reason: "the migration needs a decision".into(),
            },
            needs_human_reason: Some("the migration needs a decision".into()),
        },
    )
    .await
    .expect("the escalation succeeds");

    assert_eq!(escalated.task.state, "needs_human");
    assert_eq!(
        escalated.task.needs_human_reason.as_deref(),
        Some("the migration needs a decision"),
    );
    assert_eq!(escalated.task.attempts, 0);

    let written = events_after(&pool, project_id, seq).await;
    assert_eq!(written.len(), 1);
    assert_eq!(written[0].kind, TaskEventKind::Escalated);
    assert_eq!(written[0].to.as_deref(), Some("needs_human"));
    assert_eq!(
        written[0].reason.as_deref(),
        Some("the migration needs a decision"),
    );

    // A user dragging the card back out is a plain `state_changed`, and it
    // leaves the recorded reason where it is unless asked to change it.
    let seq = last_seq(&pool, project_id).await;
    let back = change(
        &pool,
        project_id,
        TaskActor::User {
            user_id: fixture.user_id,
        },
        subject.id,
        "ready",
        plain(),
    )
    .await
    .expect("the move back succeeds");

    assert_eq!(
        back.task.needs_human_reason.as_deref(),
        Some("the migration needs a decision"),
    );
    let after = events_after(&pool, project_id, seq).await;
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].kind, TaskEventKind::StateChanged);
    assert_eq!(after[0].reason, None);
}

#[tokio::test]
async fn an_unknown_state_name_lists_the_states_that_exist() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;

    let subject = task(&pool, project_id, "nowhere to go", "ready").await;
    let error = change(
        &pool,
        project_id,
        TaskActor::User {
            user_id: fixture.user_id,
        },
        subject.id,
        "shipped",
        plain(),
    )
    .await
    .expect_err("an unknown state is refused");

    assert_eq!(error.status(), StatusCode::BAD_REQUEST);
    assert_eq!(error.to_string(), UNKNOWN_STATE);

    // The lock-free form a board filter uses answers identically.
    let same = resolve_state_in_pool(&pool, project_id, "shipped")
        .await
        .expect_err("an unknown state is refused there too");
    assert_eq!(same.to_string(), UNKNOWN_STATE);

    // And a name that exists resolves, whichever way it is asked.
    let state = resolve_state_in_pool(&pool, project_id, "review")
        .await
        .expect("a known state resolves");
    assert_eq!(state.id, state_id(&pool, project_id, "review").await);

    assert!(events_after(&pool, project_id, 0).await.is_empty());
}
