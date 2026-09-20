//! `list_session_branches`: the mirror's session branches (git; profile-gated;
//! `SPEC.md`, "MCP tool contracts" → `list_session_branches`).
//!
//! A stub: the dispatcher already routes here, and the tool's own task
//! replaces the body. Nothing outside this module changes when it does.

use crate::mcp::tools::{BranchesOutput, EmptyInput};
use crate::mcp::{McpError, McpResult, SessionContext};
use crate::prelude::*;

/// Not implemented yet. Answers `internal`, and says so in the log — an agent
/// that calls it has found a gap in the server, not a mistake of its own.
pub async fn handle(
    state: &AppState,
    ctx: &SessionContext,
    input: EmptyInput,
) -> McpResult<BranchesOutput> {
    let _ = (state, ctx, input);
    error!(tool = "list_session_branches", "tool not implemented");

    Err(McpError::internal())
}
