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

use std::collections::BTreeMap;
use std::net::SocketAddr;

use chrono::{DateTime, Utc};
use rmcp::ServiceExt;
use rmcp::model::{CallToolRequestParams, ErrorData, Tool};
use rmcp::service::{RoleClient, RunningService, ServiceError};
use rmcp::transport::DynamicTransportError;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransportConfig, StreamableHttpError,
};
use serde_json::Value;
use sqlx::PgPool;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use uuid::Uuid;

use mars_orchestrator::events::{TaskActor, TaskEvent, TaskEventKind};
use mars_orchestrator::git::testutil::test_identity;
use mars_orchestrator::git::{create_work_clone, resolve_base};
use mars_orchestrator::mcp::mcp_router;
use mars_orchestrator::models::{
    NewAgentProfile, NewProject, NewSession, ProfileKind, SessionState, StateChange,
};
use mars_orchestrator::repositories::{ProjectRepository, SessionRepository, TaskRepository};
use mars_orchestrator::session::initial_token;
use mars_orchestrator::tracker::{TaskDto, TrackerMutation};

use super::TestApp;
use super::handoffs::Fixture as GitFixture;

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
    ///
    /// A failure that is *not* a tool error panics, because for every scenario
    /// but the two below it is a bug rather than an expected answer; those two
    /// call [`McpClient::try_call`] instead.
    pub async fn call(&self, name: &str, args: Value) -> Result<Value, ErrorData> {
        self.try_call(name, args).await.map_err(|err| match err {
            McpCallError::Tool(error) => error,
            other => panic!("tools/call failed outside the protocol: {other}"),
        })
    }

    /// The same call, with the transport's refusals kept rather than panicked.
    ///
    /// The bearer middleware runs on every HTTP request, not only on
    /// `initialize` (`ARCHITECTURE.md`, "MCP design" → Authentication), so a
    /// token replaced by a relaunch (ADR 0029) and a session that has ended
    /// are refused *underneath* JSON-RPC, with the API's own error body and no
    /// `data.code` to read. [`McpCallError::Http`] is how a scenario asserts
    /// that 401 or 403.
    pub async fn try_call(&self, name: &str, args: Value) -> Result<Value, McpCallError> {
        let params = CallToolRequestParams::new(name.to_string()).with_arguments(
            args.as_object()
                .cloned()
                .expect("tool arguments are a JSON object"),
        );

        match self.service().call_tool(params).await {
            Ok(result) => Ok(result.structured_content.unwrap_or(Value::Null)),
            Err(ServiceError::McpError(error)) => Err(McpCallError::Tool(error)),
            Err(ServiceError::TransportSend(error)) => Err(match transport_status(&error) {
                Some(status) => McpCallError::Http(status),
                None => McpCallError::Other(error.to_string()),
            }),
            Err(other) => Err(McpCallError::Other(other.to_string())),
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
        ClientInitializeError::TransportError { error, .. } => transport_status(error),
        _ => None,
    };

    match status {
        Some(status) => McpConnectError::Http(status),
        None => McpConnectError::Other(err.to_string()),
    }
}

/// The HTTP status inside one of `rmcp`'s boxed transport errors, wherever
/// this version of the crate put it.
///
/// Shared by [`connect_error`] and [`McpClient::try_call`]: an `initialize`
/// and a `tools/call` refused by the same middleware arrive through the same
/// [`StreamableHttpError`], one wrapped in a `ClientInitializeError` and the
/// other in a `ServiceError::TransportSend`.
fn transport_status(error: &DynamicTransportError) -> Option<reqwest::StatusCode> {
    error
        .error
        .downcast_ref::<StreamableHttpError<reqwest::Error>>()
        .and_then(|transport| match transport {
            StreamableHttpError::AuthRequired(_) => Some(reqwest::StatusCode::UNAUTHORIZED),
            StreamableHttpError::InsufficientScope(_) => Some(reqwest::StatusCode::FORBIDDEN),
            StreamableHttpError::Client(client) => client.status(),
            StreamableHttpError::UnexpectedServerResponse(message) => status_in_message(message),
            _ => None,
        })
}

/// The status code in `HTTP 401 Unauthorized: <body>`.
fn status_in_message(message: &str) -> Option<reqwest::StatusCode> {
    let code = message.strip_prefix("HTTP ")?.split_whitespace().next()?;
    reqwest::StatusCode::from_u16(code.parse().ok()?).ok()
}

/// Why a `tools/call` did not answer with a result.
///
/// Three outcomes, because three layers can refuse. A tool that ran and said
/// no is [`McpCallError::Tool`], with the `data.code` the contract fixes. The
/// bearer middleware, which runs in front of `rmcp` on every request, answers
/// with an HTTP status and the API's `{ "status", "error" }` body instead —
/// that is [`McpCallError::Http`]. Anything else is kept as its message,
/// because a test that meets one has found a bug.
#[derive(Debug)]
pub enum McpCallError {
    /// The handler's own JSON-RPC error.
    Tool(ErrorData),
    /// The request never reached a handler: the middleware refused it.
    Http(reqwest::StatusCode),
    /// A protocol or connection failure.
    Other(String),
}

impl McpCallError {
    /// The status a refusal underneath JSON-RPC carried, if it was one.
    pub fn status(&self) -> Option<reqwest::StatusCode> {
        match self {
            McpCallError::Http(status) => Some(*status),
            _ => None,
        }
    }
}

impl std::fmt::Display for McpCallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            McpCallError::Tool(error) => write!(f, "tool error: {error:?}"),
            McpCallError::Http(status) => write!(f, "refused with HTTP {status}"),
            McpCallError::Other(message) => write!(f, "{message}"),
        }
    }
}

// ---- what every MCP suite asserts with ----

/// The `data.code` every tool failure carries (`SPEC.md`, "MCP tool
/// contracts": "`data.code` is always present").
#[track_caller]
pub fn code(err: &ErrorData) -> String {
    err.data
        .as_ref()
        .and_then(|data| data.get("code"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("every tool error carries data.code: {err:?}"))
        .to_string()
}

/// The `data.conflicts` a `merge` or `rebase` that stopped on paths carries,
/// in git's order.
#[track_caller]
pub fn conflicts(err: &ErrorData) -> Vec<String> {
    err.data
        .as_ref()
        .and_then(|data| data.get("conflicts"))
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("a stopped merge carries data.conflicts: {err:?}"))
        .iter()
        .map(|path| {
            path.as_str()
                .expect("a conflicting path is a string")
                .to_string()
        })
        .collect()
}

/// The failure a call answered with, or a panic naming what came back instead.
#[track_caller]
pub fn refused(result: std::result::Result<Value, ErrorData>) -> ErrorData {
    match result {
        Ok(value) => panic!("expected a refusal, got {value}"),
        Err(err) => err,
    }
}

/// The `{ task: Task }` body a tool answered with.
#[track_caller]
pub fn task_of(value: &Value) -> TaskDto {
    serde_json::from_value(value["task"].clone()).expect("the output carries a Task")
}

// ---- the ADR 0030 audit ----

/// One `task_sessions` row: which session touched which task, and when it
/// first and last did.
pub type SessionLink = (Uuid, Uuid, DateTime<Utc>, DateTime<Utc>);

/// Everything a read, a rejection or an update with no effective change must
/// leave exactly as it was (`SPEC.md`, "MCP tool contracts"; ADR 0030).
///
/// Three things, because the contract names three: the tracker history
/// (`task_events`), the sessions' links to tasks with their touch timestamps
/// (`task_sessions`), and every task's `updated_at`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackerFingerprint {
    /// How many `task_events` rows the project has.
    pub events: usize,
    /// Every `task_sessions` row of the project, ordered by task then session.
    pub links: Vec<SessionLink>,
    /// `updated_at` per task.
    pub updated_at: BTreeMap<Uuid, DateTime<Utc>>,
}

/// The project's tracker history, links and touch timestamps as they are now.
///
/// Raw SQL rather than a repository: the `task_sessions` timestamps and the
/// bare event count are row-level facts no interface answers (`CLAUDE.md`,
/// "Testing expectations").
pub async fn tracker_fingerprint(pool: &PgPool, project_id: Uuid) -> TrackerFingerprint {
    let events =
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM task_events WHERE project_id = $1")
            .bind(project_id)
            .fetch_one(pool)
            .await
            .expect("the events count") as usize;

    let links = sqlx::query_as::<_, SessionLink>(
        "SELECT ts.task_id, ts.session_id, ts.first_touched_at, ts.last_touched_at
         FROM task_sessions ts
         JOIN tasks t ON t.id = ts.task_id
         WHERE t.project_id = $1
         ORDER BY ts.task_id, ts.session_id",
    )
    .bind(project_id)
    .fetch_all(pool)
    .await
    .expect("the links read");

    let updated_at = sqlx::query_as::<_, (Uuid, DateTime<Utc>)>(
        "SELECT id, updated_at FROM tasks WHERE project_id = $1",
    )
    .bind(project_id)
    .fetch_all(pool)
    .await
    .expect("the timestamps read")
    .into_iter()
    .collect();

    TrackerFingerprint {
        events,
        links,
        updated_at,
    }
}

impl TrackerFingerprint {
    /// Nothing at all moved: the answer for every read-only call and every
    /// rejected one.
    #[track_caller]
    pub fn assert_unchanged(&self, after: &TrackerFingerprint, what: &str) {
        assert_eq!(self.events, after.events, "{what} wrote a task event");
        assert_eq!(self.links, after.links, "{what} touched task_sessions");
        assert_eq!(
            self.updated_at, after.updated_at,
            "{what} advanced a task's updated_at",
        );
    }

    /// History grew, only `changed` moved, and no link lost the moment it was
    /// first made.
    ///
    /// `changed` is a list rather than one id because the tracker's own rules
    /// carry a change outwards: a terminal state recomputes `blocked` on the
    /// dependants and can close the parent. The tasks a call does *not* reach
    /// must still be untouched, which is the half this asserts.
    #[track_caller]
    pub fn assert_changed_only(&self, after: &TrackerFingerprint, changed: &[Uuid], what: &str) {
        assert!(
            after.events > self.events,
            "{what} wrote no task event: {} then {}",
            self.events,
            after.events,
        );

        for (id, before) in &self.updated_at {
            if changed.contains(id) {
                continue;
            }
            assert_eq!(
                after.updated_at.get(id),
                Some(before),
                "{what} advanced updated_at of a task it did not change ({id})",
            );
        }

        // "with `task_sessions.first_touched_at` preserved on repeat touches":
        // a link that existed keeps the moment it was made, whatever else the
        // call did to it.
        for (task_id, session_id, first, _) in &self.links {
            let found = after
                .links
                .iter()
                .find(|(task, session, _, _)| task == task_id && session == session_id)
                .unwrap_or_else(|| {
                    panic!("{what} removed the link of task {task_id} and session {session_id}")
                });
            assert_eq!(
                &found.2, first,
                "{what} moved first_touched_at of task {task_id} / session {session_id}",
            );
        }
    }
}

// ---- the four-profile workflow fixture ----

/// One agent in the workflow: its profile, its session and the client that
/// speaks as it.
pub struct Role {
    /// What the scenario calls this agent, for assertion messages.
    pub name: &'static str,
    pub profile_id: Uuid,
    pub session_id: Uuid,
    pub client: McpClient,
}

impl Role {
    /// `tools/call` as this agent.
    pub async fn call(&self, tool: &str, args: Value) -> Result<Value, ErrorData> {
        self.client.call(tool, args).await
    }

    /// The tasks this agent's profile could claim now, in the order `ready`
    /// answered with.
    pub async fn ready(&self) -> Vec<Value> {
        let output = self
            .call("ready", serde_json::json!({}))
            .await
            .unwrap_or_else(|err| panic!("{} lists its queue: {err:?}", self.name));

        output["tasks"]
            .as_array()
            .expect("the output is { tasks }")
            .clone()
    }
}

/// The git tools the merger may call (`SPEC.md`, "MCP tool contracts": the
/// git tools are profile-gated).
pub const MERGER_TOOLS: &[&str] = &["list_session_branches", "merge", "push"];

/// The four profiles of `ARCHITECTURE.md`, "Task tracker" → "State is a
/// queue", each with a live session and a connected client, over a project
/// with a real repository and the documented default states.
///
/// The planner serves `backlog` and hands to `ready`; the implementer serves
/// `ready` and hands to `review`; the reviewer serves `review` and hands to
/// `merge` or back to `ready`; the merger serves `merge` and closes. Only the
/// merger's profile names git tools, and only the implementer has a work clone
/// to commit in.
pub struct WorkflowFixture {
    pub git: GitFixture,
    pub planner: Role,
    pub implementer: Role,
    pub reviewer: Role,
    pub merger: Role,
}

impl WorkflowFixture {
    pub async fn create(name: &str) -> WorkflowFixture {
        let git = GitFixture::create(name).await;

        let planner = role(&git, "planner", "backlog", &[]).await;
        let implementer = role(&git, "implementer", "ready", &[]).await;
        let reviewer = role(&git, "reviewer", "review", &[]).await;
        let merger = role(&git, "merger", "merge", MERGER_TOOLS).await;

        // Only the implementer commits, so only the implementer needs a
        // checkout; the reviewer and the merger work from the retained
        // hand-off refs.
        work_clone(&git, implementer.session_id).await;

        WorkflowFixture {
            git,
            planner,
            implementer,
            reviewer,
            merger,
        }
    }

    pub fn app(&self) -> &TestApp {
        &self.git.app
    }

    pub fn pool(&self) -> &PgPool {
        &self.git.app.pool
    }

    pub fn project_id(&self) -> Uuid {
        self.git.project.id
    }

    /// A second live session of `profile_id`, with a client of its own.
    ///
    /// The lost-claim scenario needs two *connections*, not two calls on one:
    /// a lease is taken per session, and one client could never lose that race
    /// to itself.
    pub async fn second_session(&self, profile_id: Uuid) -> (McpClient, Uuid) {
        let seeded = self
            .git
            .app
            .seed_mcp_session(self.project_id(), profile_id, SessionState::Running)
            .await;
        let client = McpClient::connect(&self.git.app, &seeded.token)
            .await
            .expect("a running session's token authenticates");

        (client, seeded.session_id)
    }

    /// The project's tracker history, links and timestamps right now.
    pub async fn fingerprint(&self) -> TrackerFingerprint {
        tracker_fingerprint(self.pool(), self.project_id()).await
    }

    /// The task as it is committed, by id.
    pub async fn task(&self, task_id: Uuid) -> TaskDto {
        TaskRepository::new(self.pool())
            .load_task_dto(self.project_id(), task_id)
            .await
            .expect("the task reads")
            .expect("the task is in this project")
    }

    /// Every committed event of the project, oldest first.
    pub async fn events(&self) -> Vec<TaskEvent> {
        TaskRepository::new(self.pool())
            .list_task_events_after(self.project_id(), 0, 500)
            .await
            .expect("the events read")
            .into_iter()
            .map(|row| TaskEvent::from_row(row).expect("the row is a documented event"))
            .collect()
    }

    /// The events of one task, oldest first.
    pub async fn events_of(&self, task_id: Uuid) -> Vec<TaskEvent> {
        self.events()
            .await
            .into_iter()
            .filter(|event| event.task_id == Some(task_id))
            .collect()
    }

    /// One commit in the implementer's work clone, not fetched back: what an
    /// agent leaves behind before it hands off.
    pub async fn implementer_commit(&self, file: &str, content: &str) -> String {
        self.git
            .commit_in_work_clone(self.implementer.session_id, file, content)
            .await
    }

    /// Replace a profile's `mcp_tools` the way an operator editing the profile
    /// would, without reconnecting anything.
    ///
    /// Raw SQL on purpose: the profile is read per request by the bearer
    /// middleware, and the point of the scenario is that the *next* call sees
    /// the new list.
    pub async fn set_mcp_tools(&self, profile_id: Uuid, tools: &[&str]) {
        let tools: Vec<String> = tools.iter().map(|tool| (*tool).to_string()).collect();
        sqlx::query("UPDATE agent_profiles SET mcp_tools = $2 WHERE id = $1")
            .bind(profile_id)
            .bind(&tools)
            .execute(self.pool())
            .await
            .expect("the profile's tools are replaced");
    }

    /// Walk a session to `state` along the documented lifecycle edges.
    pub async fn set_session_state(&self, session_id: Uuid, state: SessionState) {
        let mut tx = self.pool().begin().await.expect("a transaction begins");
        SessionRepository::new(self.pool())
            .set_state(&mut tx, session_id, state, &StateChange::plain())
            .await
            .expect("the transition is a documented edge");
        tx.commit().await.expect("the transaction commits");
    }

    /// Every path in a ref's tree in the project repository: how "`main` has
    /// the implementer's commit" is asserted by content.
    pub async fn files_in(&self, reference: &str) -> Vec<String> {
        self.run_git(&["ls-tree", "--name-only", "-r", reference])
            .await
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// The commit a ref in the project repository points at.
    pub async fn commit_of(&self, reference: &str) -> String {
        self.run_git(&["rev-parse", reference]).await.trim().into()
    }

    async fn run_git(&self, args: &[&str]) -> String {
        mars_orchestrator::git::testutil::run_git(
            &self.git.paths().project_repo(self.project_id()),
            args,
        )
        .await
    }
}

/// One profile serving exactly `serves`, with a live session and a client.
async fn role(git: &GitFixture, name: &'static str, serves: &str, mcp_tools: &[&str]) -> Role {
    let project_id = git.project.id;
    let profile_id = git.app.seed_mcp_profile(project_id, mcp_tools).await;

    let mut mutation = TrackerMutation::begin(&git.app.pool, project_id, TaskActor::System)
        .await
        .expect("the mutation opens");
    TaskRepository::new(&git.app.pool)
        .set_profile_states_by_name(
            mutation.conn(),
            project_id,
            profile_id,
            &[serves.to_string()],
        )
        .await
        .expect("the profile serves its queue");
    mutation.commit().await.expect("the mutation commits");

    let seeded = git
        .app
        .seed_mcp_session(project_id, profile_id, SessionState::Running)
        .await;
    let client = McpClient::connect(&git.app, &seeded.token)
        .await
        .expect("a running session's token authenticates");

    Role {
        name,
        profile_id,
        session_id: seeded.session_id,
        client,
    }
}

/// A work clone made from `main`, as a launching session's would be.
async fn work_clone(git: &GitFixture, session_id: Uuid) {
    let guard = git.guard().await;
    let paths = git.paths();
    let base = resolve_base(&guard, &paths, None, "main")
        .await
        .expect("the base resolves");
    create_work_clone(&guard, &paths, session_id, &base, &test_identity())
        .await
        .expect("the work clone is created");
}

/// The kinds of a run of events, which is usually the whole assertion.
pub fn kinds(events: &[TaskEvent]) -> Vec<TaskEventKind> {
    events.iter().map(|event| event.kind).collect()
}
