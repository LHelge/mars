//! `claim`: take a task's lease (`SPEC.md`, "MCP tool contracts" → `claim`).
//!
//! A stub: the dispatcher already routes here, and the tool's own task
//! replaces the body. Nothing outside this module changes when it does.

use crate::mcp::tools::{TaskOnlyInput, TaskOutput};
use crate::mcp::{McpError, McpResult, SessionContext};
use crate::prelude::*;

/// Not implemented yet. Answers `internal`, and says so in the log — an agent
/// that calls it has found a gap in the server, not a mistake of its own.
pub async fn handle(
    state: &AppState,
    ctx: &SessionContext,
    input: TaskOnlyInput,
) -> McpResult<TaskOutput> {
    let _ = (state, ctx, input);
    error!(tool = "claim", "tool not implemented");

    Err(McpError::internal())
}
