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

use mars_orchestrator::mcp::mcp_router;

use super::TestApp;

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
