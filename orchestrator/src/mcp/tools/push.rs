//! `push`: publish a mirror branch upstream (git; profile-gated; `SPEC.md`,
//! "MCP tool contracts" → `push`).
//!
//! A stub: the dispatcher already routes here, and the tool's own task
//! replaces the body. Nothing outside this module changes when it does.

use crate::mcp::tools::{PushInput, PushOutput};
use crate::mcp::{McpError, McpResult, SessionContext};
use crate::prelude::*;

/// Not implemented yet. Answers `internal`, and says so in the log — an agent
/// that calls it has found a gap in the server, not a mistake of its own.
pub async fn handle(
    state: &AppState,
    ctx: &SessionContext,
    input: PushInput,
) -> McpResult<PushOutput> {
    let _ = (state, ctx, input);
    error!(tool = "push", "tool not implemented");

    Err(McpError::internal())
}
