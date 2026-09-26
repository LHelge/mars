//! The whole delivery path, end to end (`ARCHITECTURE.md`, "Event delivery";
//! ADR 0005, ADR 0022, ADR 0028).
//!
//! Every other stream suite tests one half: `tests/events_listener.rs` the
//! listener, `tests/ws_session_stream.rs` and `tests/sse_task_stream.rs` one
//! handler each, `tests/repositories_*` the writers. This file is the
//! acceptance suite for the path as a whole — repository transaction →
//! `pg_notify` inside it → shared listener → fan-out → WebSocket and SSE
//! handlers → client — in the situations no single-module test can arrange:
//!
//! - many subscribers at once, each with a cursor of its own, while two
//!   writers commit concurrently: "at-least-once delivery to clients with
//!   dedupe on `seq`, at any scale of subscribers" (ADR 0005), asserted here
//!   as the stronger fact the handlers actually give — exactly once, ascending
//!   — because the cursor advances per row;
//! - one *combined* tracker/session transaction observed on both stream kinds
//!   at the same time, which is where "nothing is broadcast before commit" and
//!   the documented lock order — the project row before session row locks
//!   (`ARCHITECTURE.md`, "Task tracker"; `docs/data-model.md`, "Tracker
//!   mutation transactions") — are visible from outside;
//! - the same shape rolled back, which must expose neither the rows nor their
//!   events anywhere;
//! - the listener's connection lost at the worst moment, recovered through the
//!   streams themselves, by `Resync` or by the safety read — the assertion is
//!   the outcome, not which of the two did it;
//! - the client-visible ordering guarantees at a replay page boundary, where a
//!   gap or a duplicate would hide.
//!
//! The periods are `common::TEST_TIMINGS` (200 ms ping, 300 ms safety read,
//! 100 ms SSE keepalive), not the documented tens of seconds, so the whole
//! file runs in well under a minute.
//!
//! Every wait is bounded by `tokio::time::timeout`: a scenario that does not
//! see what it is waiting for fails with what it did see, and never hangs the
//! suite.
//!
//! Fixtures go in through the repositories and the tracker's own
//! `TrackerMutation`, never raw SQL — with the two exceptions this path cannot
//! express: `pg_terminate_backend` (`common::terminate_listener_backend`) and
//! the `projects`, `agent_profiles` rows the other suites seed the same way.
//! Every credential-shaped value is an obviously fake stand-in (rule 3).
//!
//! Needs a container engine; see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use std::collections::BTreeSet;
use std::time::Duration;

use axum_test::{TestWebSocket, WsMessage};
use bytes::Bytes;
use common::sse::{SseReader, collect_sse_ids};
use common::{AuthenticatedUser, TEST_TIMINGS, TestApp, collect_ws_events};
use futures_util::Stream;
use mars_orchestrator::events::{AgentEvent, AgentEventBody, TaskActor};
use mars_orchestrator::models::{NewEvent, NewSession, ProfileKind, SessionState};
use mars_orchestrator::repositories::{SessionRepository, TaskRepository, Transition};
use mars_orchestrator::tracker::TrackerMutation;
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use uuid::Uuid;

/// How long a frame that should already be on its way is waited for.
///
/// Generous against the test timings: every assertion here is "this arrives",
/// never "this arrives quickly", and a scheduling hiccup on a loaded machine
/// must not be a failure.
const WITHIN: Duration = Duration::from_secs(5);

/// How long the busiest scenarios — twenty sockets, a thousand replayed
/// events — are given.
const WITHIN_BULK: Duration = Duration::from_secs(30);

/// An obviously fake password, long enough for `POST /api/test/users`
/// (rule 3).
const PASSWORD: &str = "not-a-real-password";

/// The rows a stream needs: a user to open it as, a project to watch task
/// events on and a session to watch transcript events on.
struct Fixture {
    user: AuthenticatedUser,
    project_id: Uuid,
    session_id: Uuid,
}

/// A signed-in user, a project, a profile and one `parked` session.
///
/// The project and profile go in with unchecked statements, the way the other
/// stream suites seed them: what this file is about starts at the events.
async fn arrange(app: &TestApp) -> Fixture {
    let user = app
        .create_user("watcher", "watcher@example.test", PASSWORD)
        .await;
    let project_id = insert_project(app).await;
    let session_id = insert_session(app, project_id, user.user.id).await;

    Fixture {
        user,
        project_id,
        session_id,
    }
}

/// One project row.
async fn insert_project(app: &TestApp) -> Uuid {
    let project_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id, name, remote_url) VALUES ($1, $2, $3)")
        .bind(project_id)
        .bind(format!("project-{project_id}"))
        // `.invalid` can never resolve (rule 3).
        .bind("https://example.invalid/org/repo.git")
        .execute(&app.pool)
        .await
        .expect("the project seeds");

    project_id
}

/// One profile and one session on `project_id`, parked.
///
/// Straight to `parked`: the lifecycle that gets a session there is not what
/// this file is testing, and `parked → running` is the transition the
/// state-change scenario drives.
async fn insert_session(app: &TestApp, project_id: Uuid, user_id: Uuid) -> Uuid {
    let profile_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO agent_profiles (id, project_id, name, image, partial_messages)
         VALUES ($1, $2, $3, $4, TRUE)",
    )
    .bind(profile_id)
    .bind(project_id)
    .bind(format!("profile-{profile_id}"))
    .bind("localhost/mars-session:test")
    .execute(&app.pool)
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

    let mut tx = app.pool.begin().await.expect("a transaction begins");
    let inserted = SessionRepository::new(&app.pool)
        .insert(&mut tx, &session)
        .await
        .expect("the session inserts");
    tx.commit().await.expect("the transaction commits");

    sqlx::query("UPDATE sessions SET state = 'parked' WHERE id = $1")
        .bind(inserted.id)
        .execute(&app.pool)
        .await
        .expect("the session parks");

    inserted.id
}

/// `count` `text` events appended to `session_id` in one committed
/// transaction, which is one session row lock and one notification.
async fn append_session_events(pool: &PgPool, session_id: Uuid, count: usize) -> Vec<i64> {
    let events: Vec<NewEvent> = (0..count)
        .map(|index| NewEvent::now("text", json!({ "text": format!("line {index}") })))
        .collect();

    let mut tx = pool.begin().await.expect("a transaction begins");
    let sequences = SessionRepository::new(pool)
        .append_events(&mut tx, session_id, &events)
        .await
        .expect("the events append");
    tx.commit().await.expect("the transaction commits");

    sequences
}

/// `count` `deleted` task events appended to `project_id` in one tracker
/// mutation, which is one project row lock and one notification.
///
/// `deleted` throughout, for the reason `tests/sse_task_stream.rs` gives: it
/// is the one kind whose task row is expected to be missing, so it needs no
/// task fixtures.
async fn append_task_events(pool: &PgPool, project_id: Uuid, count: usize) -> Vec<i64> {
    let mut mutation = TrackerMutation::begin(pool, project_id, TaskActor::System)
        .await
        .expect("the mutation begins");

    for _ in 0..count {
        mutation
            .emit_deleted(Uuid::new_v4())
            .expect("the event is emitted");
    }

    mutation.commit().await.expect("the mutation commits").seqs
}

/// The session frame the socket opens with, consumed so the scenario starts
/// from a known point.
async fn expect_session_frame(socket: &mut TestWebSocket) -> Value {
    let frame = tokio::time::timeout(WITHIN, async {
        loop {
            match socket.receive_message().await {
                WsMessage::Text(text) => {
                    return serde_json::from_str::<Value>(text.as_str())
                        .expect("every text frame is JSON");
                }
                WsMessage::Ping(_) | WsMessage::Pong(_) => {}
                other => panic!("expected the session snapshot, got {other:?}"),
            }
        }
    })
    .await
    .expect("the snapshot arrives");

    assert_eq!(frame["type"], "session", "the first frame is the snapshot");

    frame["session"].clone()
}

/// Poll the socket for `during`, failing on anything but a ping or a pong.
///
/// Polled rather than slept through on purpose: a socket that is not read
/// answers no ping and would be closed by the server, which is a different
/// scenario from the silence being asserted here.
async fn assert_ws_silent(socket: &mut TestWebSocket, during: Duration, what: &str) {
    let deadline = tokio::time::Instant::now() + during;

    while let Ok(message) = tokio::time::timeout_at(deadline, socket.receive_message()).await {
        match message {
            WsMessage::Ping(_) | WsMessage::Pong(_) => {}
            other => panic!("{what}: the socket received {other:?}"),
        }
    }
}

/// A socket that a task of its own reads for the whole scenario, so every
/// ping is answered whatever the scenario is doing meanwhile.
///
/// A `TestWebSocket` writes its pongs only while it is being read, and the
/// server closes with 1001 after two pings went unanswered (`SPEC.md`,
/// "WebSocket: session stream"): 600 ms of not reading, with the test
/// timings. A scenario that turns to something else for that long — waits on
/// the task stream, say — has really stopped answering, and the close it then
/// gets is the server being right. Pings and pongs end here; every other
/// message is forwarded in order, a close last.
struct PolledSocket {
    messages: mpsc::UnboundedReceiver<WsMessage>,
    reader: JoinHandle<()>,
}

impl PolledSocket {
    fn spawn(mut socket: TestWebSocket) -> Self {
        let (tx, messages) = mpsc::unbounded_channel();

        let reader = tokio::spawn(async move {
            loop {
                let message = socket.receive_message().await;
                if matches!(message, WsMessage::Ping(_) | WsMessage::Pong(_)) {
                    continue;
                }

                let last = matches!(message, WsMessage::Close(_));
                if tx.send(message).is_err() || last {
                    break;
                }
            }
        });

        Self { messages, reader }
    }

    /// `common::collect_ws_events` over the forwarded messages, and the same
    /// contract: the sequences seen until one reaches `until_seq`, a panic on
    /// anything that is not an `event` or a `session`, and a panic rather than
    /// a hang when `within` runs out.
    async fn events_until(&mut self, until_seq: i64, within: Duration) -> Vec<i64> {
        let mut seen: Vec<i64> = Vec::new();

        let collected = tokio::time::timeout(within, async {
            while seen.iter().copied().max().unwrap_or(0) < until_seq {
                match self.messages.recv().await {
                    Some(WsMessage::Text(text)) => {
                        let frame: Value =
                            serde_json::from_str(text.as_str()).expect("every text frame is JSON");

                        match frame["type"].as_str() {
                            Some("event") => seen.push(
                                frame["event"]["seq"]
                                    .as_i64()
                                    .expect("every event frame carries a sequence"),
                            ),
                            Some("session") => {}
                            _ => panic!("unexpected frame while collecting events: {frame}"),
                        }
                    }
                    other => panic!("the socket ended before sequence {until_seq}: {other:?}"),
                }
            }
        })
        .await;

        collected.unwrap_or_else(|_| {
            panic!("sequence {until_seq} did not arrive within {within:?}; saw {seen:?}")
        });

        seen
    }
}

impl Drop for PolledSocket {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

/// Read the stream for `during`, failing on anything but a comment frame
/// (the opening `: ready` and the periodic `: keepalive`).
async fn assert_sse_silent<S>(reader: &mut SseReader<S>, during: Duration, what: &str)
where
    S: Stream<Item = Bytes> + Unpin,
{
    let deadline = tokio::time::Instant::now() + during;

    while let Ok(Some(frame)) = tokio::time::timeout_at(deadline, reader.next_frame()).await {
        assert!(frame.is_comment(), "{what}: the stream sent {frame:?}");
    }
}

// ---- 1. many subscribers, two writers ----

/// Twenty sockets, twenty different cursors, a hundred events from two
/// concurrent writers: every client sees exactly the sequences above its own
/// cursor, once each, ascending (ADR 0005).
///
/// With two writers and 64-notice channels a subscriber can be told it lagged;
/// the handlers answer a `Lagged` exactly as they answer a notice, by reading
/// from the cursor, so the assertion is unchanged by it.
#[tokio::test]
async fn twenty_sockets_receive_every_event_once() {
    const SOCKETS: usize = 20;
    const EVENTS: i64 = 100;

    let app = TestApp::spawn().await;
    let fixture = arrange(&app).await;

    // Cursors 0, 5, 10, … 95: every one of them below the hundred events the
    // writers are about to commit, so every socket has something to receive.
    let mut collectors = Vec::with_capacity(SOCKETS);
    for index in 0..SOCKETS {
        let after = (index as i64) * 5;
        let mut socket = app
            .ws(fixture.session_id, &fixture.user.access_token, after)
            .await;

        expect_session_frame(&mut socket).await;
        collectors.push(tokio::spawn(async move {
            let seen = collect_ws_events(&mut socket, EVENTS, WITHIN_BULK).await;
            (after, seen)
        }));
    }

    // Batches of 1 to 7, summing to exactly a hundred, dealt alternately to
    // two tasks that commit them without coordinating: the session row lock is
    // what orders them, and the sequences are contiguous whichever order the
    // lock grants.
    let mut batches: Vec<usize> = Vec::new();
    let mut total = 0usize;
    while total < EVENTS as usize {
        let size = (batches.len() % 7) + 1;
        let size = size.min(EVENTS as usize - total);
        batches.push(size);
        total += size;
    }

    let writers: Vec<_> = [0usize, 1]
        .into_iter()
        .map(|which| {
            let pool = app.pool.clone();
            let session_id = fixture.session_id;
            let mine: Vec<usize> = batches
                .iter()
                .enumerate()
                .filter(|(index, _)| index % 2 == which)
                .map(|(_, size)| *size)
                .collect();

            tokio::spawn(async move {
                for size in mine {
                    append_session_events(&pool, session_id, size).await;
                }
            })
        })
        .collect();

    for writer in writers {
        writer.await.expect("the writer finishes");
    }

    for collector in collectors {
        let (after, seen) = collector.await.expect("the collector finishes");

        assert_eq!(
            seen,
            ((after + 1)..=EVENTS).collect::<Vec<i64>>(),
            "the socket at cursor {after} saw exactly what is above its cursor, in order",
        );
        assert_eq!(
            seen.iter().copied().collect::<BTreeSet<i64>>().len(),
            seen.len(),
            "the socket at cursor {after} received an event twice",
        );
    }
}

// ---- 2 and 3. one combined transaction, committed and rolled back ----

/// A single transaction that locks the project row, appends a `task_events`
/// row and appends a session event delivers both frames — and neither of them
/// before it commits (`ARCHITECTURE.md`, "Task tracker"; ADR 0028).
///
/// This is the lock order the documents fix, built the only way the crate
/// allows: `TrackerMutation::begin` takes the project row, and the session
/// append runs on that same locked connection, so the project lock precedes
/// the session row lock by construction.
#[tokio::test]
async fn sse_and_ws_observe_one_combined_transaction() {
    let app = TestApp::spawn().await;
    let fixture = arrange(&app).await;

    let mut socket = app
        .ws(fixture.session_id, &fixture.user.access_token, 0)
        .await;
    expect_session_frame(&mut socket).await;

    let mut stream = SseReader::new(
        app.sse(fixture.project_id, &fixture.user.access_token, None, None)
            .await,
    );
    stream.keepalive_within(WITHIN).await;

    let mut mutation = TrackerMutation::begin(&app.pool, fixture.project_id, TaskActor::System)
        .await
        .expect("the mutation begins");

    // The project row is held; the session row is locked from inside it.
    let session_seq = SessionRepository::new(&app.pool)
        .append_event(
            &mut mutation.conn(),
            fixture.session_id,
            &AgentEvent::new(AgentEventBody::Text {
                text: "launched for a task".to_string(),
            }),
        )
        .await
        .expect("the session event appends");
    assert_eq!(session_seq, 1);

    // The task event is queued on the same mutation; its row, its sequence and
    // its notification are written on commit, inside this transaction
    // (`TrackerMutation::commit`).
    mutation
        .emit_deleted(Uuid::new_v4())
        .expect("the task event is emitted");

    // The session row is written and its `pg_notify` has run — inside the
    // transaction, so PostgreSQL has delivered nothing.
    tokio::join!(
        assert_ws_silent(
            &mut socket,
            Duration::from_secs(1),
            "nothing is delivered before the commit",
        ),
        assert_sse_silent(
            &mut stream,
            Duration::from_secs(1),
            "nothing is delivered before the commit",
        ),
    );

    let outcome = mutation.commit().await.expect("the mutation commits");
    assert_eq!(outcome.seqs, vec![1]);

    assert_eq!(
        collect_ws_events(&mut socket, 1, WITHIN).await,
        vec![1],
        "the socket receives the session event the transaction committed",
    );
    assert_eq!(
        collect_sse_ids(&mut stream, 1, WITHIN).await,
        vec![1],
        "the task stream receives the task event the same transaction committed",
    );
}

/// The same transaction rolled back exposes neither the changes nor their
/// events, anywhere (`ARCHITECTURE.md`, "Task tracker"; ADR 0028).
#[tokio::test]
async fn rollback_delivers_nothing_anywhere() {
    let app = TestApp::spawn().await;
    let fixture = arrange(&app).await;

    let mut socket = app
        .ws(fixture.session_id, &fixture.user.access_token, 0)
        .await;
    expect_session_frame(&mut socket).await;

    let mut stream = SseReader::new(
        app.sse(fixture.project_id, &fixture.user.access_token, None, None)
            .await,
    );
    stream.keepalive_within(WITHIN).await;

    {
        let mut mutation = TrackerMutation::begin(&app.pool, fixture.project_id, TaskActor::System)
            .await
            .expect("the mutation begins");

        SessionRepository::new(&app.pool)
            .append_events(
                &mut mutation.conn(),
                fixture.session_id,
                &[NewEvent::now("text", json!({ "text": "never committed" }))],
            )
            .await
            .expect("the session event appends");

        mutation
            .emit_deleted(Uuid::new_v4())
            .expect("the task event is emitted");

        // Ended without `commit`: the transaction rolls back, the task batch is
        // never appended, and PostgreSQL discards the session notification with
        // its row.
        mutation.no_change().await.expect("the mutation rolls back");
    }

    // Two safety reads, so a stream that would have read the rows has had two
    // chances to.
    tokio::join!(
        assert_ws_silent(
            &mut socket,
            2 * TEST_TIMINGS.safety_read,
            "a rolled-back transaction reached the socket",
        ),
        assert_sse_silent(
            &mut stream,
            2 * TEST_TIMINGS.safety_read,
            "a rolled-back transaction reached the task stream",
        ),
    );

    assert_eq!(
        SessionRepository::new(&app.pool)
            .max_seq(fixture.session_id)
            .await
            .expect("the session's highest sequence reads"),
        0,
        "a rolled-back transaction allocated no session sequence",
    );
    assert_eq!(
        TaskRepository::new(&app.pool)
            .max_task_event_seq(fixture.project_id)
            .await
            .expect("the project's highest sequence reads"),
        0,
        "a rolled-back transaction allocated no task sequence",
    );
}

// ---- 4. the listener's connection lost ----

/// Losing the listener's backend loses the notifications issued while it is
/// away; the streams deliver anyway, by `Resync` or by the safety read
/// (ADR 0005; `ARCHITECTURE.md`, "Event delivery").
///
/// Which of the two did it is deliberately not asserted: both are the
/// documented recovery, and a test that named one would break when the
/// reconnect happened to be faster.
///
/// The socket is a [`PolledSocket`]: this scenario spends its time on the task
/// stream — a keepalive to begin with, then a recovery that may take a whole
/// safety read — and a socket left unread for that long misses its two pongs
/// on a loaded machine, which is a close this scenario is not about.
#[tokio::test]
async fn listener_loss_is_recovered_by_streams() {
    let app = TestApp::spawn().await;
    let fixture = arrange(&app).await;

    let mut socket = app
        .ws(fixture.session_id, &fixture.user.access_token, 0)
        .await;
    expect_session_frame(&mut socket).await;
    let mut socket = PolledSocket::spawn(socket);

    let mut stream = SseReader::new(
        app.sse(fixture.project_id, &fixture.user.access_token, None, None)
            .await,
    );
    stream.keepalive_within(WITHIN).await;

    assert_eq!(
        common::terminate_listener_backend(&app.pool).await,
        1,
        "exactly one backend in this database is LISTENing",
    );

    // Committed in the window where the listener is gone: these are exactly
    // the notifications ADR 0005 calls harmless losses.
    append_session_events(&app.pool, fixture.session_id, 1).await;
    append_task_events(&app.pool, fixture.project_id, 1).await;

    // `max(5 s, 2 × safety read)`, which with the test timings is 5 s.
    assert_eq!(socket.events_until(1, WITHIN).await, vec![1]);
    assert_eq!(collect_sse_ids(&mut stream, 1, WITHIN).await, vec![1]);

    // And the listener is listening again: an append after the reconnect is an
    // ordinary live delivery.
    append_session_events(&app.pool, fixture.session_id, 1).await;
    append_task_events(&app.pool, fixture.project_id, 1).await;

    assert_eq!(
        socket.events_until(2, Duration::from_secs(2)).await,
        vec![2],
    );
    assert_eq!(
        collect_sse_ids(&mut stream, 2, Duration::from_secs(2)).await,
        vec![2],
    );
}

// ---- 5. a state change on every open socket ----

/// One `parked → running` transaction gives every open socket for that session
/// both frames: the `state_change` event and a fresh `session` snapshot
/// (`SPEC.md`, "WebSocket: session stream").
///
/// The order between the two is Postgres's — two notifications out of one
/// commit — so both are collected rather than asserted in sequence.
#[tokio::test]
async fn state_change_delivers_event_and_session_frames() {
    let app = TestApp::spawn().await;
    let fixture = arrange(&app).await;

    let mut sockets = Vec::new();
    for _ in 0..3 {
        let mut socket = app
            .ws(fixture.session_id, &fixture.user.access_token, 0)
            .await;
        assert_eq!(expect_session_frame(&mut socket).await["state"], "parked");
        sockets.push(socket);
    }

    let mut tx = app.pool.begin().await.expect("a transaction begins");
    SessionRepository::new(&app.pool)
        .transition(
            &mut tx,
            fixture.session_id,
            &Transition::new(SessionState::Parked, SessionState::Running, "relaunched"),
        )
        .await
        .expect("the session runs");
    tx.commit().await.expect("the transaction commits");

    for (index, socket) in sockets.iter_mut().enumerate() {
        let mut state_change = None;
        let mut snapshot = None;

        tokio::time::timeout(WITHIN, async {
            while state_change.is_none() || snapshot.is_none() {
                match socket.receive_message().await {
                    WsMessage::Text(text) => {
                        let frame: Value =
                            serde_json::from_str(text.as_str()).expect("every text frame is JSON");
                        match frame["type"].as_str() {
                            Some("event") => state_change = Some(frame["event"].clone()),
                            Some("session") => snapshot = Some(frame["session"].clone()),
                            other => panic!("unexpected frame type {other:?}"),
                        }
                    }
                    WsMessage::Ping(_) | WsMessage::Pong(_) => {}
                    other => panic!("socket {index} ended: {other:?}"),
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("socket {index} received both frames"));

        let state_change = state_change.expect("the state change event arrives");
        assert_eq!(state_change["kind"], "state_change");
        assert_eq!(state_change["from"], "parked");
        assert_eq!(state_change["to"], "running");

        assert_eq!(
            snapshot.expect("the session snapshot arrives")["state"],
            "running",
            "socket {index} sees the state it moved to",
        );
    }
}

// ---- 6. the replay page boundary ----

/// Replays that end exactly on, just past and well past a page boundary are
/// contiguous, on both stream kinds and from both cursors.
///
/// 500 is one full page (`MAX_EVENT_PAGE`, `MAX_TASK_EVENT_PAGE`), 501 the
/// page that is followed by a single row, 1000 two full pages: the three
/// counts where a reader that stops on a full page, or restarts one row late,
/// would show a gap or a duplicate.
#[tokio::test]
async fn replay_page_boundary_has_no_gap() {
    const COUNTS: [i64; 3] = [500, 501, 1000];
    const CURSORS: [i64; 2] = [0, 499];

    let app = TestApp::spawn().await;
    let fixture = arrange(&app).await;

    for count in COUNTS {
        // A session and a project of their own per count, so each replay is
        // exactly `count` rows long.
        let session_id = insert_session(&app, fixture.project_id, fixture.user.user.id).await;
        let project_id = insert_project(&app).await;

        append_session_events(&app.pool, session_id, count as usize).await;
        append_task_events(&app.pool, project_id, count as usize).await;

        for cursor in CURSORS {
            let expected: Vec<i64> = ((cursor + 1)..=count).collect();

            let mut socket = app.ws(session_id, &fixture.user.access_token, cursor).await;
            expect_session_frame(&mut socket).await;
            assert_eq!(
                collect_ws_events(&mut socket, count, WITHIN_BULK).await,
                expected,
                "{count} events replayed from {cursor} on the socket",
            );
            socket.close().await;

            let mut stream = SseReader::new(
                app.sse(project_id, &fixture.user.access_token, None, Some(cursor))
                    .await,
            );
            assert_eq!(
                collect_sse_ids(&mut stream, count, WITHIN_BULK).await,
                expected,
                "{count} task events replayed from Last-Event-ID {cursor}",
            );
        }
    }
}

// ---- 7. one client that never reads ----

/// A socket whose client stopped reading is the server's problem to end, not
/// the other subscribers' problem to wait for.
///
/// What is asserted is the outcome both documented mechanisms give — the
/// reading socket gets everything, the other one is ended by the server — and
/// not which mechanism ended it: with loopback buffers 200 small frames never
/// fill the socket, so it is the unanswered pings (`SPEC.md`, "WebSocket:
/// session stream": two missed pongs) rather than the ten-second send timeout
/// that closes it here.
#[tokio::test]
async fn slow_client_does_not_block_others() {
    const EVENTS: i64 = 200;

    let app = TestApp::spawn().await;
    let fixture = arrange(&app).await;

    // Never polled again from here on: no pong is written back, and the
    // frames the server sends it are never read.
    let _deaf = app
        .ws(fixture.session_id, &fixture.user.access_token, 0)
        .await;

    let mut reader = app
        .ws(fixture.session_id, &fixture.user.access_token, 0)
        .await;
    expect_session_frame(&mut reader).await;
    // Read by a task of its own from here on, and that is the whole point of
    // the scenario, not a convenience: this is the socket that *does* answer
    // its pings. Read only inside the waits below, it would answer none while
    // the scenario waits for the subscriptions and commits two hundred rows —
    // on a loaded runner more than the 600 ms two unanswered pings take — and
    // the server would rightly close it with 1001, the close this scenario
    // asserts only for the other socket.
    let mut reader = PolledSocket::spawn(reader);

    tokio::time::timeout(WITHIN, async {
        while app.state.fanout.session_subscribers(fixture.session_id) < 2 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("both sockets are subscribed");

    append_session_events(&app.pool, fixture.session_id, EVENTS as usize).await;

    assert_eq!(
        reader.events_until(EVENTS, WITHIN).await,
        (1..=EVENTS).collect::<Vec<i64>>(),
        "the reading socket receives every event, in order, within five seconds",
    );

    // The server ends the socket that answers nothing, and the handler drops
    // its fan-out receiver as it goes. Two unanswered pings close it (600 ms
    // with the test timings: a ping on the first two ticks, the close on the
    // third) and the receiver is dropped before the close linger, so five
    // seconds is a wide margin around a sub-second event. The reading socket
    // keeps answering throughout, from its own task.
    tokio::time::timeout(WITHIN, async {
        while app.state.fanout.session_subscribers(fixture.session_id) != 1 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the server ends the socket that never reads");

    // Nothing reached the reading socket meanwhile — no close, no error: a
    // close would have been forwarded, and would be what `events_until`
    // panics on below.
    match reader.messages.try_recv() {
        Err(mpsc::error::TryRecvError::Empty) => {}
        other => panic!("the reading socket received {other:?}"),
    }

    // And the socket that did read is still there and still live.
    append_session_events(&app.pool, fixture.session_id, 1).await;
    assert_eq!(
        reader.events_until(EVENTS + 1, WITHIN).await,
        vec![EVENTS + 1],
        "the reading socket is unaffected by the one the server ended",
    );
}
