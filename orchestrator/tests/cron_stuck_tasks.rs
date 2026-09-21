//! The stuck-task reaper against a real Postgres (`CLAUDE.md`, "Testing
//! expectations").
//!
//! `ARCHITECTURE.md`, "Background jobs" and "Task tracker" → "Liveness comes
//! from the session, not from tool calls": every minute, every lease whose
//! holder is `done` or `failed` goes back — to its queue, or to a person when
//! the task has run out of attempts. The scenarios are what that sentence
//! promises:
//!
//! - a `done` holder's lease is cleared with a `released` event carrying
//!   `session_ended`, actor `system`, and a system comment naming the holder;
//! - a holder the idle reaper failed with `stalled` says `stalled` instead,
//!   and nothing else differs;
//! - a `parked` or `running` holder is alive, so its leases are not touched;
//! - a task at the project's `max_attempts` moves to the human state instead,
//!   with `attempts` reset, `needs_human_reason` recorded, an `escalated`
//!   event, a second system comment and exactly one email to the assignee;
//! - an unassigned escalation emails every administrator who wants email and
//!   nobody else;
//! - the sweep counts what it released, notifies once per transaction, and a
//!   second run does nothing at all.
//!
//! The job is driven directly through `TestApp::cron()`, with the sessions put
//! into their states through `SessionRepository::transition` and the leases
//! taken through the tracker's own claim: what is under test here is the
//! sweep, not the release primitive, which `tests/tracker_escalation.rs`
//! covers.
//!
//! Needs a container engine; see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use std::time::Duration;

use chrono::Utc;
use common::TestApp;
use mars_orchestrator::cron::JobReport;
use mars_orchestrator::events::{TaskActor, TaskEvent, TaskEventKind};
use mars_orchestrator::models::{
    NewSession, NewTask, ProfileKind, SessionState, Task, TaskComment, User,
};
use mars_orchestrator::repositories::{SessionRepository, TaskRepository, Transition};
use mars_orchestrator::tracker::leases::claim_for_profile;
use mars_orchestrator::tracker::{TaskDto, TrackerMutation};
use sqlx::postgres::PgListener;
use tokio::time::timeout;
use uuid::Uuid;

/// Not a real image: the stub the session tests replay a fixture transcript
/// with.
const TEST_IMAGE: &str = "localhost/mars-session-stub:test";

/// The project default this suite relies on (`SPEC.md`, "Projects").
const MAX_ATTEMPTS: i16 = 3;

/// The `sessions.error` the idle reaper writes (`ARCHITECTURE.md`, "Task
/// tracker").
const STALLED: &str = "stalled";

/// How long a committed notification is waited for before the test fails.
const NOTIFY_WITHIN: Duration = Duration::from_secs(5);

/// How long the channel is given to prove itself silent.
const SILENT_FOR: Duration = Duration::from_millis(300);

/// A project with the documented default states and a profile to hang
/// sessions off.
struct Fixture {
    project_id: Uuid,
    profile_id: Uuid,
}

async fn seed(app: &TestApp) -> Fixture {
    let pool = &app.pool;

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
    assert_eq!(
        mutation.project().max_attempts,
        MAX_ATTEMPTS,
        "this suite assumes the documented default",
    );
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

/// A session of this project, moved into `state` the way the lifecycle moves
/// it; `error` is what `sessions.error` carries when that state is `failed`.
async fn session_in(app: &TestApp, fixture: &Fixture, state: SessionState, error: &str) -> Uuid {
    let new = NewSession::new(
        fixture.project_id,
        fixture.profile_id,
        ProfileKind::Ephemeral,
        "main",
        // Not a credential: a fake stand-in for the hashed MCP token (rule 3).
        format!("fake-mcp-token-hash-{}", Uuid::new_v4()),
    );

    let mut tx = app.pool.begin().await.expect("a transaction begins");
    let session = SessionRepository::new(&app.pool)
        .insert(&mut tx, &new)
        .await
        .expect("the session inserts");
    tx.commit().await.expect("the transaction commits");

    // Every state this suite wants is reached through `running`, which is the
    // only way a real session gets there (`ARCHITECTURE.md`, "Session
    // lifecycle").
    transition(
        app,
        session.id,
        SessionState::Creating,
        SessionState::Running,
    )
    .await;
    if state != SessionState::Running {
        let mut change = Transition::new(SessionState::Running, state, "for the test");
        if state == SessionState::Failed {
            change = change.with_error(error);
        }
        apply(app, session.id, &change).await;
    }

    session.id
}

async fn transition(app: &TestApp, id: Uuid, from: SessionState, to: SessionState) {
    apply(app, id, &Transition::new(from, to, "for the test")).await;
}

async fn apply(app: &TestApp, id: Uuid, change: &Transition<'_>) {
    let mut tx = app.pool.begin().await.expect("a transaction begins");
    SessionRepository::new(&app.pool)
        .transition(&mut tx, id, change)
        .await
        .expect("the session transitions");
    tx.commit().await.expect("the transaction commits");
}

/// A `ready` task of this project, assigned to `assignee` when there is one.
async fn task(app: &TestApp, fixture: &Fixture, title: &str, assignee: Option<&User>) -> Task {
    let project_id = fixture.project_id;
    let mut new = NewTask::new(project_id, title).expect("the title parses");
    new.state_id = Some(state_id(app, project_id, "ready").await);
    new.assignee_user_id = assignee.map(|user| user.id);

    let mut mutation = TrackerMutation::begin(&app.pool, project_id, TaskActor::System)
        .await
        .expect("the mutation opens");
    let inserted = TaskRepository::new(&app.pool)
        .insert_task(mutation.conn(), project_id, &new)
        .await
        .expect("the task inserts");
    mutation.commit().await.expect("the mutation commits");

    inserted
}

/// The id of a project's state by name.
async fn state_id(app: &TestApp, project_id: Uuid, name: &str) -> Uuid {
    TaskRepository::new(&app.pool)
        .find_state_by_name(project_id, name)
        .await
        .expect("the state reads")
        .expect("the project has this state")
        .id
}

/// Claim as an agent would, through the tool's own path. `attempts` becomes 1.
async fn claim(app: &TestApp, fixture: &Fixture, task_id: Uuid, session_id: Uuid) {
    let project_id = fixture.project_id;
    let served = vec![state_id(app, project_id, "ready").await];

    let mut mutation =
        TrackerMutation::begin(&app.pool, project_id, TaskActor::Session { session_id })
            .await
            .expect("the mutation opens");
    let locked = TaskRepository::new(&app.pool)
        .find_task_for_update(mutation.conn(), project_id, task_id.into())
        .await
        .expect("the row reads")
        .expect("the task is in this project");
    claim_for_profile(&mut mutation, &locked, session_id, &served)
        .await
        .expect("the claim succeeds");
    mutation.commit().await.expect("the mutation commits");
}

/// The same lease, but with `attempts` already at the limit, so that "this is
/// the last attempt" is a precondition rather than three more sessions.
async fn claim_at_the_limit(app: &TestApp, fixture: &Fixture, task_id: Uuid, session_id: Uuid) {
    common::tracker::hold_with_attempts(
        &app.pool,
        fixture.project_id,
        task_id,
        session_id,
        MAX_ATTEMPTS,
    )
    .await;
}

/// Run the job with the clock the caller chose.
async fn reap(app: &TestApp) -> JobReport {
    app.cron()
        .stuck_task_reaper(Utc::now())
        .await
        .expect("the sweep runs")
}

/// The task as it is committed.
async fn read(app: &TestApp, project_id: Uuid, task_id: Uuid) -> TaskDto {
    TaskRepository::new(&app.pool)
        .load_task_dto(project_id, task_id)
        .await
        .expect("the task reads")
        .expect("the task is in this project")
}

/// Every committed event of the project, oldest first.
/// Where the stream stands right now.
///
/// The cursor an assertion about "what this call emitted" starts from: a
/// lease is a claim's and `attempts` is what claims left behind
/// (`common::tracker`), so an arrangement really writes `claimed` and
/// `released` events of its own.
async fn since(app: &TestApp, project_id: Uuid) -> i64 {
    TaskRepository::new(&app.pool)
        .max_task_event_seq(project_id)
        .await
        .expect("the cursor reads")
}

async fn events(app: &TestApp, project_id: Uuid, after: i64) -> Vec<TaskEvent> {
    TaskRepository::new(&app.pool)
        .list_task_events_after(project_id, after, 200)
        .await
        .expect("the events read")
        .into_iter()
        .map(|row| TaskEvent::from_row(row).expect("the row is a documented event"))
        .collect()
}

/// The events about one task.
async fn events_for(app: &TestApp, project_id: Uuid, task_id: Uuid, after: i64) -> Vec<TaskEvent> {
    events(app, project_id, after)
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
async fn comments(app: &TestApp, project_id: Uuid, task_id: Uuid) -> Vec<TaskComment> {
    TaskRepository::new(&app.pool)
        .list_comments(project_id, task_id)
        .await
        .expect("the comments read")
}

/// The system comments of a task, which is all the reaper writes.
async fn system_comments(app: &TestApp, project_id: Uuid, task_id: Uuid) -> Vec<String> {
    comments(app, project_id, task_id)
        .await
        .into_iter()
        .filter(|comment| {
            // Both author columns are NULL on a system comment
            // (`docs/data-model.md`, `task_comments`).
            assert!(comment.author_user_id.is_none());
            assert!(comment.author_session_id.is_none());
            comment.system
        })
        .map(|comment| comment.body)
        .collect()
}

/// Turn a user's `notify_email` off, as `PATCH /users/me` would.
async fn opt_out(app: &TestApp, user: &User) {
    sqlx::query("UPDATE users SET notify_email = FALSE WHERE id = $1")
        .bind(user.id)
        .execute(&app.pool)
        .await
        .expect("the opt-out writes");
}

#[tokio::test]
async fn a_done_holders_lease_goes_back_to_its_queue() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;

    let subject = task(&app, &fixture, "implement it", None).await;
    let session_id = session_in(&app, &fixture, SessionState::Done, "").await;
    claim(&app, &fixture, subject.id, session_id).await;

    let report = reap(&app).await;
    assert_eq!(
        report,
        JobReport {
            items: 1,
            skipped: 0,
            failures: 0,
        },
    );

    let stored = read(&app, fixture.project_id, subject.id).await;
    assert!(
        stored.lease_holder_session_id.is_none(),
        "the lease is back"
    );
    assert_eq!(stored.state, "ready", "a release keeps the state");
    assert_eq!(stored.attempts, 1, "and the counter");

    let written = events_for(&app, fixture.project_id, subject.id, 0).await;
    assert_eq!(
        kinds(&written),
        vec![
            TaskEventKind::Claimed,
            TaskEventKind::Commented,
            TaskEventKind::Released,
        ],
    );
    let released = written.last().expect("the stream is not empty");
    assert_eq!(released.reason.as_deref(), Some("session_ended"));
    assert_eq!(released.actor, TaskActor::System);

    assert_eq!(
        system_comments(&app, fixture.project_id, subject.id).await,
        vec![format!(
            "Lease released by the orchestrator: holder session {session_id} ended."
        )],
    );

    assert!(
        app.mock_email().sent().is_empty(),
        "an ordinary release tells nobody",
    );
}

#[tokio::test]
async fn a_stalled_holders_lease_says_stalled() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;

    let subject = task(&app, &fixture, "implement it", None).await;
    let session_id = session_in(&app, &fixture, SessionState::Failed, STALLED).await;
    claim(&app, &fixture, subject.id, session_id).await;

    assert_eq!(reap(&app).await.items, 1);

    let stored = read(&app, fixture.project_id, subject.id).await;
    assert!(stored.lease_holder_session_id.is_none());
    assert_eq!(stored.state, "ready");

    let written = events_for(&app, fixture.project_id, subject.id, 0).await;
    let released = written.last().expect("the stream is not empty");
    assert_eq!(released.kind, TaskEventKind::Released);
    assert_eq!(released.reason.as_deref(), Some("stalled"));
    assert_eq!(released.actor, TaskActor::System);

    assert_eq!(
        system_comments(&app, fixture.project_id, subject.id).await,
        vec![format!(
            "Lease released by the orchestrator: holder session {session_id} stalled."
        )],
    );
}

#[tokio::test]
async fn a_live_holder_keeps_its_leases() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;

    let parked_task = task(&app, &fixture, "the planner is talking", None).await;
    let parked = session_in(&app, &fixture, SessionState::Parked, "").await;
    claim(&app, &fixture, parked_task.id, parked).await;

    let running_task = task(&app, &fixture, "the implementer is working", None).await;
    let running = session_in(&app, &fixture, SessionState::Running, "").await;
    claim(&app, &fixture, running_task.id, running).await;

    assert_eq!(reap(&app).await, JobReport::default());

    for (task_id, holder) in [(parked_task.id, parked), (running_task.id, running)] {
        let stored = read(&app, fixture.project_id, task_id).await;
        assert_eq!(stored.lease_holder_session_id, Some(holder));
        assert_eq!(
            kinds(&events_for(&app, fixture.project_id, task_id, 0).await),
            vec![TaskEventKind::Claimed],
            "a live holder's task hears nothing",
        );
        assert!(
            system_comments(&app, fixture.project_id, task_id)
                .await
                .is_empty(),
        );
    }
}

#[tokio::test]
async fn a_task_at_the_attempt_limit_goes_to_its_assignee() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;

    let assignee = app
        .insert_user("assignee", "assignee@example.test", false, false)
        .await;
    let subject = task(&app, &fixture, "nobody can build it", Some(&assignee)).await;
    let session_id = session_in(&app, &fixture, SessionState::Done, "").await;
    claim_at_the_limit(&app, &fixture, subject.id, session_id).await;
    // The three claims that raised the counter are the arrangement; what
    // follows is the reaper's own.
    let after = since(&app, fixture.project_id).await;

    assert_eq!(reap(&app).await.items, 1);

    let stored = read(&app, fixture.project_id, subject.id).await;
    assert_eq!(stored.state, "needs_human");
    assert!(stored.lease_holder_session_id.is_none());
    assert_eq!(stored.attempts, 0, "the move resets the counter");
    assert_eq!(
        stored.needs_human_reason.as_deref(),
        Some(format!("attempt limit reached (3/3): session {session_id} ended").as_str()),
    );

    // The reaper's own events, past the claims that raised the counter.
    let written = events_for(&app, fixture.project_id, subject.id, after).await;
    assert_eq!(
        kinds(&written),
        vec![
            TaskEventKind::Commented,
            TaskEventKind::Commented,
            TaskEventKind::Escalated,
        ],
    );
    let escalated = written.last().expect("the stream is not empty");
    assert_eq!(escalated.from.as_deref(), Some("ready"));
    assert_eq!(escalated.to.as_deref(), Some("needs_human"));
    assert_eq!(
        escalated.reason.as_deref(),
        stored.needs_human_reason.as_deref(),
    );
    assert_eq!(escalated.actor, TaskActor::System);

    // Both comments are written in the one transaction and share its
    // timestamp, so the thread is read out as a set rather than in order.
    let mut written_comments = system_comments(&app, fixture.project_id, subject.id).await;
    written_comments.sort();
    let mut expected = vec![
        format!("Lease released by the orchestrator: holder session {session_id} ended."),
        format!(
            "Escalated to needs_human after 3 attempts. attempt limit reached (3/3): session {session_id} ended"
        ),
    ];
    expected.sort();
    assert_eq!(written_comments, expected);

    let sent = app.mock_email().sent();
    assert_eq!(sent.len(), 1, "exactly one message: {sent:?}");
    assert_eq!(sent[0].to, assignee.email.as_str());
    assert!(
        sent[0].text.contains("attempt limit reached (3/3)"),
        "the email names the reason: {:?}",
        sent[0].text,
    );
}

#[tokio::test]
async fn an_unassigned_escalation_reaches_the_admins_who_want_email() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;

    let told = app
        .insert_user("admin-told", "told@example.test", true, false)
        .await;
    let quiet = app
        .insert_user("admin-quiet", "quiet@example.test", true, false)
        .await;
    opt_out(&app, &quiet).await;
    // A plain user is not an administrator and is never a fallback recipient.
    app.insert_user("bystander", "bystander@example.test", false, false)
        .await;

    let subject = task(&app, &fixture, "nobody owns it", None).await;
    let session_id = session_in(&app, &fixture, SessionState::Failed, STALLED).await;
    claim_at_the_limit(&app, &fixture, subject.id, session_id).await;

    assert_eq!(reap(&app).await.items, 1);

    assert_eq!(
        read(&app, fixture.project_id, subject.id).await.state,
        "needs_human"
    );

    let sent = app.mock_email().sent();
    assert_eq!(sent.len(), 1, "exactly one message: {sent:?}");
    assert_eq!(sent[0].to, told.email.as_str());
}

#[tokio::test]
async fn the_sweep_counts_what_it_released_and_repeats_nothing() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;

    let assignee = app
        .insert_user("assignee", "assignee@example.test", false, false)
        .await;
    app.insert_user("admin", "admin@example.test", true, false)
        .await;

    // One dead holder with two tasks, one of them out of attempts, and a
    // second dead holder — a stalled one — with the same shape: four releases
    // in two transactions.
    let ended = session_in(&app, &fixture, SessionState::Done, "").await;
    let ended_queue = task(&app, &fixture, "back to the queue", None).await;
    let ended_limit = task(&app, &fixture, "out of attempts", Some(&assignee)).await;
    claim(&app, &fixture, ended_queue.id, ended).await;
    claim_at_the_limit(&app, &fixture, ended_limit.id, ended).await;

    let stalled = session_in(&app, &fixture, SessionState::Failed, STALLED).await;
    let stalled_queue = task(&app, &fixture, "back again", None).await;
    let stalled_limit = task(&app, &fixture, "out of attempts too", None).await;
    claim(&app, &fixture, stalled_queue.id, stalled).await;
    claim_at_the_limit(&app, &fixture, stalled_limit.id, stalled).await;

    // And one live holder, which the sweep must not count.
    let alive = session_in(&app, &fixture, SessionState::Running, "").await;
    let held = task(&app, &fixture, "still being worked on", None).await;
    claim(&app, &fixture, held.id, alive).await;

    let mut listener = PgListener::connect_with(&app.pool)
        .await
        .expect("the listener connects");
    listener
        .listen("task_events")
        .await
        .expect("the channel is listened on");

    let report = reap(&app).await;
    assert_eq!(
        report,
        JobReport {
            items: 4,
            skipped: 0,
            failures: 0,
        },
    );

    // One notification per committed transaction — one per dead holder, all
    // naming this project (ADR 0028).
    for _ in 0..2 {
        let notification = timeout(NOTIFY_WITHIN, listener.recv())
            .await
            .expect("a committed batch notifies")
            .expect("the listener is healthy");
        assert_eq!(notification.channel(), "task_events");
        assert!(
            notification
                .payload()
                .starts_with(&format!("{}:", fixture.project_id)),
            "unexpected payload {:?}",
            notification.payload(),
        );
    }

    for queued in [ended_queue.id, stalled_queue.id] {
        let stored = read(&app, fixture.project_id, queued).await;
        assert_eq!(stored.state, "ready");
        assert!(stored.lease_holder_session_id.is_none());
    }
    for escalated in [ended_limit.id, stalled_limit.id] {
        assert_eq!(
            read(&app, fixture.project_id, escalated).await.state,
            "needs_human"
        );
    }
    assert_eq!(
        read(&app, fixture.project_id, held.id)
            .await
            .lease_holder_session_id,
        Some(alive),
        "the live holder kept its task",
    );

    let after_first = events(&app, fixture.project_id, 0).await;
    let emails_after_first = app.mock_email().sent();
    assert_eq!(emails_after_first.len(), 2, "one per escalated task");

    // Idempotent: nothing is held by a dead session any more, so the second
    // run has nothing to list (`ARCHITECTURE.md`, "Background jobs").
    assert_eq!(reap(&app).await, JobReport::default());

    assert_eq!(
        events(&app, fixture.project_id, 0).await,
        after_first,
        "a second run writes no event",
    );
    assert_eq!(
        app.mock_email().sent(),
        emails_after_first,
        "and sends no second email",
    );
    assert!(
        timeout(SILENT_FOR, listener.recv()).await.is_err(),
        "a second run notifies nobody",
    );
}
