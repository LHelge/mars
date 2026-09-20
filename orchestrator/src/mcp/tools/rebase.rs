//! `rebase`: rebase a branch onto another (git; profile-gated; `SPEC.md`,
//! "MCP tool contracts" → `rebase`).
//!
//! A thin adapter over [`GitService::rebase`], attributed to the calling
//! session (ADR 0007). The ref-kind rules are not restated here: `branch` must
//! be an integration head or a session ref and `onto` an integration head or
//! an upstream-tracking name, and the service refuses anything else with the
//! 400 that becomes `invalid_argument` — before it locks, syncs or writes
//! anything.
//!
//! When `branch` is the calling session's own, the service reconciles that
//! session's checkout afterwards and reports what became of it in the `git`
//! event's `detail.work_tree` (`updated` or `reconciliation_required`;
//! `SPEC.md`, "AgentEvent"). That is where the agent reads it: the tool's own
//! answer stays `{ commit }`, so a dirty checkout is not a failed rebase — the
//! mirror's ref really was rewritten.

use crate::git::{GitActor, GitService};
use crate::mcp::tools::{CommitOutput, RebaseInput};
use crate::mcp::{McpResult, SessionContext};
use crate::prelude::*;

/// Rebase, and answer the commit the rewritten branch now points at.
pub async fn handle(
    state: &AppState,
    ctx: &SessionContext,
    input: RebaseInput,
) -> McpResult<CommitOutput> {
    let outcome = GitService::from_state(state)
        .rebase(
            ctx.project_id,
            &input.branch,
            &input.onto,
            &GitActor::Session(ctx.session_id),
        )
        .await?;

    Ok(CommitOutput {
        commit: outcome.commit,
    })
}
