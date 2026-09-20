//! What the bearer middleware resolves a session token into.
//!
//! `ARCHITECTURE.md`, "MCP design", Authentication: the middleware hashes the
//! token, finds the session and attaches the resolved context to the request.
//! Tool handlers never take a session id as an argument — they read it from
//! here, so a caller cannot name a session it does not hold the token for.

// No `use crate::prelude::*;`: this module is one plain data type and touches
// nothing the prelude carries, like `models::token` and `ws::protocol`.
use crate::models::AgentProfile;
use uuid::Uuid;

/// The calling session, as the bearer middleware resolved it.
///
/// Attached to the HTTP request by the middleware and read back by
/// `McpServer::context` on every tool call (the method is private, so this is
/// a name and not a link). The profile travels with it because `tools/list` filters the git
/// tools on `profile.mcp_tools` and a second lookup per call would be a second
/// round trip for a value the middleware already read.
#[derive(Clone, Debug)]
pub struct SessionContext {
    pub session_id: Uuid,
    pub project_id: Uuid,
    pub profile: AgentProfile,
}
