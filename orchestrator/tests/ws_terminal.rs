//! The terminal multiplexed onto the session socket (`SPEC.md`, "WebSocket:
//! session stream", the terminal and "Authentication"; ADR 0025).
//!
//! What is asserted here is the whole round trip a unit test cannot see: that
//! `terminal_open` reaches the engine as the documented exec — `/bin/bash -l`
//! as uid 1000 at the client's size — that PTY output comes back as binary
//! frames and client binary frames reach the PTY, that a resize is applied
//! with the client's zero clamped, and that the exec is disposed of on every
//! way out of the socket, including a revoked login.
//!
//! The exec is `MockEngine`'s: it reads out what the test scripted, echoes
//! what is written to it and records its resizes and its close, which is
//! exactly the surface the socket uses.
//!
//! The periods are `common::TEST_TIMINGS` (200 ms ping), not the documented
//! thirty seconds.
//!
//! Needs a container engine; see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use std::collections::BTreeMap;
use std::time::Duration;

use axum_test::{TestWebSocket, WsMessage};
use bytes::Bytes;
use common::{AuthenticatedUser, TestApp};
use mars_orchestrator::engine::{ContainerEngine, ContainerId, ContainerSpec, LABEL_SESSION_ID};
use mars_orchestrator::models::{NewSession, ProfileKind, SessionState};
use mars_orchestrator::repositories::SessionRepository;
use serde_json::{Value, json};
use uuid::Uuid;

/// How long a frame that should already be on its way is waited for.
const WITHIN: Duration = Duration::from_secs(2);

/// How long a disposal the socket does on its way out is waited for. The
/// document's own bound on a terminal close is five seconds; a mock exec takes
/// none of it.
const DISPOSED_WITHIN: Duration = Duration::from_secs(1);

/// An obviously fake password, long enough for `POST /api/test/users`
/// (rule 3).
const PASSWORD: &str = "not-a-real-password";

/// The close code a revoked stream is closed with (`SPEC.md`,
/// "Authentication").
const POLICY_VIOLATION: u16 = 1008;

/// A signed-in user, a session and the container its terminal would exec into.
struct Fixture {
    user: AuthenticatedUser,
    session_id: Uuid,
    container: ContainerId,
}

/// A user, a project, a profile and one session in `state` whose
/// `container_id` names a running mock container.
///
/// The project and profile go in with unchecked statements, the way the other
/// socket suites seed them: what this file is about starts at the session.
async fn arrange(app: &TestApp, state: SessionState) -> Fixture {
    let suffix = &Uuid::new_v4().simple().to_string()[..8];
    let user = app
        .create_user(
            &format!("viewer-{suffix}"),
            &format!("viewer-{suffix}@example.test"),
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
    .bind(ProfileKind::Conversational)
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

    let container = running_container(app, inserted.id).await;

    // Straight into the state under test with the container the launcher would
    // have recorded: how a session gets there is the lifecycle's own suite.
    sqlx::query("UPDATE sessions SET state = $1, container_id = $2 WHERE id = $3")
        .bind(state)
        .bind(&container.0)
        .bind(inserted.id)
        .execute(&app.pool)
        .await
        .expect("the session takes its state");

    Fixture {
        user,
        session_id: inserted.id,
        container,
    }
}

/// One running mock container labelled for this session, the way a launched
/// session's is.
async fn running_container(app: &TestApp, session_id: Uuid) -> ContainerId {
    let spec = ContainerSpec {
        image: "localhost/mars-session:test".to_string(),
        name: format!("mars-session-{session_id}"),
        labels: BTreeMap::from([(LABEL_SESSION_ID.to_string(), session_id.to_string())]),
        user: "1000:1000".to_string(),
        working_dir: "/session/work".to_string(),
        cmd: vec!["claude".to_string()],
        env: Vec::new(),
        secret_env: Vec::new(),
        binds: Vec::new(),
        network: "mars-sessions".to_string(),
        extra_hosts: Vec::new(),
        runtime: None,
    };

    let engine = app.engine();
    let id = engine.create(&spec).await.expect("the mock creates");
    engine.start(&id).await.expect("the mock starts");

    id
}

/// The next JSON frame, ignoring the pings the server sends underneath and
/// any binary frame still on its way.
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

/// The next binary frame, ignoring the pings underneath.
async fn next_binary(socket: &mut TestWebSocket) -> Bytes {
    let frame = tokio::time::timeout(WITHIN, async {
        loop {
            match socket.receive_message().await {
                WsMessage::Binary(bytes) => return bytes,
                WsMessage::Ping(_) | WsMessage::Pong(_) => {}
                other => panic!("expected a binary frame, got {other:?}"),
            }
        }
    })
    .await;

    frame.expect("a binary frame arrives within the window")
}

/// The `session` snapshot every stream opens with, consumed and discarded.
async fn expect_session(socket: &mut TestWebSocket) {
    let frame = next_json(socket).await;
    assert_eq!(frame["type"], "session", "the first frame is the snapshot");
}

/// Send one `terminal_open` the way a browser does.
async fn open_terminal(socket: &mut TestWebSocket, cols: u16, rows: u16) {
    socket
        .send_text(json!({ "type": "terminal_open", "cols": cols, "rows": rows }).to_string())
        .await;
}

/// Wait for the mock to report the exec closed, or fail rather than hang.
async fn expect_exec_closed(app: &TestApp, container: &ContainerId) {
    tokio::time::timeout(DISPOSED_WITHIN, async {
        while !app.engine().exec_closed(container) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the terminal exec is closed within the window");
}

/// Revoke every session of this user, the way a password change or a
/// deactivation does.
async fn revoke(app: &TestApp, user: &AuthenticatedUser) {
    sqlx::query("UPDATE users SET auth_version = auth_version + 1 WHERE id = $1")
        .bind(user.user.id)
        .execute(&app.pool)
        .await
        .expect("the user is revoked");
}

#[tokio::test]
async fn a_terminal_execs_the_documented_shell_and_carries_bytes_both_ways() {
    let app = TestApp::spawn().await;
    let fixture = arrange(&app, SessionState::Running).await;
    app.engine()
        .script_exec_output(&fixture.container, Bytes::from_static(b"agent@mars:~$ "));

    let mut socket = app
        .ws(fixture.session_id, &fixture.user.access_token, 0)
        .await;
    expect_session(&mut socket).await;

    open_terminal(&mut socket, 100, 40).await;

    assert_eq!(
        next_binary(&mut socket).await,
        Bytes::from_static(b"agent@mars:~$ "),
        "the PTY's output reaches the client verbatim, in a binary frame",
    );

    let requests = app.engine().exec_requests();
    assert_eq!(requests.len(), 1, "one exec, for the one terminal");
    assert_eq!(requests[0].container, fixture.container);
    assert_eq!(
        requests[0].cmd,
        vec!["/bin/bash".to_string(), "-l".to_string()],
        "the documented login shell",
    );
    assert_eq!(
        requests[0].user, "1000:1000",
        "the numeric uid:gid of the image's `agent` user",
    );
    assert_eq!((requests[0].cols, requests[0].rows), (100, 40));

    // The write path: what the client types reaches the PTY, which the mock
    // exec proves by echoing it back.
    socket
        .send_message(WsMessage::Binary(Bytes::from_static(b"ls\n")))
        .await;
    assert_eq!(
        next_binary(&mut socket).await,
        Bytes::from_static(b"ls\n"),
        "a client binary frame reaches the PTY",
    );

    // A window with no columns is not a window: the zero is clamped before the
    // engine ever sees it.
    socket
        .send_text(json!({ "type": "terminal_resize", "cols": 0, "rows": 50 }).to_string())
        .await;
    tokio::time::timeout(WITHIN, async {
        while app.engine().exec_resizes(&fixture.container).is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the resize is applied within the window");
    assert_eq!(
        app.engine().exec_resizes(&fixture.container),
        vec![(1, 50)],
        "a zero is clamped to one",
    );

    socket
        .send_text(json!({ "type": "terminal_close" }).to_string())
        .await;
    let frame = next_json(&mut socket).await;
    assert_eq!(frame["type"], "terminal_closed", "{frame}");
    assert_eq!(frame["exit_code"], 0, "the mock's shell exits cleanly");
    assert!(app.engine().exec_closed(&fixture.container));

    // The socket survived the close, and a second terminal is a second exec.
    open_terminal(&mut socket, 80, 24).await;
    tokio::time::timeout(WITHIN, async {
        while app.engine().exec_requests().len() < 2 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("a second terminal opens within the window");
    assert_eq!(
        app.engine().exec_requests()[1].cols,
        80,
        "the second terminal is a fresh shell at its own size",
    );

    socket.close().await;
}

#[tokio::test]
async fn a_terminal_on_a_session_that_is_not_running_is_answered_and_never_execs() {
    let app = TestApp::spawn().await;
    let fixture = arrange(&app, SessionState::Parked).await;

    let mut socket = app
        .ws(fixture.session_id, &fixture.user.access_token, 0)
        .await;
    expect_session(&mut socket).await;

    open_terminal(&mut socket, 100, 40).await;

    let frame = next_json(&mut socket).await;
    assert_eq!(frame["type"], "terminal_closed", "{frame}");
    assert_eq!(
        frame["exit_code"], -1,
        "a terminal that cannot be opened reports -1",
    );
    assert!(
        app.engine().exec_requests().is_empty(),
        "a parked session's container is never exec'd into",
    );

    // The socket is still there: `error` would have closed it.
    socket
        .send_text(json!({ "type": "terminal_close" }).to_string())
        .await;
    open_terminal(&mut socket, 100, 40).await;
    let frame = next_json(&mut socket).await;
    assert_eq!(frame["type"], "terminal_closed", "{frame}");

    socket.close().await;
}

#[tokio::test]
async fn a_container_that_stops_under_an_open_terminal_ends_it() {
    let app = TestApp::spawn().await;
    let fixture = arrange(&app, SessionState::Running).await;

    let mut socket = app
        .ws(fixture.session_id, &fixture.user.access_token, 0)
        .await;
    expect_session(&mut socket).await;

    open_terminal(&mut socket, 100, 40).await;
    tokio::time::timeout(WITHIN, async {
        while app.engine().exec_requests().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the terminal opens within the window");

    assert!(app.engine().exit(&fixture.container, 0), "the container ends");

    let frame = next_json(&mut socket).await;
    assert_eq!(frame["type"], "terminal_closed", "{frame}");
    assert!(
        app.engine().exec_closed(&fixture.container),
        "the exec is closed, not merely forgotten",
    );

    socket.close().await;
}

#[tokio::test]
async fn a_revoked_login_closes_the_socket_and_disposes_of_the_terminal() {
    let app = TestApp::spawn().await;
    let fixture = arrange(&app, SessionState::Running).await;

    let mut socket = app
        .ws(fixture.session_id, &fixture.user.access_token, 0)
        .await;
    expect_session(&mut socket).await;

    open_terminal(&mut socket, 100, 40).await;
    tokio::time::timeout(WITHIN, async {
        while app.engine().exec_requests().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the terminal opens within the window");

    revoke(&app, &fixture.user).await;

    // Terminal bytes are an application message: the check runs before them.
    socket
        .send_message(WsMessage::Binary(Bytes::from_static(b"whoami\n")))
        .await;

    let frame = next_json(&mut socket).await;
    assert_eq!(frame["type"], "error", "the close is announced: {frame}");
    assert_eq!(frame["message"], "authentication required");

    let close = tokio::time::timeout(WITHIN, socket.receive_message())
        .await
        .expect("the close frame arrives within the window");
    match close {
        WsMessage::Close(Some(frame)) => {
            assert_eq!(u16::from(frame.code), POLICY_VIOLATION);
            assert_eq!(frame.reason.as_str(), "authentication required");
        }
        other => panic!("expected a close frame, got {other:?}"),
    }

    expect_exec_closed(&app, &fixture.container).await;
}

#[tokio::test]
async fn a_client_that_goes_away_takes_its_terminal_with_it() {
    let app = TestApp::spawn().await;
    let fixture = arrange(&app, SessionState::Running).await;

    let mut socket = app
        .ws(fixture.session_id, &fixture.user.access_token, 0)
        .await;
    expect_session(&mut socket).await;

    open_terminal(&mut socket, 100, 40).await;
    tokio::time::timeout(WITHIN, async {
        while app.engine().exec_requests().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the terminal opens within the window");

    socket.close().await;

    expect_exec_closed(&app, &fixture.container).await;
}
