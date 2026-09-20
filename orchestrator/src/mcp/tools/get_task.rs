//! `get_task`: one task in full (`SPEC.md`, "MCP tool contracts" →
//! `get_task`).
//!
//! A stub: the dispatcher already routes here, and the tool's own task
//! replaces the body. Nothing outside this module changes when it does.

use crate::mcp::tools::{TaskDetailOutput, TaskOnlyInput};
use crate::mcp::{McpError, McpResult, SessionContext};
use crate::prelude::*;

/// Not implemented yet. Answers `internal`, and says so in the log — an agent
/// that calls it has found a gap in the server, not a mistake of its own.
pub async fn handle(
    state: &AppState,
    ctx: &SessionContext,
    input: TaskOnlyInput,
) -> McpResult<TaskDetailOutput> {
    let _ = (state, ctx, input);
    error!(tool = "get_task", "tool not implemented");

    Err(McpError::internal())
}
