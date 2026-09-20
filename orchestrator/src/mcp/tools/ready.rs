//! `ready`: the claimable tasks in the profile's served states (`SPEC.md`,
//! "MCP tool contracts" → `ready`).
//!
//! A stub: the dispatcher already routes here, and the tool's own task
//! replaces the body. Nothing outside this module changes when it does.

use crate::mcp::tools::{ReadyInput, ReadyOutput};
use crate::mcp::{McpError, McpResult, SessionContext};
use crate::prelude::*;

/// Not implemented yet. Answers `internal`, and says so in the log — an agent
/// that calls it has found a gap in the server, not a mistake of its own.
pub async fn handle(
    state: &AppState,
    ctx: &SessionContext,
    input: ReadyInput,
) -> McpResult<ReadyOutput> {
    let _ = (state, ctx, input);
    error!(tool = "ready", "tool not implemented");

    Err(McpError::internal())
}
