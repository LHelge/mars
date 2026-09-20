//! Driving the MCP server in-process (`CLAUDE.md`, "Testing expectations":
//! MCP tests drive the tool handlers through the `rmcp` server in-process with
//! a session bearer token from `TestApp`).
//!
//! The MCP listener is not part of the `axum-test` server a scenario usually
//! drives: it is its own router on its own port with none of the API's
//! middleware (`ARCHITECTURE.md`, "MCP design"). So this helper binds
//! `mcp_router` on an ephemeral loopback port, serves it on a background task
//! for the lifetime of the [`McpClient`], and connects an `rmcp` Streamable
//! HTTP client to it. What a session container does over the sessions network
//! is then exactly what a test does over loopback, down to the bearer header.
//!
//! [`McpClient::raw_post`] is the way out of the `rmcp` client for the
//! scenarios that need to send something the client would never produce — a
//! body that is not JSON-RPC, a missing header — and [`McpConnectError`] is
//! the way the authentication scenarios read the status of a refused
//! `initialize`.

use std::net::SocketAddr;

use rmcp::ServiceExt;
use rmcp::model::{CallToolRequestParams, ErrorData, Tool};
use rmcp::service::{RoleClient, RunningService, ServiceError};
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransportConfig, StreamableHttpError,
};
use serde_json::Value;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use uuid::Uuid;

use mars_orchestrator::mcp::mcp_router;
use mars_orchestrator::models::{
    NewAgentProfile, NewProject, NewSession, ProfileKind, SessionState, StateChange,
};
use mars_orchestrator::repositories::{ProjectRepository, SessionRepository};
use mars_orchestrator::session::initial_token;

use super::TestApp;

/// Not a real remote: the fixture value every project test stores (rule 3).
const TEST_REMOTE: &str = "https://git.example.com/fake/repo.git";

/// A session row and the raw bearer token that authenticates as it.
///
/// The raw token exists here and nowhere else in the process: production code
/// keeps only the SHA-256 (`docs/data-model.md`, `sessions.mcp_token_hash`), so
/// a test that wants to *present* a token has to be handed one at the moment it
/// is generated.
pub struct SeededSession {
    pub session_id: Uuid,
    pub token: String,
}

impl TestApp {
    /// A project and an agent profile to hang MCP sessions off.
    ///
    /// No git repository and no default task states: the bearer middleware
    /// reads the session row and the profile, and nothing else.
    pub async fn seed_mcp_project(&self) -> (Uuid, Uuid) {
        let projects = ProjectRepository::new(&self.pool);

        let name = format!("mcp-{}", &Uuid::new_v4().simple().to_string()[..8]);
        let new_project = NewProject::new(&name, TEST_REMOTE).expect("the project is valid");

        let mut tx = self.pool.begin().await.expect("a transaction begins");
        let project = projects
            .insert(&mut tx, &new_project)
            .await
            .expect("the project inserts");
        let profile = projects
            .insert_profile(
                &mut tx,
                &NewAgentProfile::new(project.id, "default", "localhost/mars-session:test")
                    .expect("the profile is valid"),
            )
            .await
            .expect("the profile inserts");
        tx.commit().await.expect("the transaction commits");

        (project.id, profile.id)
    }

    /// A second profile in `project_id` whose `mcp_tools` are exactly these.
    ///
    /// What the dispatch suite varies: the git tools a session may call are the
    /// ones its profile names (`docs/data-model.md`,
    /// `agent_profiles.mcp_tools`). The names go through
    /// [`NewAgentProfile::validate`] like any other profile's, so a test cannot
    /// seed a set the API would refuse.
    pub async fn seed_mcp_profile(&self, project_id: Uuid, mcp_tools: &[&str]) -> Uuid {
        let name = format!("profile-{}", &Uuid::new_v4().simple().to_string()[..8]);
        let mut profile = NewAgentProfile::new(project_id, &name, "localhost/mars-session:test")
            .expect("the profile is valid");
        profile.mcp_tools = mcp_tools.iter().map(|tool| (*tool).to_string()).collect();
        profile.validate().expect("the tool names are known ones");

        let mut tx = self.pool.begin().await.expect("a transaction begins");
        let inserted = ProjectRepository::new(&self.pool)
            .insert_profile(&mut tx, &profile)
            .await
            .expect("the profile inserts");
        tx.commit().await.expect("the transaction commits");

        inserted.id
    }

    /// A session in `state`, with a freshly generated bearer token.
    ///
    /// The row is inserted through [`SessionRepository`] with
    /// `mcp_token_hash = token.hash()`, exactly as a real launch does (ADR
    /// 0029), and then walked to `state` along the documented lifecycle edges.
    /// The returned raw token is the test's copy of what a session container
    /// would read out of its `mcp.json`.
    pub async fn seed_mcp_session(
        &self,
        project_id: Uuid,
        profile_id: Uuid,
        state: SessionState,
    ) -> SeededSession {
        let token = initial_token();

        let session = NewSession::new(
            project_id,
            profile_id,
            ProfileKind::Conversational,
            "main",
            token.hash(),
        );

        let repository = SessionRepository::new(&self.pool);
        let mut tx = self.pool.begin().await.expect("a transaction begins");
        let inserted = repository
            .insert(&mut tx, &session)
            .await
            .expect("the session inserts");
        tx.commit().await.expect("the transaction commits");

        for step in lifecycle_path(state) {
            let change = if step == SessionState::Failed {
                StateChange::failed("seeded")
            } else {
                StateChange::plain()
            };
            let mut tx = self.pool.begin().await.expect("a transaction begins");
            repository
                .set_state(&mut tx, inserted.id, step, &change)
                .await
                .expect("the transition is a documented edge");
            tx.commit().await.expect("the transaction commits");
        }

        SeededSession {
            session_id: inserted.id,
            token: token.expose().to_string(),
        }
    }
}

/// The documented edges from `creating` to `state` (`ARCHITECTURE.md`, "Session
/// lifecycle"). A session is inserted `creating`, so that one needs no step.
fn lifecycle_path(state: SessionState) -> Vec<SessionState> {
    use SessionState::*;

    match state {
        Creating => vec![],
        Running => vec![Running],
        Parked => vec![Running, Parked],
        Done => vec![Running, Done],
        Failed => vec![Failed],
    }
}

/// Why `initialize` did not succeed.
///
/// The variant a test almost always wants is [`McpConnectError::Http`]: the
/// bearer middleware refused the request and the scenario asserts on 401 or
/// 403. Everything else the `rmcp` client can fail with is kept as its
/// message, because a test that hits one has found a bug rather than an
/// expected answer.
#[derive(Debug)]
pub enum McpConnectError {
    /// The server answered the `initialize` POST with this status.
    Http(reqwest::StatusCode),
    /// Anything else: a protocol mismatch, a closed connection, a body that
    /// could not be parsed.
    Other(String),
}

impl McpConnectError {
    /// The HTTP status the `initialize` request was refused with, if it was
    /// refused with one.
    pub fn status(&self) -> Option<reqwest::StatusCode> {
        match self {
            McpConnectError::Http(status) => Some(*status),
            McpConnectError::Other(_) => None,
        }
    }
}

impl std::fmt::Display for McpConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            McpConnectError::Http(status) => write!(f, "initialize refused with HTTP {status}"),
            McpConnectError::Other(message) => write!(f, "initialize failed: {message}"),
        }
    }
}

/// An `rmcp` client and the server it is talking to.
///
/// Field order is drop order: the client service goes before the task that
/// serves it, so the session's `DELETE` has somewhere to land before the
/// listener is aborted.
pub struct McpClient {
    service: Option<RunningService<RoleClient, ()>>,
    server: JoinHandle<()>,
    addr: SocketAddr,
    http: reqwest::Client,
}

impl Drop for McpClient {
    /// Stop serving. `Drop` is not async, so the task is aborted rather than
    /// drained; nothing outlives the scenario that owns the client.
    fn drop(&mut self) {
        drop(self.service.take());
        self.server.abort();
    }
}

impl McpClient {
    /// Serve this app's MCP router on loopback and connect to it as a session
    /// holding `token`.
    pub async fn connect(app: &TestApp, token: &str) -> Result<McpClient, McpConnectError> {
        Self::connect_to_router(mcp_router(app.state.clone()), token).await
    }

    /// The same, over a router the caller built.
    ///
    /// The one caller that needs it is the scenario that checks how a refused
    /// `initialize` is reported, which has to answer 401 before the real
    /// middleware exists to do it.
    pub async fn connect_to_router(
        router: axum::Router,
        token: &str,
    ) -> Result<McpClient, McpConnectError> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("an ephemeral loopback port is free");
        let addr = listener
            .local_addr()
            .expect("a bound listener has an address");

        let server = tokio::spawn(async move {
            // The scenario's assertions are what fail when this stops early;
            // there is nothing here that could act on the error.
            let _ = axum::serve(listener, router).await;
        });

        // The bearer token goes in the transport config rather than in
        // `custom_headers`, so `rmcp` sends it on the POST, on the SSE GET and
        // on the session `DELETE` alike — which is what a session container's
        // `mcp.json` header does too.
        let transport = StreamableHttpClientTransport::with_client(
            reqwest::Client::new(),
            StreamableHttpClientTransportConfig::with_uri(format!("http://{addr}/mcp"))
                .auth_header(token),
        );

        let service = match ().serve(transport).await {
            Ok(service) => service,
            Err(err) => {
                server.abort();
                return Err(connect_error(&err));
            }
        };

        Ok(McpClient {
            service: Some(service),
            server,
            addr,
            http: reqwest::Client::new(),
        })
    }

    /// The address the MCP router is served on.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// `tools/list`, every page of it.
    pub async fn list_tools(&self) -> Vec<Tool> {
        self.service()
            .list_all_tools()
            .await
            .expect("tools/list answers")
    }

    /// `tools/call`: the tool's structured result, or the JSON-RPC error.
    pub async fn call(&self, name: &str, args: Value) -> Result<Value, ErrorData> {
        let params = CallToolRequestParams::new(name.to_string()).with_arguments(
            args.as_object()
                .cloned()
                .expect("tool arguments are a JSON object"),
        );

        match self.service().call_tool(params).await {
            Ok(result) => Ok(result.structured_content.unwrap_or(Value::Null)),
            Err(ServiceError::McpError(error)) => Err(error),
            Err(other) => panic!("tools/call failed outside the protocol: {other}"),
        }
    }

    /// One POST to `/mcp` with exactly these headers and this body.
    ///
    /// For the scenarios the `rmcp` client cannot express: a malformed body, a
    /// missing `Accept`, an absent `Authorization`.
    pub async fn raw_post(&self, headers: &[(&str, &str)], body: &str) -> reqwest::Response {
        let mut request = self
            .http
            .post(format!("http://{}/mcp", self.addr))
            .body(body.to_string());
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        request.send().await.expect("the mcp listener answers")
    }

    fn service(&self) -> &RunningService<RoleClient, ()> {
        self.service
            .as_ref()
            .expect("the service is taken only by Drop")
    }
}

/// Read the HTTP status out of a failed `initialize`, if it failed with one.
///
/// `rmcp` wraps the transport's own error in a `DynamicTransportError`, which
/// keeps it as a boxed `dyn Error` rather than in the source chain, so it has
/// to be downcast to the concrete transport error before it can be read.
///
/// What the pinned `rmcp` (3.4.0) then hands over, verified by
/// `tests/mcp_server.rs`: a refusal without a `WWW-Authenticate` header — which
/// is every refusal Mars produces, since its error body is the ordinary
/// `{ "status", "error" }` — arrives as
/// `UnexpectedServerResponse("HTTP <status> <reason>: <body>")`, with the
/// status only in that message. Parsing it is the only channel this version
/// offers; the scenario in `tests/mcp_server.rs` is what fails first if a
/// version bump rewords it. The other two arms are `rmcp`'s typed paths, kept
/// because they are what a server that *does* send a challenge produces.
fn connect_error(err: &rmcp::service::ClientInitializeError) -> McpConnectError {
    use rmcp::service::ClientInitializeError;

    let status = match err {
        ClientInitializeError::TransportError { error, .. } => error
            .error
            .downcast_ref::<StreamableHttpError<reqwest::Error>>()
            .and_then(|transport| match transport {
                StreamableHttpError::AuthRequired(_) => Some(reqwest::StatusCode::UNAUTHORIZED),
                StreamableHttpError::InsufficientScope(_) => Some(reqwest::StatusCode::FORBIDDEN),
                StreamableHttpError::Client(client) => client.status(),
                StreamableHttpError::UnexpectedServerResponse(message) => {
                    status_in_message(message)
                }
                _ => None,
            }),
        _ => None,
    };

    match status {
        Some(status) => McpConnectError::Http(status),
        None => McpConnectError::Other(err.to_string()),
    }
}

/// The status code in `HTTP 401 Unauthorized: <body>`.
fn status_in_message(message: &str) -> Option<reqwest::StatusCode> {
    let code = message.strip_prefix("HTTP ")?.split_whitespace().next()?;
    reqwest::StatusCode::from_u16(code.parse().ok()?).ok()
}
