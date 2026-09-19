//! The lease endings that can escalate, against a real Postgres (`CLAUDE.md`,
//! "Testing expectations").
//!
//! Three callers end a lease without a user behind them — an agent giving the
//! task back, the orchestrator releasing a dead session's leases, and an agent
//! asking for a person — and all three obey the same counter
//! (`ARCHITECTURE.md`, "Task tracker" → "Attempts and escalation", "Liveness
//! comes from the session, not from tool calls"; `SPEC.md`, `release` and
//! `needs_human`; `docs/data-model.md`, `tasks`):
//!
//! - three claim-and-release cycles put the task in `needs_human` with the
//!   three agents' comments, a system comment saying why, `attempts` back to
//!   zero and exactly one escalation for the caller to email;
//! - a release by a session that does not hold the task is a conflict that
//!   writes nothing at all;
//! - a dead session's leases are released with a system comment each, with
//!   actor `system`, escalating only the tasks that ran out of attempts, and
//!   `session_ended` and `stalled` differ only in what they say;
//! - `needs_human` escalates whatever the counter says, from any state and
//!   from no lease at all — but a task already waiting for a person is not
//!   escalated twice: the reason is recorded, the lease is released, and
//!   `attempts` survives.
//!
//! Driven through `TrackerMutation` directly, with sessions inserted through
//! `SessionRepository`: the rules are the tracker's, and the MCP tools and the
//! session hooks that will call them are other tasks'.
//!
//! Needs a container engine; see `tests/common/db.rs`.

mod common;

use chrono::Utc;
use mars_orchestrator::events::{TaskActor, TaskEvent, TaskEventKind};
use mars_orchestrator::models::{NewSession, NewTask, ProfileKind, Task, TaskComment};
use mars_orchestrator::prelude::*;
use mars_orchestrator::repositories::tasks::test_support::{StateFields, TaskRepositoryTestExt};
use mars_orchestrator::repositories::{SessionRepository, TaskRepository};
use mars_orchestrator::tracker::leases::{
    ReleaseReason, claim_for_profile, needs_human, release_by_agent, release_leases_for_session,
};
use mars_orchestrator::tracker::{Escalation, TaskDto, TrackerMutation};
use uuid::Uuid;

/// Not a credential: an obviously fake stand-in for the Argon2id PHC string
/// the seeded user would carry (`CLAUDE.md`, rule 3).
const FAKE_PASSWORD_HASH: &str = "$argon2id$fake$hash";

/// Not a real image: the stub the session tests replay a fixture transcript
/// with.
const TEST_IMAGE: &str = "localhost/mars-session-stub:test";

/// The project default this suite relies on (`SPEC.md`, "Projects").
const MAX_ATTEMPTS: i16 = 3;

/// A release by a session that holds nothing (`SPEC.md`, `release`).
const NOT_HELD_BY_SESSION: &str = "task is not held by this session";
/// `needs_human` on somebody else's task.
const HELD_BY_ANOTHER: &str = "task is held by another session";

/// A project with the documented default states, a profile to hang sessions
/// off, and a user to own it.
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

    let mut mutation = TrackerMutation::begin(pool, project_id, TaskActor::System)
        .await
        .expect("the mutation opens");
    TaskRepository::new(pool)
        .insert_default_states(mutation.conn(), project_id)
        .await
        .expect("the default states insert");
    mutation.commit().await.expect("the mutation commits");

    Fixture {
        project_id,
        profile_id,
    }
}

/// A session, so a lease has something to point at.
async fn seed_session(pool: &PgPool, project_id: Uuid, profile_id: Uuid) -> Uuid {
    let session = NewSession::new(
        project_id,
        profile_id,
        ProfileKind::Ephemeral,
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

/// A task of this project in the given state.
async fn task(pool: &PgPool, project_id: Uuid, title: &str, state: &str) -> Task {
    let mut new = NewTask::new(project_id, title).expect("the title parses");
    new.state_id = Some(state_id(pool, project_id, state).await);

    let mut mutation = TrackerMutation::begin(pool, project_id, TaskActor::System)
        .await
        .expect("the mutation opens");
    let inserted = TaskRepository::new(pool)
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

/// Claim as an agent would, through the tool's own path.
async fn claim(pool: &PgPool, project_id: Uuid, task_id: Uuid, session_id: Uuid) -> TaskDto {
    let served = vec![state_id(pool, project_id, "ready").await];

    let mut mutation = TrackerMutation::begin(pool, project_id, TaskActor::Session { session_id })
        .await
        .expect("the mutation opens");
    let task = locked_task(&mut mutation, project_id, task_id).await;
    let claimed = claim_for_profile(&mut mutation, &task, session_id, &served).await;

    finish(mutation, claimed)
        .await
        .0
        .expect("the claim succeeds")
}

/// Release as the MCP `release` tool would: its own mutation, committed only
/// when the release succeeded.
async fn agent_release(
    pool: &PgPool,
    project_id: Uuid,
    task_id: Uuid,
    session_id: Uuid,
    reason: &str,
) -> (Result<TaskDto>, Vec<Escalation>) {
    let mut mutation = TrackerMutation::begin(pool, project_id, TaskActor::Session { session_id })
        .await
        .expect("the mutation opens");
    let task = locked_task(&mut mutation, project_id, task_id).await;
    let released = release_by_agent(&mut mutation, &task, session_id, reason).await;

    finish(mutation, released).await
}

/// The same, for the MCP `needs_human` tool.
async fn hand_to_human(
    pool: &PgPool,
    project_id: Uuid,
    task_id: Uuid,
    session_id: Uuid,
    reason: &str,
) -> (Result<TaskDto>, Vec<Escalation>) {
    let mut mutation = TrackerMutation::begin(pool, project_id, TaskActor::Session { session_id })
        .await
        .expect("the mutation opens");
    let task = locked_task(&mut mutation, project_id, task_id).await;
    let handed = needs_human(&mut mutation, &task, session_id, reason).await;

    finish(mutation, handed).await
}

/// The task row under the mutation's lock.
async fn locked_task(m: &mut TrackerMutation<'_>, project_id: Uuid, task_id: Uuid) -> Task {
    let pool = m.pool();
    TaskRepository::new(pool)
        .find_task_for_update(m.conn(), project_id, task_id.into())
        .await
        .expect("the row reads")
        .expect("the task is in this project")
}

/// Commit exactly when the operation succeeded, roll back otherwise, and hand
/// back the emails the commit made due.
async fn finish(
    mutation: TrackerMutation<'_>,
    outcome: Result<TaskDto>,
) -> (Result<TaskDto>, Vec<Escalation>) {
    match outcome {
        Ok(dto) => {
            let committed = mutation.commit().await.expect("the mutation commits");
            (Ok(dto), committed.escalations)
        }
        Err(error) => {
            mutation.no_change().await.expect("the mutation rolls back");
            (Err(error), Vec::new())
        }
    }
}

/// Put a lease and an attempt count on a task without claiming it, so that
/// "this is the last attempt" is a precondition rather than three more calls.
async fn hold_with_attempts(
    pool: &PgPool,
    project_id: Uuid,
    task_id: Uuid,
    session_id: Uuid,
    attempts: i16,
) {
    let mut mutation = TrackerMutation::begin(pool, project_id, TaskActor::System)
        .await
        .expect("the mutation opens");
    TaskRepository::new(pool)
        .set_task_state_fields(
            mutation.conn(),
            project_id,
            task_id,
            &StateFields {
                lease: Some(Some((session_id, Utc::now()))),
                attempts: Some(attempts),
                ..StateFields::default()
            },
        )
        .await
        .expect("the lease writes");
    mutation.commit().await.expect("the mutation commits");
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

/// The events about one task, which is what a two-task mutation is read by.
async fn events_for(pool: &PgPool, project_id: Uuid, task_id: Uuid) -> Vec<TaskEvent> {
    events(pool, project_id)
        .await
        .into_iter()
        .filter(|event| event.task_id == Some(task_id))
        .collect()
}

/// The kinds of a stream, which is most of what these tests assert.
fn kinds(written: &[TaskEvent]) -> Vec<TaskEventKind> {
    written.iter().map(|event| event.kind).collect()
}

/// The task's comments, oldest first.
async fn comments(pool: &PgPool, project_id: Uuid, task_id: Uuid) -> Vec<TaskComment> {
    TaskRepository::new(pool)
        .list_comments(project_id, task_id)
        .await
        .expect("the comments read")
}

/// How many sessions are linked to this task.
async fn link_count(pool: &PgPool, task_id: Uuid) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM task_sessions WHERE task_id = $1")
        .bind(task_id)
        .fetch_one(pool)
        .await
        .expect("the count runs")
}

/// The conflict message, or a failure naming what came back instead.
fn conflict(error: Error) -> String {
    match error {
        Error::Conflict(message) => message,
        other => panic!("expected a conflict, got {other:?}"),
    }
}

#[tokio::test]
async fn three_agent_releases_hand_the_task_to_a_person() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;

    let subject = task(&pool, project_id, "implement it", "ready").await;

    let mut escalations = Vec::new();
    for attempt in 1..=MAX_ATTEMPTS {
        let session_id = seed_session(&pool, project_id, fixture.profile_id).await;
        let claimed = claim(&pool, project_id, subject.id, session_id).await;
        assert_eq!(claimed.attempts, attempt);

        let (released, owed) = agent_release(
            &pool,
            project_id,
            subject.id,
            session_id,
            &format!("attempt {attempt} failed"),
        )
        .await;
        let released = released.expect("the release succeeds");
        escalations = owed;

        if attempt < MAX_ATTEMPTS {
            assert_eq!(
                released.state, "ready",
                "below the limit the queue keeps it"
            );
            assert_eq!(released.attempts, attempt, "a release keeps the counter");
            assert!(released.lease_holder_session_id.is_none());
            assert!(escalations.is_empty(), "no email below the limit");
        }
    }

    // The third release ran out of attempts, so the task is a person's now.
    let stored = read(&pool, project_id, subject.id).await;
    assert_eq!(stored.state, "needs_human");
    assert!(stored.lease_holder_session_id.is_none());
    assert_eq!(stored.attempts, 0, "the move resets the counter");
    let reason = stored
        .needs_human_reason
        .as_deref()
        .expect("the reason is recorded");
    assert!(
        reason.starts_with("attempt limit reached (3/3):"),
        "unexpected reason {reason:?}",
    );
    assert!(reason.ends_with("attempt 3 failed"), "and the last reason");

    // Three agents said what went wrong, and the orchestrator said why it
    // stopped asking.
    let written_comments = comments(&pool, project_id, subject.id).await;
    assert_eq!(written_comments.len(), 4);
    // The agents', in order; the escalation's shares its transaction's
    // timestamp with the third one, so it is read out by its flag, not by its
    // position.
    let agent_comments = written_comments
        .iter()
        .filter(|comment| !comment.system)
        .collect::<Vec<_>>();
    assert_eq!(agent_comments.len(), 3);
    for (index, comment) in agent_comments.iter().enumerate() {
        assert!(comment.author_session_id.is_some());
        assert_eq!(comment.body, format!("attempt {} failed", index + 1));
    }
    let system_comment = written_comments
        .iter()
        .find(|comment| comment.system)
        .expect("the escalation says why in the thread");
    assert!(system_comment.author_session_id.is_none());
    assert!(system_comment.author_user_id.is_none());
    assert!(
        system_comment
            .body
            .starts_with("Escalated to needs_human after 3 attempts."),
        "unexpected comment {:?}",
        system_comment.body,
    );

    let written = events_for(&pool, project_id, subject.id).await;
    assert_eq!(
        kinds(&written),
        vec![
            TaskEventKind::Claimed,
            TaskEventKind::Commented,
            TaskEventKind::Released,
            TaskEventKind::Claimed,
            TaskEventKind::Commented,
            TaskEventKind::Released,
            TaskEventKind::Claimed,
            TaskEventKind::Commented,
            TaskEventKind::Commented,
            TaskEventKind::Escalated,
        ],
    );
    assert_eq!(written[2].reason.as_deref(), Some("given_back"));
    assert_eq!(written[5].reason.as_deref(), Some("given_back"));

    let escalated = written.last().expect("the stream is not empty");
    assert_eq!(escalated.from.as_deref(), Some("ready"));
    assert_eq!(escalated.to.as_deref(), Some("needs_human"));
    assert_eq!(escalated.reason.as_deref(), Some(reason));
    // The system comment is the orchestrator's, whoever's mutation wrote it.
    assert_eq!(written[8].actor, TaskActor::System);

    assert_eq!(escalations.len(), 1, "one email is owed");
    assert_eq!(
        escalations[0],
        Escalation {
            project_id,
            project_name: escalations[0].project_name.clone(),
            task_id: subject.id,
            task_number: subject.number,
            task_title: subject.title.clone(),
            assignee_user_id: None,
            reason: reason.to_string(),
        },
    );

    assert_eq!(link_count(&pool, subject.id).await, 3, "three workers");
}

#[tokio::test]
async fn a_release_by_a_session_that_does_not_hold_the_task_writes_nothing() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let holder = seed_session(&pool, project_id, fixture.profile_id).await;
    let other = seed_session(&pool, project_id, fixture.profile_id).await;

    let subject = task(&pool, project_id, "implement it", "ready").await;
    claim(&pool, project_id, subject.id, holder).await;
    let before = events(&pool, project_id).await.len();

    let (refused, escalations) =
        agent_release(&pool, project_id, subject.id, other, "not mine").await;
    assert_eq!(
        conflict(refused.expect_err("the release is refused")),
        NOT_HELD_BY_SESSION,
    );
    assert!(escalations.is_empty());

    // A rejection writes no history: no comment, no event, no lease change.
    let stored = read(&pool, project_id, subject.id).await;
    assert_eq!(stored.lease_holder_session_id, Some(holder));
    assert!(comments(&pool, project_id, subject.id).await.is_empty());
    assert_eq!(events(&pool, project_id).await.len(), before);
}

#[tokio::test]
async fn a_dead_session_releases_what_it_held_and_escalates_what_ran_out() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let session_id = seed_session(&pool, project_id, fixture.profile_id).await;

    let ongoing = task(&pool, project_id, "still has attempts", "ready").await;
    let exhausted = task(&pool, project_id, "out of attempts", "ready").await;
    hold_with_attempts(&pool, project_id, ongoing.id, session_id, 1).await;
    hold_with_attempts(&pool, project_id, exhausted.id, session_id, MAX_ATTEMPTS).await;

    let escalations = release_leases_for_session(&pool, session_id, ReleaseReason::SessionEnded)
        .await
        .expect("the release runs");

    // The one with attempts left went back to its queue.
    let stored = read(&pool, project_id, ongoing.id).await;
    assert_eq!(stored.state, "ready");
    assert_eq!(stored.attempts, 1);
    assert!(stored.lease_holder_session_id.is_none());

    let written = events_for(&pool, project_id, ongoing.id).await;
    assert_eq!(
        kinds(&written),
        vec![TaskEventKind::Commented, TaskEventKind::Released],
    );
    assert_eq!(written[1].reason.as_deref(), Some("session_ended"));
    assert!(written.iter().all(|event| event.actor == TaskActor::System));

    let written_comments = comments(&pool, project_id, ongoing.id).await;
    assert_eq!(written_comments.len(), 1);
    assert!(written_comments[0].system);
    assert_eq!(
        written_comments[0].body,
        format!("Lease released by the orchestrator: holder session {session_id} ended."),
    );

    // The one that ran out went to a person instead.
    let stored = read(&pool, project_id, exhausted.id).await;
    assert_eq!(stored.state, "needs_human");
    assert_eq!(stored.attempts, 0);
    assert!(stored.lease_holder_session_id.is_none());
    let reason = stored
        .needs_human_reason
        .as_deref()
        .expect("the reason is recorded");
    assert_eq!(
        reason,
        format!("attempt limit reached (3/3): session {session_id} ended"),
    );

    let written = events_for(&pool, project_id, exhausted.id).await;
    assert_eq!(
        kinds(&written),
        vec![
            TaskEventKind::Commented,
            TaskEventKind::Commented,
            TaskEventKind::Escalated,
        ],
    );
    assert_eq!(written[2].from.as_deref(), Some("ready"));
    assert_eq!(written[2].to.as_deref(), Some("needs_human"));
    assert!(written.iter().all(|event| event.actor == TaskActor::System));

    assert_eq!(
        escalations.len(),
        1,
        "only the escalated task owes an email"
    );
    assert_eq!(escalations[0].task_id, exhausted.id);
    assert_eq!(escalations[0].reason, reason);

    // The orchestrator releasing a lease is not a session having worked on
    // the task (ADR 0030): the claim would have written the link, and these
    // leases were never claimed.
    assert_eq!(link_count(&pool, ongoing.id).await, 0);
    assert_eq!(link_count(&pool, exhausted.id).await, 0);
}

#[tokio::test]
async fn a_stalled_holder_says_stalled() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let session_id = seed_session(&pool, project_id, fixture.profile_id).await;

    let subject = task(&pool, project_id, "implement it", "ready").await;
    hold_with_attempts(&pool, project_id, subject.id, session_id, 1).await;

    let escalations = release_leases_for_session(&pool, session_id, ReleaseReason::Stalled)
        .await
        .expect("the release runs");
    assert!(escalations.is_empty());

    let written = events_for(&pool, project_id, subject.id).await;
    assert_eq!(
        kinds(&written),
        vec![TaskEventKind::Commented, TaskEventKind::Released],
    );
    assert_eq!(written[1].reason.as_deref(), Some("stalled"));

    let written_comments = comments(&pool, project_id, subject.id).await;
    assert_eq!(
        written_comments[0].body,
        format!("Lease released by the orchestrator: holder session {session_id} stalled."),
    );
}

#[tokio::test]
async fn a_session_holding_nothing_writes_nothing() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let session_id = seed_session(&pool, project_id, fixture.profile_id).await;

    let subject = task(&pool, project_id, "nobody holds it", "ready").await;
    let before = events(&pool, project_id).await.len();

    let escalations = release_leases_for_session(&pool, session_id, ReleaseReason::SessionEnded)
        .await
        .expect("the release runs");

    assert!(escalations.is_empty());
    assert_eq!(events(&pool, project_id).await.len(), before);
    assert!(comments(&pool, project_id, subject.id).await.is_empty());
}

#[tokio::test]
async fn needs_human_escalates_whatever_the_counter_says() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let session_id = seed_session(&pool, project_id, fixture.profile_id).await;

    let subject = task(&pool, project_id, "the spec is ambiguous", "ready").await;
    claim(&pool, project_id, subject.id, session_id).await;

    let (handed, escalations) = hand_to_human(
        &pool,
        project_id,
        subject.id,
        session_id,
        "the spec contradicts itself",
    )
    .await;
    let handed = handed.expect("the hand-off succeeds");

    assert_eq!(handed.state, "needs_human");
    assert_eq!(handed.attempts, 0);
    assert!(handed.lease_holder_session_id.is_none());
    assert_eq!(
        handed.needs_human_reason.as_deref(),
        Some("the spec contradicts itself"),
    );

    let written = events_for(&pool, project_id, subject.id).await;
    assert_eq!(
        kinds(&written),
        vec![
            TaskEventKind::Claimed,
            TaskEventKind::Commented,
            TaskEventKind::Escalated,
        ],
    );
    assert_eq!(written[2].from.as_deref(), Some("ready"));
    assert_eq!(written[2].to.as_deref(), Some("needs_human"));
    assert_eq!(
        written[2].reason.as_deref(),
        Some("the spec contradicts itself"),
    );

    // Only the agent's own comment: `needs_human` is not an attempt limit.
    let written_comments = comments(&pool, project_id, subject.id).await;
    assert_eq!(written_comments.len(), 1);
    assert_eq!(written_comments[0].author_session_id, Some(session_id));

    assert_eq!(escalations.len(), 1);
    assert_eq!(escalations[0].task_id, subject.id);
}

#[tokio::test]
async fn needs_human_on_a_task_already_waiting_records_the_reason_and_releases() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let session_id = seed_session(&pool, project_id, fixture.profile_id).await;

    let subject = task(&pool, project_id, "already waiting", "needs_human").await;
    hold_with_attempts(&pool, project_id, subject.id, session_id, 2).await;

    let (handed, escalations) = hand_to_human(
        &pool,
        project_id,
        subject.id,
        session_id,
        "still need a decision",
    )
    .await;
    let handed = handed.expect("the hand-off succeeds");

    assert_eq!(handed.state, "needs_human");
    assert_eq!(handed.attempts, 2, "an unmoved task keeps its counter");
    assert!(handed.lease_holder_session_id.is_none());
    assert_eq!(
        handed.needs_human_reason.as_deref(),
        Some("still need a decision"),
    );
    assert_eq!(read(&pool, project_id, subject.id).await, handed);

    let written = events_for(&pool, project_id, subject.id).await;
    assert_eq!(
        kinds(&written),
        vec![
            TaskEventKind::Commented,
            TaskEventKind::Updated,
            TaskEventKind::Released,
        ],
        "no second escalation",
    );
    assert_eq!(written[2].reason.as_deref(), Some("given_back"));
    assert!(escalations.is_empty(), "and no second email");
}

#[tokio::test]
async fn needs_human_takes_an_unheld_task_and_links_the_session() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let session_id = seed_session(&pool, project_id, fixture.profile_id).await;

    let subject = task(&pool, project_id, "nobody holds it", "ready").await;

    let (handed, escalations) =
        hand_to_human(&pool, project_id, subject.id, session_id, "needs a person").await;
    let handed = handed.expect("an unheld task may be handed over");

    assert_eq!(handed.state, "needs_human");
    assert_eq!(handed.needs_human_reason.as_deref(), Some("needs a person"));

    let written = events_for(&pool, project_id, subject.id).await;
    assert_eq!(
        kinds(&written),
        vec![TaskEventKind::Commented, TaskEventKind::Escalated],
    );
    assert_eq!(escalations.len(), 1);

    // The session never claimed it, so this is what links the two.
    assert_eq!(link_count(&pool, subject.id).await, 1);
}

#[tokio::test]
async fn needs_human_on_somebody_elses_task_writes_nothing() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let holder = seed_session(&pool, project_id, fixture.profile_id).await;
    let other = seed_session(&pool, project_id, fixture.profile_id).await;

    let subject = task(&pool, project_id, "implement it", "ready").await;
    claim(&pool, project_id, subject.id, holder).await;
    let before = events(&pool, project_id).await.len();

    let (refused, escalations) =
        hand_to_human(&pool, project_id, subject.id, other, "not mine").await;
    assert_eq!(
        conflict(refused.expect_err("the hand-off is refused")),
        HELD_BY_ANOTHER,
    );

    assert!(escalations.is_empty());
    assert_eq!(
        read(&pool, project_id, subject.id)
            .await
            .lease_holder_session_id,
        Some(holder),
    );
    assert!(comments(&pool, project_id, subject.id).await.is_empty());
    assert_eq!(events(&pool, project_id).await.len(), before);
}

#[tokio::test]
async fn a_release_at_the_limit_on_a_task_already_waiting_does_not_escalate_twice() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let session_id = seed_session(&pool, project_id, fixture.profile_id).await;

    // A user launched an agent on an escalated task and it failed again.
    let subject = task(&pool, project_id, "already waiting", "needs_human").await;
    hold_with_attempts(&pool, project_id, subject.id, session_id, MAX_ATTEMPTS).await;

    let (released, escalations) =
        agent_release(&pool, project_id, subject.id, session_id, "failed again").await;
    let released = released.expect("the release succeeds");

    assert_eq!(released.state, "needs_human");
    assert_eq!(released.attempts, MAX_ATTEMPTS, "no move, no reset");
    assert!(released.lease_holder_session_id.is_none());
    let reason = released
        .needs_human_reason
        .as_deref()
        .expect("the reason is recorded");
    assert_eq!(reason, "attempt limit reached (3/3): failed again");

    let written = events_for(&pool, project_id, subject.id).await;
    assert_eq!(
        kinds(&written),
        vec![
            TaskEventKind::Commented,
            TaskEventKind::Updated,
            TaskEventKind::Released,
        ],
    );
    assert_eq!(written[2].reason.as_deref(), Some("given_back"));

    // The agent's reason, and no "Escalated to ..." comment.
    let written_comments = comments(&pool, project_id, subject.id).await;
    assert_eq!(written_comments.len(), 1);
    assert_eq!(written_comments[0].body, "failed again");

    assert!(escalations.is_empty(), "the person was already told");
}
