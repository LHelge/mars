//! The write side of the session socket: `input`, `stop` and the
//! re-authorization that guards both (`SPEC.md`, "WebSocket: session stream",
//! "Authentication"; `ARCHITECTURE.md`, "Event delivery"; ADR 0020, ADR 0025).
//!
//! What is asserted here is what a unit test cannot see: that a frame sent by
//! a real client reaches the session's owner through the registry, that every
//! refusal the service can produce comes back as `input_rejected` with the
//! service's own words, and that a login revoked while the socket is open
//! closes it — on the next message *and* on the next ping — without touching
//! the agent session.
//!
//! The owner is a fake one: `register` + `mark_running` puts an entry in the
//! registry and the test holds the receiving half, so the assertion is the
//! [`OwnerCommand`] that arrives. What the real owner then does with it is the
//! session owner's own suite, and running one here would say nothing more
//! about the socket.
//!
//! The periods are `common::TEST_TIMINGS` (200 ms ping), not the documented
//! thirty seconds.
//!
//! Needs a container engine; see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use std::time::Duration;

use axum_test::{TestWebSocket, WsMessage};
use common::{AuthenticatedUser, TEST_TIMINGS, TestApp};
use mars_orchestrator::models::{NewEvent, NewSession, ProfileKind, SessionState};
use mars_orchestrator::repositories::SessionRepository;
use mars_orchestrator::session::{OwnerCommand, OwnerRx, Phase, StopReason};
use serde_json::{Value, json};
use uuid::Uuid;

/// How long a frame that should already be on its way is waited for.
///
/// Generous against the test timings on purpose: every assertion below is
/// "this arrives", never "this arrives quickly".
const WITHIN: Duration = Duration::from_secs(2);

/// An obviously fake password, long enough for `POST /api/test/users`
/// (rule 3).
const PASSWORD: &str = "not-a-real-password";

/// The close code a revoked stream is closed with (`SPEC.md`,
/// "Authentication").
const POLICY_VIOLATION: u16 = 1008;

/// A signed-in user and one session to send input to.
struct Fixture {
    user: AuthenticatedUser,
    session_id: Uuid,
}

/// A user, a project, a profile of `kind` and one session of that kind in
/// `state`.
///
/// The project and profile go in with unchecked statements, the way the other
/// socket suite seeds them: what this file is about starts at the session.
async fn arrange(app: &TestApp, kind: ProfileKind, state: SessionState) -> Fixture {
    let suffix = &Uuid::new_v4().simple().to_string()[..8];
    let user = app
        .create_user(
            &format!("writer-{suffix}"),
            &format!("writer-{suffix}@example.test"),
            PASSWORD,
        )
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
        "INSERT INTO agent_profiles (id, project_id, name, kind, image, partial_messages)
         VALUES ($1, $2, 'default', $3, $4, TRUE)",
    )
    .bind(profile_id)
    .bind(project_id)
    .bind(kind)
    .bind("localhost/mars-session:test")
    .execute(&app.pool)
    .await
    .expect("the profile seeds");

    let mut session = NewSession::new(
        project_id,
        profile_id,
        kind,
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

    // Straight into the state under test: how a session gets there is the
    // lifecycle's own suite.
    sqlx::query("UPDATE sessions SET state = $1 WHERE id = $2")
        .bind(state)
        .bind(inserted.id)
        .execute(&app.pool)
        .await
        .expect("the session takes its state");

    Fixture {
        user,
        session_id: inserted.id,
    }
}

/// Put a live fake owner in the registry for this session and keep its end of
/// the channel.
fn attach_owner(app: &TestApp, session_id: Uuid) -> OwnerRx {
    let registry = app.session_registry();
    let owner = registry.register(session_id, ProfileKind::Conversational, Phase::Creating);
    registry.mark_running(session_id);

    owner
}

/// Append `count` `text` events, so the acknowledged `seq` is not trivially
/// zero.
async fn append_events(app: &TestApp, session_id: Uuid, count: usize) {
    let events: Vec<NewEvent> = (0..count)
        .map(|index| NewEvent::now("text", json!({ "text": format!("line {index}") })))
        .collect();

    let mut tx = app.pool.begin().await.expect("a transaction begins");
    SessionRepository::new(&app.pool)
        .append_events(&mut tx, session_id, &events)
        .await
        .expect("the events append");
    tx.commit().await.expect("the transaction commits");
}

/// The state the session row is in right now.
async fn state_of(app: &TestApp, session_id: Uuid) -> SessionState {
    SessionRepository::new(&app.pool)
        .get(session_id)
        .await
        .expect("the session is readable")
        .state
}

/// The next JSON frame, ignoring the pings the server sends underneath.
async fn next_json(socket: &mut TestWebSocket) -> Value {
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

/// The `session` snapshot every stream opens with, consumed and discarded.
async fn expect_session(socket: &mut TestWebSocket) {
    let frame = next_json(socket).await;
    assert_eq!(frame["type"], "session", "the first frame is the snapshot");
}

/// Send one `input` message the way a browser does.
async fn send_input(socket: &mut TestWebSocket, client_id: &str, text: &str) {
    socket
        .send_text(
            json!({
                "type": "input",
                "client_id": client_id,
                "input": { "kind": "message", "text": text },
            })
            .to_string(),
        )
        .await;
}

/// Assert that nothing but a ping arrives for `window`.
///
/// The socket has to be polled throughout, or the pings this ignores would go
/// unanswered and close it.
async fn expect_silence(socket: &mut TestWebSocket, window: Duration) {
    let deadline = tokio::time::Instant::now() + window;
    while let Ok(message) = tokio::time::timeout_at(deadline, socket.receive_message()).await {
        match message {
            WsMessage::Ping(_) | WsMessage::Pong(_) => {}
            other => panic!("the socket should have said nothing, got {other:?}"),
        }
    }
}

/// Assert the documented revocation close: `error { authentication required }`
/// and code 1008.
async fn expect_authentication_close(socket: &mut TestWebSocket) {
    let frame = next_json(socket).await;
    assert_eq!(frame["type"], "error", "the close is announced: {frame}");
    assert_eq!(frame["message"], "authentication required");

    let close = tokio::time::timeout(WITHIN, socket.receive_message())
        .await
        .expect("the close frame arrives within the window");

    match close {
        WsMessage::Close(Some(frame)) => {
            assert_eq!(
                u16::from(frame.code),
                POLICY_VIOLATION,
                "a revoked stream closes with 1008",
            );
            assert_eq!(frame.reason.as_str(), "authentication required");
        }
        other => panic!("expected a close frame, got {other:?}"),
    }
}

#[tokio::test]
async fn an_input_reaches_the_owner_and_is_acknowledged_with_the_current_sequence() {
    let app = TestApp::spawn().await;
    let fixture = arrange(&app, ProfileKind::Conversational, SessionState::Running).await;
    let mut owner = attach_owner(&app, fixture.session_id);
    append_events(&app, fixture.session_id, 3).await;

    let mut socket = app
        .ws(fixture.session_id, &fixture.user.access_token, 3)
        .await;
    expect_session(&mut socket).await;

    send_input(&mut socket, "client-1", "hello").await;

    let frame = next_json(&mut socket).await;
    assert_eq!(frame["type"], "input_accepted", "{frame}");
    assert_eq!(
        frame["client_id"], "client-1",
        "the client's own id comes back verbatim",
    );
    assert_eq!(
        frame["seq"], 3,
        "`seq` is the highest committed sequence at acceptance",
    );

    let command = tokio::time::timeout(WITHIN, owner.recv())
        .await
        .expect("the owner is told within the window")
        .expect("the registry forwarded the input");

    match command {
        OwnerCommand::Input(queued) => {
            assert_eq!(queued.client_id.as_deref(), Some("client-1"));
            assert_eq!(queued.user_id, Some(fixture.user.user.id));
            assert_eq!(queued.input.text(), "hello");
        }
        other => panic!("expected the input, got {other:?}"),
    }

    socket.close().await;
}

#[tokio::test]
async fn an_ephemeral_session_rejects_input_with_the_service_s_own_words() {
    let app = TestApp::spawn().await;
    let fixture = arrange(&app, ProfileKind::Ephemeral, SessionState::Running).await;
    let mut owner = attach_owner(&app, fixture.session_id);

    let mut socket = app
        .ws(fixture.session_id, &fixture.user.access_token, 0)
        .await;
    expect_session(&mut socket).await;

    send_input(&mut socket, "client-2", "another one").await;

    let frame = next_json(&mut socket).await;
    assert_eq!(frame["type"], "input_rejected", "{frame}");
    assert_eq!(frame["client_id"], "client-2");
    assert_eq!(frame["reason"], "ephemeral sessions accept no input");
    assert!(
        owner.try_recv().is_err(),
        "a refused input never reaches the owner",
    );

    socket.close().await;
}

#[tokio::test]
async fn a_session_that_has_ended_rejects_input() {
    let app = TestApp::spawn().await;
    let fixture = arrange(&app, ProfileKind::Conversational, SessionState::Done).await;

    let mut socket = app
        .ws(fixture.session_id, &fixture.user.access_token, 0)
        .await;
    expect_session(&mut socket).await;

    send_input(&mut socket, "client-3", "too late").await;

    let frame = next_json(&mut socket).await;
    assert_eq!(frame["type"], "input_rejected", "{frame}");
    assert_eq!(frame["reason"], "session is done");

    socket.close().await;
}

#[tokio::test]
async fn an_input_with_no_owner_to_deliver_it_is_rejected() {
    let app = TestApp::spawn().await;
    // `running` in the database with nothing in the registry: what a restart
    // that could not adopt the container leaves behind (ADR 0020).
    let fixture = arrange(&app, ProfileKind::Conversational, SessionState::Running).await;

    let mut socket = app
        .ws(fixture.session_id, &fixture.user.access_token, 0)
        .await;
    expect_session(&mut socket).await;

    send_input(&mut socket, "client-4", "anyone there").await;

    let frame = next_json(&mut socket).await;
    assert_eq!(frame["type"], "input_rejected", "{frame}");
    assert_eq!(frame["reason"], "session has no owner");

    socket.close().await;
}

#[tokio::test]
async fn an_input_to_a_parked_session_is_queued_and_accepted() {
    let app = TestApp::spawn().await;
    let fixture = arrange(&app, ProfileKind::Conversational, SessionState::Parked).await;

    // The resume a parked session's message triggers is the launcher's, and
    // its suite drives it end to end; holding a launch guard here makes the
    // registry answer "somebody is already relaunching", which is the same
    // acceptance without a container.
    let _launching = app
        .session_registry()
        .try_begin_launch(fixture.session_id)
        .expect("nothing else is launching this session");

    let mut socket = app
        .ws(fixture.session_id, &fixture.user.access_token, 0)
        .await;
    expect_session(&mut socket).await;

    send_input(&mut socket, "client-5", "wake up").await;

    let frame = next_json(&mut socket).await;
    assert_eq!(frame["type"], "input_accepted", "{frame}");
    assert_eq!(frame["client_id"], "client-5");
    assert_eq!(
        app.session_registry().queued(fixture.session_id),
        1,
        "the input waits for the resume that will drain it",
    );

    socket.close().await;
}

#[tokio::test]
async fn a_blank_input_is_rejected_rather_than_closing_the_socket() {
    let app = TestApp::spawn().await;
    let fixture = arrange(&app, ProfileKind::Conversational, SessionState::Running).await;
    let mut owner = attach_owner(&app, fixture.session_id);

    let mut socket = app
        .ws(fixture.session_id, &fixture.user.access_token, 0)
        .await;
    expect_session(&mut socket).await;

    send_input(&mut socket, "client-6", "   ").await;

    let frame = next_json(&mut socket).await;
    assert_eq!(frame["type"], "input_rejected", "{frame}");
    assert_eq!(frame["reason"], "text must not be empty");
    assert!(owner.try_recv().is_err(), "nothing blank reaches the owner");

    socket.close().await;
}

#[tokio::test]
async fn a_client_id_over_the_bound_is_rejected_without_asking_the_service() {
    let app = TestApp::spawn().await;
    let fixture = arrange(&app, ProfileKind::Conversational, SessionState::Running).await;
    let mut owner = attach_owner(&app, fixture.session_id);

    let mut socket = app
        .ws(fixture.session_id, &fixture.user.access_token, 0)
        .await;
    expect_session(&mut socket).await;

    let too_long = "c".repeat(129);
    send_input(&mut socket, &too_long, "hello").await;

    let frame = next_json(&mut socket).await;
    assert_eq!(frame["type"], "input_rejected", "{frame}");
    assert_eq!(frame["client_id"], too_long, "the id comes back verbatim");
    assert_eq!(frame["reason"], "client_id too long");
    assert!(
        owner.try_recv().is_err(),
        "the service is never asked about it",
    );

    socket.close().await;
}

#[tokio::test]
async fn a_stop_reaches_the_owner_and_is_answered_with_no_frame() {
    let app = TestApp::spawn().await;
    let fixture = arrange(&app, ProfileKind::Conversational, SessionState::Running).await;
    let mut owner = attach_owner(&app, fixture.session_id);

    let mut socket = app
        .ws(fixture.session_id, &fixture.user.access_token, 0)
        .await;
    expect_session(&mut socket).await;

    socket
        .send_text(json!({ "type": "stop" }).to_string())
        .await;

    let command = tokio::time::timeout(WITHIN, owner.recv())
        .await
        .expect("the owner is told within the window")
        .expect("the registry forwarded the stop");
    assert!(
        matches!(
            command,
            OwnerCommand::Stop {
                reason: StopReason::User
            }
        ),
        "expected the stop, got {command:?}",
    );

    // The state change and the `session` snapshot are the read side's; a stop
    // itself is answered with nothing at all.
    expect_silence(&mut socket, 2 * TEST_TIMINGS.ping).await;

    socket.close().await;
}

#[tokio::test]
async fn a_stop_on_a_session_that_is_not_running_is_ignored() {
    let app = TestApp::spawn().await;
    let fixture = arrange(&app, ProfileKind::Conversational, SessionState::Parked).await;
    let _launching = app
        .session_registry()
        .try_begin_launch(fixture.session_id)
        .expect("nothing else is launching this session");

    let mut socket = app
        .ws(fixture.session_id, &fixture.user.access_token, 0)
        .await;
    expect_session(&mut socket).await;

    socket
        .send_text(json!({ "type": "stop" }).to_string())
        .await;

    // The protocol has no message for a refused stop, and `error` would close
    // the stream over a button pressed twice.
    expect_silence(&mut socket, 2 * TEST_TIMINGS.ping).await;

    // Still open, and still able to do the next thing asked of it.
    send_input(&mut socket, "client-7", "carry on").await;
    assert_eq!(next_json(&mut socket).await["type"], "input_accepted");

    socket.close().await;
}

#[tokio::test]
async fn a_revoked_login_closes_the_socket_on_its_next_input_and_forwards_nothing() {
    let app = TestApp::spawn().await;
    let fixture = arrange(&app, ProfileKind::Conversational, SessionState::Running).await;
    let mut owner = attach_owner(&app, fixture.session_id);

    let mut socket = app
        .ws(fixture.session_id, &fixture.user.access_token, 0)
        .await;
    expect_session(&mut socket).await;

    sqlx::query("UPDATE users SET auth_version = auth_version + 1 WHERE id = $1")
        .bind(fixture.user.user.id)
        .execute(&app.pool)
        .await
        .expect("the login generation moves on");

    send_input(&mut socket, "client-8", "still here?").await;

    expect_authentication_close(&mut socket).await;
    assert!(
        owner.try_recv().is_err(),
        "a revoked user's input is never forwarded",
    );
    assert_eq!(
        state_of(&app, fixture.session_id).await,
        SessionState::Running,
        "the agent session is untouched by a revoked socket",
    );
}

#[tokio::test]
async fn a_revoked_login_closes_an_idle_socket_at_the_next_ping() {
    let app = TestApp::spawn().await;
    let fixture = arrange(&app, ProfileKind::Conversational, SessionState::Running).await;
    let mut owner = attach_owner(&app, fixture.session_id);

    let mut socket = app
        .ws(fixture.session_id, &fixture.user.access_token, 0)
        .await;
    expect_session(&mut socket).await;

    sqlx::query("UPDATE users SET auth_version = auth_version + 1 WHERE id = $1")
        .bind(fixture.user.user.id)
        .execute(&app.pool)
        .await
        .expect("the login generation moves on");

    // No client traffic at all: the ping tick is what notices.
    expect_authentication_close(&mut socket).await;
    assert!(owner.try_recv().is_err(), "nothing was forwarded");
    assert_eq!(
        state_of(&app, fixture.session_id).await,
        SessionState::Running,
    );
}

#[tokio::test]
async fn a_deleted_user_closes_the_socket() {
    let app = TestApp::spawn().await;
    let fixture = arrange(&app, ProfileKind::Conversational, SessionState::Running).await;
    attach_owner(&app, fixture.session_id);

    let mut socket = app
        .ws(fixture.session_id, &fixture.user.access_token, 0)
        .await;
    expect_session(&mut socket).await;

    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(fixture.user.user.id)
        .execute(&app.pool)
        .await
        .expect("the user is deleted");

    send_input(&mut socket, "client-9", "anyone").await;

    expect_authentication_close(&mut socket).await;
}

#[tokio::test]
async fn a_password_change_gate_set_after_the_open_closes_the_socket() {
    let app = TestApp::spawn().await;
    let fixture = arrange(&app, ProfileKind::Conversational, SessionState::Running).await;
    attach_owner(&app, fixture.session_id);

    let mut socket = app
        .ws(fixture.session_id, &fixture.user.access_token, 0)
        .await;
    expect_session(&mut socket).await;

    sqlx::query("UPDATE users SET must_change_password = TRUE WHERE id = $1")
        .bind(fixture.user.user.id)
        .execute(&app.pool)
        .await
        .expect("the gate is set");

    send_input(&mut socket, "client-10", "anyone").await;

    expect_authentication_close(&mut socket).await;
}
