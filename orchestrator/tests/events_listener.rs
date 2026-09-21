//! The shared Postgres listener, end to end (`CLAUDE.md`, "Testing
//! expectations").
//!
//! `tests/repositories_sessions.rs` and `tests/tracker_states.rs`
//! assert that the writers issue their `pg_notify` inside the writing
//! transaction, over a `PgListener` of the test's own. This file asserts the
//! other half — that the orchestrator's *one* listener turns those
//! notifications into [`Notice`]s on the fan-out every WebSocket and SSE
//! subscriber waits on (`ARCHITECTURE.md`, "Event delivery"; ADR 0028's
//! acceptance list).
//!
//! Nothing here opens a `PgListener`: a test observes notifications exactly
//! where a handler does, through `app.state.fanout`. `TestApp::spawn` starts
//! the listener, so what is under test is the wiring a running orchestrator
//! has.
//!
//! The last scenario is the one ADR 0005 makes necessary: notifications issued
//! while the listener is away are lost and "their loss is harmless", provided
//! every live subscriber is told to read from its cursor *now* rather than
//! waiting out the 30-second safety read. It kills the listener's backend and
//! asserts the [`Notice::Resync`] and the delivery that follows it.
//!
//! Every credential-shaped value is an obviously fake stand-in (rule 3).
//!
//! Needs a container engine; see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use std::time::Duration;

use common::TestApp;
use mars_orchestrator::events::{Notice, TaskActor};
use mars_orchestrator::models::{
    NewEvent, NewSession, NewTaskEvent, ProfileKind, SessionState, StateChange, task_event_kind,
};
use mars_orchestrator::repositories::tasks::test_support::TaskRepositoryTestExt;
use mars_orchestrator::repositories::{SessionRepository, TaskRepository};
use mars_orchestrator::tracker::TrackerMutation;
use serde_json::json;
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::TryRecvError;
use tokio::time::timeout;
use uuid::Uuid;

/// Not a credential: an obviously fake stand-in for the Argon2id PHC string
/// the seeded user would carry (rule 3).
const FAKE_PASSWORD_HASH: &str = "$argon2id$fake$hash";

/// How long a notice that is expected is given to arrive.
const WITHIN: Duration = Duration::from_secs(5);

/// How long a notice that must *not* arrive is given to prove it.
const SETTLE: Duration = Duration::from_millis(400);

/// The rows one session needs to exist.
struct Fixture {
    project_id: Uuid,
    session_id: Uuid,
}

/// Seed a user, a project, a profile and a session directly, since this file
/// is about delivery and not about the routes that create them.
async fn seed(app: &TestApp) -> Fixture {
    let pool = &app.pool;

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
         VALUES ($1, $2, 'default', $3, TRUE)",
    )
    .bind(profile_id)
    .bind(project_id)
    .bind("localhost/mars-session:test")
    .execute(pool)
    .await
    .expect("the profile seeds");

    let mut session = NewSession::new(
        project_id,
        profile_id,
        ProfileKind::Conversational,
        "main",
        format!("fake-mcp-token-hash-{}", Uuid::new_v4()),
    );
    session.created_by = Some(user_id);

    let mut tx = pool.begin().await.expect("a transaction begins");
    let session_id = SessionRepository::new(pool)
        .insert(&mut tx, &session)
        .await
        .expect("the session inserts")
        .id;
    tx.commit().await.expect("the transaction commits");

    Fixture {
        project_id,
        session_id,
    }
}

/// One `text` event, the shape a session owner appends per translated line.
fn text_event(text: &str) -> NewEvent {
    NewEvent::now("text", json!({ "text": text }))
}

/// The next notice, or a failure naming what was being waited for.
async fn next_notice(rx: &mut Receiver<Notice>, what: &str) -> Notice {
    timeout(WITHIN, rx.recv())
        .await
        .unwrap_or_else(|_| panic!("{what} did not arrive within {WITHIN:?}"))
        .unwrap_or_else(|err| panic!("{what} could not be received: {err}"))
}

/// Give anything in flight a moment, then assert nothing was published.
async fn assert_silent(rx: &mut Receiver<Notice>, what: &str) {
    tokio::time::sleep(SETTLE).await;
    assert_eq!(rx.try_recv(), Err(TryRecvError::Empty), "{what}");
}

/// Append `events` to `session_id` in one committed transaction.
async fn append(app: &TestApp, session_id: Uuid, events: &[NewEvent]) -> Vec<i64> {
    let mut tx = app.pool.begin().await.expect("a transaction begins");
    let sequences = SessionRepository::new(&app.pool)
        .append_events(&mut tx, session_id, events)
        .await
        .expect("the events append");
    tx.commit().await.expect("the transaction commits");

    sequences
}

#[tokio::test]
async fn a_committed_batch_wakes_the_session_once_with_its_highest_sequence() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;
    let mut rx = app.state.fanout.subscribe_session(fixture.session_id);

    let sequences = append(
        &app,
        fixture.session_id,
        &[text_event("one"), text_event("two"), text_event("three")],
    )
    .await;
    assert_eq!(sequences, vec![1, 2, 3]);

    assert_eq!(
        next_notice(&mut rx, "the batch's notice").await,
        Notice::SessionEvents { seq: 3 },
        "a batch notifies once, with the highest sequence it committed",
    );
    assert_silent(&mut rx, "a batch notified more than once").await;
}

#[tokio::test]
async fn a_rolled_back_batch_wakes_nobody() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;
    let mut rx = app.state.fanout.subscribe_session(fixture.session_id);

    let mut tx = app.pool.begin().await.expect("a transaction begins");
    SessionRepository::new(&app.pool)
        .append_events(&mut tx, fixture.session_id, &[text_event("never")])
        .await
        .expect("the events append");
    tx.rollback().await.expect("the transaction rolls back");

    assert_silent(&mut rx, "a rolled-back batch published a notice").await;

    // And the stream is still at zero, so nothing was committed either.
    let appended = append(&app, fixture.session_id, &[text_event("first")]).await;
    assert_eq!(appended, vec![1]);
    assert_eq!(
        next_notice(&mut rx, "the committed batch's notice").await,
        Notice::SessionEvents { seq: 1 },
    );
}

#[tokio::test]
async fn a_state_change_wakes_the_session_with_the_state_it_moved_to() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;
    let mut rx = app.state.fanout.subscribe_session(fixture.session_id);
    let repository = SessionRepository::new(&app.pool);

    for to in [SessionState::Running, SessionState::Parked] {
        let mut tx = app.pool.begin().await.expect("a transaction begins");
        repository
            .set_state(&mut tx, fixture.session_id, to, &StateChange::plain())
            .await
            .expect("the transition applies");
        tx.commit().await.expect("the transaction commits");

        assert_eq!(
            next_notice(&mut rx, "the state change's notice").await,
            Notice::SessionState { state: to },
        );
    }

    assert_silent(&mut rx, "a state change published more than one notice").await;
}

#[tokio::test]
async fn a_committed_task_event_batch_wakes_the_project_once() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;
    let mut rx = app.state.fanout.subscribe_project(fixture.project_id);

    let repository = TaskRepository::new(&app.pool);
    let mut mutation = TrackerMutation::begin(&app.pool, fixture.project_id, TaskActor::System)
        .await
        .expect("the mutation opens");
    let sequences = repository
        .append_task_events(
            mutation.conn(),
            fixture.project_id,
            &[
                NewTaskEvent::project_wide(
                    task_event_kind::STATES_CHANGED,
                    json!({ "states": [] }),
                ),
                NewTaskEvent::project_wide(
                    task_event_kind::STATES_CHANGED,
                    json!({ "states": [] }),
                ),
            ],
        )
        .await
        .expect("the task events append");
    mutation.commit().await.expect("the mutation commits");
    assert_eq!(sequences, vec![1, 2]);

    assert_eq!(
        next_notice(&mut rx, "the task batch's notice").await,
        Notice::TaskEvents { seq: 2 },
    );
    assert_silent(&mut rx, "a task batch notified more than once").await;
}

#[tokio::test]
async fn a_session_notice_reaches_only_that_session() {
    let app = TestApp::spawn().await;
    let first = seed(&app).await;
    let second = seed(&app).await;

    let mut theirs = app.state.fanout.subscribe_session(second.session_id);
    let mut mine = app.state.fanout.subscribe_session(first.session_id);
    // A project subscriber is a different map entirely, even for this id.
    let mut project = app.state.fanout.subscribe_project(first.session_id);

    append(&app, first.session_id, &[text_event("mine")]).await;

    assert_eq!(
        next_notice(&mut mine, "this session's notice").await,
        Notice::SessionEvents { seq: 1 },
    );
    assert_silent(&mut theirs, "another session's subscriber was woken").await;
    assert_silent(&mut project, "a project subscriber was woken by a session").await;
}

#[tokio::test]
async fn a_lost_connection_broadcasts_a_resync_and_delivery_resumes() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;
    let mut session = app.state.fanout.subscribe_session(fixture.session_id);
    let mut project = app.state.fanout.subscribe_project(fixture.project_id);

    // The listener is listening by the time `spawn` returned, so its backend
    // is there to be killed. Scoped to this test's own database, because every
    // test in the process shares one Postgres (`tests/common/db.rs`).
    let killed: Vec<bool> = sqlx::query_scalar(
        "SELECT pg_terminate_backend(pid) FROM pg_stat_activity
         WHERE pid <> pg_backend_pid()
           AND datname = current_database()
           AND query ILIKE 'LISTEN%'",
    )
    .fetch_all(&app.pool)
    .await
    .expect("the listener's backend can be looked up");
    assert_eq!(
        killed.len(),
        1,
        "exactly one backend in this database is LISTENing",
    );

    // Whether sqlx reconnects underneath (`Ok(None)`) or the read fails and
    // the task reconnects itself, both live subscribers are told to read.
    assert_eq!(
        next_notice(&mut session, "the session subscriber's resync").await,
        Notice::Resync,
    );
    assert_eq!(
        next_notice(&mut project, "the project subscriber's resync").await,
        Notice::Resync,
    );

    // And the reconnected listener is listening again: the next committed
    // batch is delivered like any other.
    append(&app, fixture.session_id, &[text_event("after")]).await;
    assert_eq!(
        next_notice(&mut session, "the notice after the reconnect").await,
        Notice::SessionEvents { seq: 1 },
    );
}
