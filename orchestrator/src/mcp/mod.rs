//! The `rmcp` server: tool handlers and session bearer auth.

// The tool handlers, which are what will use the prelude, arrive with the MCP
// epic; until then the glob import below has nothing to feed (`CLAUDE.md`,
// "Backend conventions"). The first handler module here removes this `allow`.
#![allow(unused_imports)]

use axum::Router;

use crate::prelude::*;

/// The router served on `MCP_PORT` until the MCP epic lands.
///
/// It answers 404 for every path. The MCP epic replaces it with the `rmcp`
/// Streamable HTTP service at `/mcp` behind the session bearer-token
/// middleware (`ARCHITECTURE.md`, "MCP design"); binding the listener now
/// means the port, the network and the shutdown path are already in place when
/// it does.
pub fn placeholder_router() -> Router {
    Router::new()
}
