//! `GET /ws/sessions/{id}` end to end (`SPEC.md`, "WebSocket: session
//! stream"; `ARCHITECTURE.md`, "Event delivery").
//!
//! The read side of the session socket, over a real HTTP transport so the
//! upgrade actually happens (`TestApp::ws`). What is asserted here is the part
//! of the contract that cannot be seen from a unit test:
//!
//! - everything that must be answered with a *status* is answered before the
//!   upgrade — 401, 403, 404 and 400 — because a browser can act on those and
//!   cannot act on a close frame;
//! - the handler subscribes before it replays, so an event committed while the
//!   first page is being written is delivered exactly once, and the
//!   commit-during-replay scenario below is the proof;
//! - a lost notification is covered by the safety read, which is asserted with
//!   the shared listener switched off, so nothing *but* the safety read can
//!   deliver the event;
//! - the socket pings and closes after two missed pongs, and lets go of its
//!   fan-out subscription when the client leaves.
//!
//! The periods are `common::TEST_TIMINGS` (200 ms ping, 300 ms safety read),
//! not the documented thirty seconds.
//!
//! Needs a container engine; see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use std::collections::BTreeSet;
use std::time::Duration;

use axum::http::StatusCode;
use axum_test::WsMessage;
use common::{AuthenticatedUser, TEST_TIMINGS, TestApp};
use mars_orchestrator::events::{AgentEvent, AgentEventBody, Notice};
use mars_orchestrator::models::{NewEvent, NewSession, ProfileKind, SessionState};
use mars_orchestrator::repositories::{SessionRepository, Transition};
use serde_json::{Value, json};
use uuid::Uuid;

/// How long a frame that should already be on its way is waited for.
///
/// Generous against the test timings on purpose: every assertion below is
/// "this arrives", never "this arrives quickly", and a scheduling hiccup on a
/// loaded machine must not be a failure.
const WITHIN: Duration = Duration::from_secs(2);

/// An obviously fake password, long enough for `POST /api/test/users`
/// (rule 3).
const PASSWORD: &str = "not-a-real-password";

/// How many of a `TestApp`'s pooled connections a test can hold at once.
///
/// The pool is built one larger than the harness default precisely because the
/// shared listener holds one of them permanently (`tests/common/app.rs`), so
/// the default is what is left for everybody else — and holding all of it is
/// how the scenario below stalls a handler.
const POOL_CONNECTIONS: usize = common::db::DEFAULT_MAX_CONNECTIONS as usize;

/// The rows one socket needs: a user to open it as and a session to watch.
struct Fixture {
    user: AuthenticatedUser,
    session_id: Uuid,
}

/// A signed-in user, a project, a profile and one `parked` conversational
/// session.
///
/// The project and profile go in with unchecked statements, the way the other
/// repository-level suites seed them: what this file is about starts at the
/// session.
async fn arrange(app: &TestApp) -> Fixture {
    let user = app
        .create_user("streamer", "streamer@example.test", PASSWORD)
        .await;

    let project_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id, name, remote_url) VALUES ($1, $2, $3)")
        .bind(project_id)
        .bind(format!("project-{project_id}"))
        .bind("https://git.example.test/mars.git")
        .execute(&app.pool)
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
    session.created_by = Some(user.user.id);

    let mut tx = app.pool.begin().await.expect("a transaction begins");
    let inserted = SessionRepository::new(&app.pool)
        .insert(&mut tx, &session)
        .await
        .expect("the session inserts");
    tx.commit().await.expect("the transaction commits");

    // Straight to `parked`: the lifecycle that gets a session there is not
    // what this file is testing, and `parked → running` is the transition the
    // state-change scenario drives.
    sqlx::query("UPDATE sessions SET state = 'parked' WHERE id = $1")
        .bind(inserted.id)
        .execute(&app.pool)
        .await
        .expect("the session parks");

    Fixture {
        user,
        session_id: inserted.id,
    }
}

/// Append `count` `text` events in one transaction, each carrying an
/// `_offset` — the internal field that must never leave the orchestrator.
async fn append_events(app: &TestApp, session_id: Uuid, count: usize) -> Vec<i64> {
    let events: Vec<NewEvent> = (0..count)
        .map(|index| {
            NewEvent::now("text", json!({ "text": format!("line {index}") }))
                .with_offset((index as i64 + 1) * 10)
        })
        .collect();

    let mut tx = app.pool.begin().await.expect("a transaction begins");
    let sequences = SessionRepository::new(&app.pool)
        .append_events(&mut tx, session_id, &events)
        .await
        .expect("the events append");
    tx.commit().await.expect("the transaction commits");

    sequences
}

/// The next JSON frame, ignoring the pings the server sends underneath.
///
/// Panics if the socket closes or nothing arrives within [`WITHIN`], which is
/// what every "the client receives X" assertion here wants.
async fn next_json(socket: &mut axum_test::TestWebSocket) -> Value {
    let frame = tokio::time::timeout(WITHIN, async {
        loop {
            match socket.receive_message().await {
                WsMessage::Text(text) => {
                    return serde_json::from_str::<Value>(text.as_str())
                        .expect("every text frame is JSON");
                }
                WsMessage::Ping(_) | WsMessage::Pong(_) => {}
                other => panic!("expected a JSON frame, got {other:?}"),
            }
        }
    })
    .await;

    frame.expect("a frame arrives within the window")
}

/// The `session` snapshot every stream opens with.
async fn expect_session(socket: &mut axum_test::TestWebSocket) -> Value {
    let frame = next_json(socket).await;
    assert_eq!(frame["type"], "session", "the first frame is the snapshot");

    frame["session"].clone()
}

/// Read the socket for `during`, answering pings and failing on anything the
/// server should not be sending.
///
/// A socket has to be *polled* for tungstenite to answer a ping, so "wait"
/// and "stay alive" are the same loop.
async fn poll_for(socket: &mut axum_test::TestWebSocket, during: Duration) {
    let deadline = tokio::time::Instant::now() + during;

    while let Ok(message) = tokio::time::timeout_at(deadline, socket.receive_message()).await {
        match message {
            WsMessage::Ping(_) | WsMessage::Pong(_) => {}
            other => panic!("the socket received {other:?} while it was only being kept alive"),
        }
    }
}

/// The next frame, asserted to be an `event`.
async fn expect_event(socket: &mut axum_test::TestWebSocket) -> Value {
    let frame = next_json(socket).await;
    assert_eq!(frame["type"], "event", "expected an event frame: {frame}");

    frame["event"].clone()
}

#[tokio::test]
async fn a_socket_without_a_token_is_refused_before_the_upgrade() {
    let app = TestApp::spawn().await;
    let fixture = arrange(&app).await;

    let response = app.ws_request(fixture.session_id, None, None).await;

    response.assert_status(StatusCode::UNAUTHORIZED);
    assert_eq!(
        response.json::<Value>()["error"],
        "authentication required",
        "the socket is refused with the one authentication message",
    );
}

#[tokio::test]
async fn a_garbage_or_expired_token_is_refused_before_the_upgrade() {
    let app = TestApp::spawn().await;
    let fixture = arrange(&app).await;

    let expired = app.expired_token_for(&fixture.user.user);

    for token in ["not-a-real-token", expired.as_str()] {
        let response = app
            .ws_request(fixture.session_id, Some(token), Some(0))
            .await;

        response.assert_status(StatusCode::UNAUTHORIZED);
        assert_eq!(response.json::<Value>()["error"], "authentication required");
    }
}

#[tokio::test]
async fn a_gated_user_is_refused_before_the_upgrade() {
    let app = TestApp::spawn().await;
    let fixture = arrange(&app).await;

    let gated = app.create_gated_user("gated", "gated@example.test").await;
    let response = app
        .ws_request(fixture.session_id, Some(&gated.access_token), Some(0))
        .await;

    response.assert_status(StatusCode::FORBIDDEN);
    assert_eq!(
        response.json::<Value>()["error"],
        "password change required"
    );
}

#[tokio::test]
async fn an_unknown_session_is_a_not_found_before_the_upgrade() {
    let app = TestApp::spawn().await;
    let fixture = arrange(&app).await;

    let response = app
        .ws_request(Uuid::new_v4(), Some(&fixture.user.access_token), Some(0))
        .await;

    response.assert_status(StatusCode::NOT_FOUND);
    assert_eq!(response.json::<Value>()["error"], "session not found");
}

#[tokio::test]
async fn an_after_that_is_not_a_cursor_is_a_bad_request_before_the_upgrade() {
    let app = TestApp::spawn().await;
    let fixture = arrange(&app).await;

    let response = app
        .ws_request(
            fixture.session_id,
            Some(&fixture.user.access_token),
            Some(-1),
        )
        .await;

    response.assert_status(StatusCode::BAD_REQUEST);
    assert_eq!(response.json::<Value>()["error"], "after must be >= 0");

    let response = app
        .ws_request_raw_after(
            fixture.session_id,
            Some(&fixture.user.access_token),
            "seven",
        )
        .await;

    response.assert_status(StatusCode::BAD_REQUEST);
    assert_eq!(response.json::<Value>()["error"], "after must be >= 0");
}

#[tokio::test]
async fn the_stream_opens_with_a_snapshot_and_replays_from_the_cursor() {
    let app = TestApp::spawn().await;
    let fixture = arrange(&app).await;
    append_events(&app, fixture.session_id, 7).await;

    let mut socket = app
        .ws(fixture.session_id, &fixture.user.access_token, 4)
        .await;

    let session = expect_session(&mut socket).await;
    assert_eq!(session["id"], fixture.session_id.to_string());
    assert_eq!(session["state"], "parked");
    assert!(
        session.get("mcp_token_hash").is_none(),
        "the session DTO never carries the MCP token hash",
    );

    for seq in 5..=7 {
        let event = expect_event(&mut socket).await;

        assert_eq!(event["seq"], seq, "the replay is in sequence order");
        assert_eq!(event["kind"], "text");
        assert!(
            event.get("_offset").is_none(),
            "an internal field must never leave the orchestrator: {event}",
        );
    }

    socket.close().await;
}

#[tokio::test]
async fn an_after_beyond_the_end_replays_nothing_and_still_follows() {
    let app = TestApp::spawn().await;
    let fixture = arrange(&app).await;
    append_events(&app, fixture.session_id, 3).await;

    let mut socket = app
        .ws(fixture.session_id, &fixture.user.access_token, 99)
        .await;

    // The snapshot and nothing else: every stored event is at or below the
    // cursor the client named.
    expect_session(&mut socket).await;

    // The live reads carry on from that same cursor, so an event written now
    // — sequence 4, still below 99 — is not sent either, through the notice
    // *or* through the safety read.
    append_events(&app, fixture.session_id, 1).await;

    // Three safety reads' worth of silence, pings excepted — and the socket
    // has to be polled throughout, or the pings this ignores would go
    // unanswered and close it.
    let deadline = tokio::time::Instant::now() + 3 * TEST_TIMINGS.safety_read;
    while let Ok(message) = tokio::time::timeout_at(deadline, socket.receive_message()).await {
        if let WsMessage::Text(text) = message {
            panic!("nothing below the client's cursor is ever sent: {text}");
        }
    }

    socket.close().await;
}

#[tokio::test]
async fn a_live_event_arrives_after_the_replay() {
    let app = TestApp::spawn().await;
    let fixture = arrange(&app).await;
    append_events(&app, fixture.session_id, 7).await;

    let mut socket = app
        .ws(fixture.session_id, &fixture.user.access_token, 7)
        .await;

    expect_session(&mut socket).await;
    append_events(&app, fixture.session_id, 1).await;

    let event = expect_event(&mut socket).await;
    assert_eq!(event["seq"], 8, "the live event follows the replay");

    socket.close().await;
}

#[tokio::test]
async fn an_event_committed_during_the_replay_is_delivered_exactly_once() {
    let app = TestApp::spawn().await;
    let fixture = arrange(&app).await;
    append_events(&app, fixture.session_id, 600).await;

    let mut socket = app
        .ws(fixture.session_id, &fixture.user.access_token, 0)
        .await;

    expect_session(&mut socket).await;

    // Committed from another task while the first page — 500 events — is
    // still being written to the socket. The subscription precedes the replay,
    // so this row is either in a later page or wakes the live loop; it cannot
    // be lost, and the cursor advances per row, so it cannot arrive twice.
    let pool = app.pool.clone();
    let session_id = fixture.session_id;
    let appender = tokio::spawn(async move {
        let event = AgentEvent::new(AgentEventBody::Text {
            text: "committed during the replay".to_string(),
        });
        let mut tx = pool.begin().await.expect("a transaction begins");
        let seq = SessionRepository::new(&pool)
            .append_event(&mut tx, session_id, &event)
            .await
            .expect("the event appends");
        tx.commit().await.expect("the transaction commits");
        seq
    });

    let mut seen: Vec<i64> = Vec::new();
    while seen.last() != Some(&601) {
        let event = expect_event(&mut socket).await;
        seen.push(event["seq"].as_i64().expect("every event carries a seq"));
    }

    assert_eq!(appender.await.expect("the appender finishes"), 601);
    assert_eq!(
        seen,
        (1..=601).collect::<Vec<i64>>(),
        "every event arrives once, in order, across the page boundary",
    );
    assert_eq!(
        seen.iter().copied().collect::<BTreeSet<i64>>().len(),
        seen.len(),
        "no event is delivered twice",
    );

    socket.close().await;
}

#[tokio::test]
async fn a_state_change_sends_both_its_event_and_a_session_snapshot() {
    let app = TestApp::spawn().await;
    let fixture = arrange(&app).await;

    let mut socket = app
        .ws(fixture.session_id, &fixture.user.access_token, 0)
        .await;

    assert_eq!(expect_session(&mut socket).await["state"], "parked");

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

    // Two notices out of one commit, and their order on the wire is
    // Postgres's, so both are collected rather than asserted in sequence.
    let mut state_change = None;
    let mut snapshot = None;
    while state_change.is_none() || snapshot.is_none() {
        let frame = next_json(&mut socket).await;
        match frame["type"].as_str() {
            Some("event") => state_change = Some(frame["event"].clone()),
            Some("session") => snapshot = Some(frame["session"].clone()),
            other => panic!("unexpected frame type {other:?}"),
        }
    }

    let state_change = state_change.expect("the state change event arrives");
    assert_eq!(state_change["kind"], "state_change");
    assert_eq!(state_change["from"], "parked");
    assert_eq!(state_change["to"], "running");

    assert_eq!(
        snapshot.expect("the session snapshot arrives")["state"],
        "running",
    );

    socket.close().await;
}

/// A `result` moves the session's cost and token counters in the commit that
/// stores it, and no state changes: the socket follows the event with a fresh
/// `session` frame, or the client's header would keep the counters of the last
/// state change.
#[tokio::test]
async fn a_result_is_followed_by_a_session_snapshot_with_its_counters() {
    let app = TestApp::spawn().await;
    let fixture = arrange(&app).await;

    let mut socket = app
        .ws(fixture.session_id, &fixture.user.access_token, 0)
        .await;

    assert_eq!(expect_session(&mut socket).await["cost_usd"], 0.0);

    let result = AgentEvent::new(AgentEventBody::Result {
        subtype: "success".to_string(),
        terminal_reason: Some("completed".to_string()),
        is_error: false,
        num_turns: 1,
        duration_ms: 1200,
        cost_usd: Some(0.25),
        usage: Some(json!({ "input_tokens": 12, "output_tokens": 34 })),
        permission_denials: Vec::new(),
    });
    let repository = SessionRepository::new(&app.pool);
    let mut tx = app.pool.begin().await.expect("a transaction begins");
    repository
        .append_event(&mut tx, fixture.session_id, &result)
        .await
        .expect("the result appends");
    repository
        .add_usage(&mut tx, fixture.session_id, 0.25, 12, 34)
        .await
        .expect("the counters move");
    tx.commit().await.expect("the transaction commits");

    assert_eq!(expect_event(&mut socket).await["kind"], "result");
    let session = expect_session_frame(&mut socket).await;
    assert_eq!(session["cost_usd"], 0.25);
    assert_eq!(session["input_tokens"], 12);
    assert_eq!(session["output_tokens"], 34);

    socket.close().await;
}

/// The next frame, asserted to be a `session` snapshot.
async fn expect_session_frame(socket: &mut axum_test::TestWebSocket) -> Value {
    let frame = next_json(socket).await;
    assert_eq!(
        frame["type"], "session",
        "expected a session frame: {frame}"
    );

    frame["session"].clone()
}

#[tokio::test]
async fn a_lost_notification_is_covered_by_the_safety_read() {
    let app = TestApp::spawn().await;
    let fixture = arrange(&app).await;

    let mut socket = app
        .ws(fixture.session_id, &fixture.user.access_token, 0)
        .await;

    expect_session(&mut socket).await;

    // No listener, so no notice can reach the fan-out: whatever arrives now
    // arrived because the handler read from its cursor on its own.
    app.listener.shutdown.cancel();
    tokio::time::sleep(TEST_TIMINGS.safety_read).await;

    append_events(&app, fixture.session_id, 1).await;

    let event = expect_event(&mut socket).await;
    assert_eq!(event["seq"], 1, "the safety read delivers the event");

    socket.close().await;
}

#[tokio::test]
async fn the_server_pings_and_closes_after_two_missed_pongs() {
    let app = TestApp::spawn().await;
    let fixture = arrange(&app).await;

    let mut socket = app
        .ws(fixture.session_id, &fixture.user.access_token, 0)
        .await;

    expect_session(&mut socket).await;

    // Nothing is polled for three ping intervals, so no pong is written back:
    // tungstenite answers a ping only while the stream is being read.
    tokio::time::sleep(3 * TEST_TIMINGS.ping + Duration::from_millis(200)).await;

    let mut pings = 0;
    let closed = loop {
        match tokio::time::timeout(WITHIN, socket.receive_message())
            .await
            .expect("the buffered frames are already there")
        {
            WsMessage::Ping(_) => pings += 1,
            WsMessage::Close(frame) => break frame,
            WsMessage::Text(_) => {}
            other => panic!("unexpected frame {other:?}"),
        }
    };

    assert!(pings >= 2, "the server pings every interval, saw {pings}");
    let closed = closed.expect("the close frame carries a code");
    assert_eq!(
        u16::from(closed.code),
        1001,
        "two missed pongs close the socket with 1001",
    );
}

#[tokio::test]
async fn a_stalled_handler_does_not_spend_the_pong_budget_in_one_instant() {
    let app = TestApp::spawn().await;
    let fixture = arrange(&app).await;

    let mut socket = app
        .ws(fixture.session_id, &fixture.user.access_token, 0)
        .await;

    expect_session(&mut socket).await;

    // Every connection the app's pool has, held by the test: the socket's loop
    // reaches its next re-authorization or safety read and waits there, which
    // is what a busy orchestrator does to it. No ping goes out while it waits,
    // so the client owes no pong.
    let mut held = Vec::new();
    tokio::time::timeout(WITHIN, async {
        while held.len() < POOL_CONNECTIONS {
            held.push(app.pool.acquire().await.expect("a connection is acquired"));
        }
    })
    .await
    .expect("the pool's connections can all be held");

    // Longer than the two ping intervals that close a silent socket, and the
    // socket is polled throughout: what is being arranged is a stalled
    // *server*, not a client that stopped reading.
    poll_for(&mut socket, 4 * TEST_TIMINGS.ping).await;

    drop(held);

    // The ticks the stall swallowed must not now fire back to back: two pings
    // in the same instant would reach `MAX_MISSED_PONGS` before the client
    // could answer either, and close a socket that never missed a pong
    // (`SPEC.md`, "WebSocket: session stream").
    poll_for(&mut socket, 4 * TEST_TIMINGS.ping).await;

    append_events(&app, fixture.session_id, 1).await;
    let event = expect_event(&mut socket).await;
    assert_eq!(event["seq"], 1, "the socket survived the stall");

    socket.close().await;
}

#[tokio::test]
async fn a_session_that_disappears_mid_stream_closes_the_socket() {
    let app = TestApp::spawn().await;
    let fixture = arrange(&app).await;

    let mut socket = app
        .ws(fixture.session_id, &fixture.user.access_token, 0)
        .await;

    expect_session(&mut socket).await;

    sqlx::query("DELETE FROM sessions WHERE id = $1")
        .bind(fixture.session_id)
        .execute(&app.pool)
        .await
        .expect("the session is deleted");

    // A deletion issues no notification, so the reload is provoked the way a
    // state change would: the handler answers a `session_state` notice by
    // reading the row, and there is no longer one to read.
    app.state.fanout.publish_session(
        fixture.session_id,
        Notice::SessionState {
            state: SessionState::Done,
        },
    );

    let frame = next_json(&mut socket).await;
    assert_eq!(frame["type"], "error");
    assert_eq!(frame["message"], "session not found");

    let closed = loop {
        if let WsMessage::Close(frame) = tokio::time::timeout(WITHIN, socket.receive_message())
            .await
            .expect("the close follows the error")
        {
            break frame;
        }
    };
    assert_eq!(
        u16::from(closed.expect("the close frame carries a code").code),
        1000,
        "a deleted session is a normal close",
    );
}

#[tokio::test]
async fn a_closed_socket_releases_its_fanout_subscription() {
    let app = TestApp::spawn().await;
    let fixture = arrange(&app).await;

    let mut socket = app
        .ws(fixture.session_id, &fixture.user.access_token, 0)
        .await;

    expect_session(&mut socket).await;
    assert_eq!(
        app.state.fanout.session_subscribers(fixture.session_id),
        1,
        "an open socket holds exactly one subscription",
    );

    socket.close().await;

    tokio::time::timeout(WITHIN, async {
        while app.state.fanout.session_subscribers(fixture.session_id) != 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the subscription is released when the client leaves");
}
