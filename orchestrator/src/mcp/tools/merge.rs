//! `merge`: merge into an integration branch (git; profile-gated; `SPEC.md`,
//! "MCP tool contracts" → `merge`).
//!
//! A stub: the dispatcher already routes here, and the tool's own task
//! replaces the body. Nothing outside this module changes when it does.

use crate::mcp::tools::{CommitOutput, McpMergeInput};
use crate::mcp::{McpError, McpResult, SessionContext};
use crate::prelude::*;

/// Not implemented yet. Answers `internal`, and says so in the log — an agent
/// that calls it has found a gap in the server, not a mistake of its own.
pub async fn handle(
    state: &AppState,
    ctx: &SessionContext,
    input: McpMergeInput,
) -> McpResult<CommitOutput> {
    let _ = (state, ctx, input);
    error!(tool = "merge", "tool not implemented");

    Err(McpError::internal())
}
