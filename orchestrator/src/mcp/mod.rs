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
use axum::http::Uri;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};

use crate::prelude::*;

pub mod auth;
pub mod context;
pub mod error;
pub mod server;
pub mod tools;

pub use auth::require_session;
pub use context::SessionContext;
pub use error::{McpError, McpErrorCode, McpResult};
pub use server::McpServer;
pub use tools::ToolName;

/// The router served on `MCP_PORT`: the Streamable HTTP service at `/mcp` and
/// nothing else.
///
/// The session manager is `rmcp`'s in-memory [`LocalSessionManager`] and the
/// transport keeps its default configuration but for the hosts it answers to
/// ([`allowed_hosts`]), which means stateful sessions keyed by
/// `Mcp-Session-Id`. Token rotation does not depend on that lifetime:
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
///
/// [`require_session`] is installed with `route_layer` rather than `layer`, so
/// it runs on `/mcp` — on its `POST`, its SSE `GET` and its session `DELETE`
/// alike — and not on the fallback: a wrong path is `not found` whether or not
/// the caller brought a token, which is the more useful answer and leaks
/// nothing either way.
pub fn mcp_router(state: AppState) -> Router {
    let factory_state = state.clone();
    let service = StreamableHttpService::new(
        move || Ok(McpServer::new(factory_state.clone())),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default()
            .with_allowed_hosts(allowed_hosts(&state.config.mcp_url)),
    );

    Router::new()
        .nest_service("/mcp", service)
        .route_layer(axum::middleware::from_fn_with_state(state, require_session))
        .fallback(not_found)
}

/// The `Host` values the transport answers to: `rmcp`'s loopback default and
/// the authority of `MCP_URL`.
///
/// `rmcp` refuses any other `Host` with 403 as its guard against DNS
/// rebinding, and its default knows only loopback names. A session container
/// reaches this listener under the authority written into its MCP config,
/// which is `MCP_URL`'s — `orchestrator:7001` under compose — so without that
/// entry every session is refused before its first tool call
/// (`ARCHITECTURE.md`, "MCP design"). A URL that names a port admits that port
/// only; one that names none admits the host on any.
fn allowed_hosts(mcp_url: &str) -> Vec<String> {
    let mut hosts = StreamableHttpServerConfig::default().allowed_hosts;
    let authority = mcp_url
        .parse::<Uri>()
        .ok()
        .and_then(|uri| uri.authority().cloned());
    if let Some(authority) = authority {
        let entry = match authority.port_u16() {
            Some(port) => format!("{}:{port}", authority.host()),
            None => authority.host().to_string(),
        };
        if !hosts.contains(&entry) {
            hosts.push(entry);
        }
    }
    hosts
}

/// Everything that is not `/mcp`.
async fn not_found() -> Error {
    Error::NotFound
}

#[cfg(test)]
mod tests {
    use super::allowed_hosts;

    #[test]
    fn the_mcp_url_authority_joins_the_loopback_defaults() {
        let hosts = allowed_hosts("http://orchestrator:7001/mcp");

        assert!(
            hosts.contains(&"orchestrator:7001".to_string()),
            "{hosts:?}"
        );
        assert!(hosts.contains(&"localhost".to_string()), "{hosts:?}");
        assert!(hosts.contains(&"127.0.0.1".to_string()), "{hosts:?}");
    }

    #[test]
    fn a_url_without_a_port_admits_the_bare_host() {
        let hosts = allowed_hosts("https://mcp.example.invalid/mcp");

        assert!(
            hosts.contains(&"mcp.example.invalid".to_string()),
            "{hosts:?}"
        );
    }

    #[test]
    fn a_loopback_url_adds_nothing_twice_and_an_unusable_one_adds_nothing() {
        let defaults = allowed_hosts("not a url");

        assert!(!defaults.is_empty());
        assert_eq!(allowed_hosts("http://localhost/mcp"), defaults);
    }
}
