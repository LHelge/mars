//! `list_session_branches`: the mirror's session branches (git; profile-gated;
//! `SPEC.md`, "MCP tool contracts" → `list_session_branches`).
//!
//! A read and nothing else. [`GitService::list_session_branches`] takes the
//! project git lock briefly so that one listing describes one moment, syncs
//! nothing and writes nothing: "`list_session_branches` emits no session `git`
//! event" (`SPEC.md`, "MCP tool contracts"; `ARCHITECTURE.md`, "MCP design" →
//! Side effects).
//!
//! The tool takes no arguments at all — the project is the calling session's,
//! read from the [`SessionContext`] the bearer middleware attached, never from
//! the arguments (`ARCHITECTURE.md`, "MCP design" → Authentication).

use crate::git::GitService;
use crate::mcp::tools::{BranchesOutput, EmptyInput};
use crate::mcp::{McpResult, SessionContext};
use crate::prelude::*;

/// Every session branch of the calling session's project, with its ahead and
/// behind counts against the default branch.
pub async fn handle(
    state: &AppState,
    ctx: &SessionContext,
    _input: EmptyInput,
) -> McpResult<BranchesOutput> {
    let branches = GitService::from_state(state)
        .list_session_branches(ctx.project_id)
        .await?;

    Ok(BranchesOutput { branches })
}
