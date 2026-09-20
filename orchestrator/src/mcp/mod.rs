//! The `rmcp` server: tool handlers and session bearer auth.
//!
//! `ARCHITECTURE.md`, "MCP design": MCP is served with `rmcp` over Streamable
//! HTTP on its own listener (`MCP_PORT`, default 7001) at the path `/mcp`, so
//! that nginx cannot accidentally expose it and a firewall rule can later
//! restrict it to the sessions network. Nothing of the public API is on this
//! router — no `/api` nesting, no JWT middleware, no CORS — because the two
//! serve different callers over different ports.

use std::sync::Arc;

use axum::Router;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};

use crate::prelude::*;

pub mod context;
pub mod server;

pub use context::SessionContext;
pub use server::McpServer;

/// The router served on `MCP_PORT`: the Streamable HTTP service at `/mcp` and
/// nothing else.
///
/// The session manager is `rmcp`'s in-memory [`LocalSessionManager`] and the
/// transport keeps its default configuration, which means stateful sessions
/// keyed by `Mcp-Session-Id`. Token rotation does not depend on that lifetime:
/// the bearer middleware authenticates every HTTP request, so a launch that
/// replaces `sessions.mcp_token_hash` invalidates the old token on the next
/// request whatever MCP session it belongs to (ADR 0029).
///
/// The factory runs once per MCP session and must stay cheap: it clones
/// [`AppState`], which is a handful of `Arc`s, and nothing else. Nothing
/// session-specific belongs on [`McpServer`]; the calling session arrives per
/// request as a [`SessionContext`].
///
/// Every other path answers 404 with the ordinary `{ "status", "error" }`
/// body, so a session container that points at the wrong URL gets the same
/// shape of answer the API gives.
pub fn mcp_router(state: AppState) -> Router {
    let service = StreamableHttpService::new(
        move || Ok(McpServer::new(state.clone())),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );

    Router::new()
        .nest_service("/mcp", service)
        .fallback(not_found)
}

/// Everything that is not `/mcp`.
async fn not_found() -> Error {
    Error::NotFound
}
