//! `release`: give a held task back (`SPEC.md`, "MCP tool contracts" →
//! `release`).
//!
//! "The caller must hold the lease. The reason is recorded as a comment. If
//! `attempts` has reached the project's `max_attempts`, the task moves to the
//! project's `human` state with `needs_human_reason` set, and the output
//! `task` shows that state."
//!
//! All three are [`release_by_agent`]'s, including the escalation and the
//! email it makes due; what this handler owes is the empty-reason rejection
//! before anything is written, the lock, and the commit that sends that email
//! only if the release actually happened.

use crate::mcp::tools::common::{
    begin_mutation, finish, require_holder, resolve_task_for_mutation,
};
use crate::mcp::tools::{ReleaseInput, TaskOutput, non_empty};
use crate::mcp::{McpResult, SessionContext};
use crate::prelude::*;
use crate::tracker::{TaskDto, TrackerMutation, release_by_agent};

/// `{ task, reason }` → `{ task: Task }`, or `conflict`.
pub async fn handle(
    state: &AppState,
    ctx: &SessionContext,
    input: ReleaseInput,
) -> McpResult<TaskOutput> {
    // Before the lock: a blank reason is the argument being wrong, and there
    // is nothing to serialise against.
    non_empty("reason", &input.reason)?;

    let mut mutation = begin_mutation(state, ctx).await?;
    let released = release(&mut mutation, ctx, &input).await;
    let task = finish(state, mutation, released).await?;

    Ok(TaskOutput { task })
}

/// The release itself, inside the open mutation.
///
/// [`require_holder`] is not redundant with the tracker's own check: it
/// refuses before the comment row is written, so a release by a non-holder
/// does not reach the point where anything would have to be rolled back. Both
/// answer with the same message.
async fn release(
    m: &mut TrackerMutation<'_>,
    ctx: &SessionContext,
    input: &ReleaseInput,
) -> McpResult<TaskDto> {
    let task = resolve_task_for_mutation(m, ctx, &input.task).await?;
    require_holder(&task, ctx)?;

    Ok(release_by_agent(m, &task, ctx.session_id, &input.reason).await?)
}
