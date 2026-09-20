//! The MCP bearer middleware (`ARCHITECTURE.md`, "MCP design" →
//! "Authentication").
//!
//! What the suite pins is the whole gate, end to end over real HTTP: a request
//! with no usable token is refused with the API's own error body before it
//! reaches `McpServer`, a session that has ended is refused whichever method it
//! uses, a live session's token connects, and a rotation (ADR 0029) takes
//! effect on the very next request because nothing is cached.
//!
//! Rule 3 has its own scenario: the raw token and its hash appear in no log
//! line of a refused or an accepted request.
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use std::io;
use std::sync::{Arc, Mutex};

use common::TestApp;
use common::mcp::McpClient;
use mars_orchestrator::models::SessionState;
use mars_orchestrator::session::{SessionDirs, rotate_token};
use serde_json::Value;
use tempfile::TempDir;
use tracing::Level;
use tracing_subscriber::fmt::MakeWriter;

/// The `MCP_URL` a rotation writes into `mcp.json`; the default of `README.md`,
/// "Configuration".
const MCP_URL: &str = "http://orchestrator:7001/mcp";

/// A syntactically plausible token that belongs to no session (rule 3: nothing
/// here is a credential).
const UNKNOWN_TOKEN: &str = "not-a-real-session-token";

/// The body of a JSON-RPC `initialize`, for the raw-header scenarios.
const INITIALIZE: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}"#;

/// An app, a project, a profile and a live session to hold the listener open.
struct Fixture {
    app: TestApp,
    project_id: uuid::Uuid,
    profile_id: uuid::Uuid,
}

impl Fixture {
    async fn create() -> Self {
        let app = TestApp::spawn().await;
        let (project_id, profile_id) = app.seed_mcp_project().await;

        Self {
            app,
            project_id,
            profile_id,
        }
    }

    /// A seeded session in `state` and its raw token.
    async fn session(&self, state: SessionState) -> common::mcp::SeededSession {
        self.app
            .seed_mcp_session(self.project_id, self.profile_id, state)
            .await
    }

    /// A connected client, for the scenarios that need a served address rather
    /// than a successful handshake of their own.
    async fn connected(&self) -> (McpClient, common::mcp::SeededSession) {
        let seeded = self.session(SessionState::Parked).await;
        let client = McpClient::connect(&self.app, &seeded.token)
            .await
            .expect("a parked session's token authenticates");

        (client, seeded)
    }
}

/// The standard error body, as the API shapes it.
fn error_body(status: u16, message: &str) -> Value {
    serde_json::json!({ "status": status, "error": message })
}

#[tokio::test]
async fn a_request_without_an_authorization_header_is_refused_before_the_server() {
    let fixture = Fixture::create().await;
    let (client, _live) = fixture.connected().await;

    let response = client
        .raw_post(
            &[
                ("content-type", "application/json"),
                ("accept", "application/json, text/event-stream"),
            ],
            INITIALIZE,
        )
        .await;

    assert_eq!(response.status().as_u16(), 401);

    // The body is the API's error shape and not a JSON-RPC answer: nothing of
    // this request reached `McpServer`.
    let body = response.json::<Value>().await.expect("a JSON body");
    assert_eq!(body, error_body(401, "missing or invalid bearer token"));
    assert!(body.get("jsonrpc").is_none(), "{body}");
    assert!(body.get("result").is_none(), "{body}");
}

#[tokio::test]
async fn a_scheme_other_than_bearer_is_refused() {
    let fixture = Fixture::create().await;
    let (client, _live) = fixture.connected().await;

    let response = client
        .raw_post(
            &[
                ("content-type", "application/json"),
                ("accept", "application/json, text/event-stream"),
                ("authorization", "Basic xyz"),
            ],
            INITIALIZE,
        )
        .await;

    assert_eq!(response.status().as_u16(), 401);
    assert_eq!(
        response.json::<Value>().await.expect("a JSON body"),
        error_body(401, "missing or invalid bearer token")
    );
}

#[tokio::test]
async fn an_empty_bearer_credential_is_refused() {
    let fixture = Fixture::create().await;
    let (client, _live) = fixture.connected().await;

    let response = client
        .raw_post(
            &[
                ("content-type", "application/json"),
                ("accept", "application/json, text/event-stream"),
                ("authorization", "Bearer "),
            ],
            INITIALIZE,
        )
        .await;

    assert_eq!(response.status().as_u16(), 401);
    assert_eq!(
        response.json::<Value>().await.expect("a JSON body"),
        error_body(401, "missing or invalid bearer token")
    );
}

#[tokio::test]
async fn an_unknown_token_is_refused() {
    let fixture = Fixture::create().await;

    let error = McpClient::connect(&fixture.app, UNKNOWN_TOKEN)
        .await
        .err()
        .expect("a token that matches no row is not a handshake");

    assert_eq!(
        error.status().map(|status| status.as_u16()),
        Some(401),
        "{error:?}"
    );
}

#[tokio::test]
async fn a_done_session_is_forbidden() {
    let fixture = Fixture::create().await;
    let ended = fixture.session(SessionState::Done).await;

    let error = McpClient::connect(&fixture.app, &ended.token)
        .await
        .err()
        .expect("a session that has ended does not connect");

    assert_eq!(
        error.status().map(|status| status.as_u16()),
        Some(403),
        "{error:?}"
    );
}

#[tokio::test]
async fn a_done_session_is_forbidden_on_the_sse_get_too() {
    let fixture = Fixture::create().await;
    let (client, _live) = fixture.connected().await;
    let ended = fixture.session(SessionState::Done).await;

    // The stream the client opens with `GET /mcp`: the same middleware, the
    // same refusal.
    let response = reqwest::Client::new()
        .get(format!("http://{}/mcp", client.addr()))
        .header("accept", "text/event-stream")
        .header("authorization", format!("Bearer {}", ended.token))
        .send()
        .await
        .expect("the mcp listener answers");

    assert_eq!(response.status().as_u16(), 403);
    assert_eq!(
        response.json::<Value>().await.expect("a JSON body"),
        error_body(403, "session has ended")
    );
}

#[tokio::test]
async fn a_failed_session_is_forbidden() {
    let fixture = Fixture::create().await;
    let failed = fixture.session(SessionState::Failed).await;

    let error = McpClient::connect(&fixture.app, &failed.token)
        .await
        .err()
        .expect("a failed session does not connect");

    assert_eq!(
        error.status().map(|status| status.as_u16()),
        Some(403),
        "{error:?}"
    );
}

#[tokio::test]
async fn every_live_state_authenticates() {
    let fixture = Fixture::create().await;

    for state in [
        SessionState::Creating,
        SessionState::Running,
        SessionState::Parked,
    ] {
        let seeded = fixture.session(state).await;

        let client = McpClient::connect(&fixture.app, &seeded.token)
            .await
            .unwrap_or_else(|err| panic!("a {state} session authenticates: {err}"));

        // The handshake really completed: the server answered a request. What
        // it answers with is `tests/mcp_dispatch.rs`; all that matters here is
        // that a listing came back at all.
        assert!(!client.list_tools().await.is_empty());
    }
}

#[tokio::test]
async fn a_rotated_token_replaces_the_previous_one_immediately() {
    let fixture = Fixture::create().await;
    let seeded = fixture.session(SessionState::Parked).await;

    let client = McpClient::connect(&fixture.app, &seeded.token)
        .await
        .expect("the first token authenticates");
    drop(client);

    let data = TempDir::new().expect("a temporary data directory");
    let dirs = SessionDirs::for_session(data.path(), data.path(), seeded.session_id);
    dirs.ensure().await.expect("the directories are created");

    let rotated = rotate_token(&fixture.app.pool, &dirs, MCP_URL, seeded.session_id)
        .await
        .expect("a parked session rotates");

    let error = McpClient::connect(&fixture.app, &seeded.token)
        .await
        .err()
        .expect("the replaced token no longer authenticates");
    assert_eq!(
        error.status().map(|status| status.as_u16()),
        Some(401),
        "{error:?}"
    );

    McpClient::connect(&fixture.app, rotated.expose())
        .await
        .expect("the replacement token authenticates");

    // And again: a second rotation retires the second token the same way.
    let again = rotate_token(&fixture.app.pool, &dirs, MCP_URL, seeded.session_id)
        .await
        .expect("the session rotates twice");

    let error = McpClient::connect(&fixture.app, rotated.expose())
        .await
        .err()
        .expect("the second token is retired too");
    assert_eq!(
        error.status().map(|status| status.as_u16()),
        Some(401),
        "{error:?}"
    );

    McpClient::connect(&fixture.app, again.expose())
        .await
        .expect("the third token authenticates");
}

/// Rule 3: neither the raw token nor its hash reaches a log line, on the path
/// that refuses a request or on the one that accepts it.
///
/// The subscriber is installed globally because the request is served on the
/// listener's own task, which a thread-local default would not reach. Other
/// scenarios in this binary write into the same buffer; that only adds noise to
/// an assertion about what is *absent*, and the one thing asserted present is
/// this session's own id.
#[tokio::test]
async fn no_log_line_carries_the_token_or_its_hash() {
    let logs = CaptureWriter::new();
    let _ = tracing::subscriber::set_global_default(logs.subscriber());

    let fixture = Fixture::create().await;
    let seeded = fixture.session(SessionState::Parked).await;
    let hash = mars_orchestrator::session::token::hash_mcp_token(&seeded.token);

    // Refused, then accepted.
    let refused = McpClient::connect(&fixture.app, UNKNOWN_TOKEN).await;
    assert!(refused.is_err(), "an unknown token is refused");

    let client = McpClient::connect(&fixture.app, &seeded.token)
        .await
        .expect("the seeded token authenticates");
    client.list_tools().await;

    let captured = logs.contents();
    assert!(
        !captured.contains(&seeded.token),
        "the raw token reached the log"
    );
    assert!(!captured.contains(&hash), "the token hash reached the log");
    assert!(
        !captured.contains(UNKNOWN_TOKEN),
        "a refused token reached the log"
    );
    assert!(
        captured.contains(&format!("session_id={}", seeded.session_id)),
        "the accepted request logs its session id and nothing else about the token"
    );
}

/// An in-memory `tracing` writer, so a scenario can assert what was *not*
/// logged. The same shape `src/email` uses for its own redaction test.
#[derive(Clone, Default)]
struct CaptureWriter {
    buffer: Arc<Mutex<Vec<u8>>>,
}

impl CaptureWriter {
    fn new() -> Self {
        Self::default()
    }

    fn contents(&self) -> String {
        let buffer = self.buffer.lock().expect("the capture lock is healthy");
        String::from_utf8_lossy(&buffer).into_owned()
    }

    fn subscriber(&self) -> impl tracing::Subscriber + Send + Sync + 'static {
        tracing_subscriber::fmt()
            .with_ansi(false)
            .with_max_level(Level::TRACE)
            .with_writer(self.clone())
            .finish()
    }
}

impl io::Write for CaptureWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut buffer = self.buffer.lock().expect("the capture lock is healthy");
        buffer.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for CaptureWriter {
    type Writer = CaptureWriter;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}
