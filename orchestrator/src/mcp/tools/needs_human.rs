//! `needs_human`: escalate a task to the project's human state (`SPEC.md`,
//! "MCP tool contracts" → `needs_human`).
//!
//! "Moves the task to the project's `human` state, sets `needs_human_reason`,
//! releases the lease and resets `attempts`. Requires holding the lease or the
//! task being unheld." The lease rule is the one place this differs from
//! `release`: an agent may hand over a task nobody is working on, and only a
//! task *somebody else* holds is refused — with "task is held by another
//! session", which is why [`require_holder`] is deliberately not called here.
//!
//! The already-escalated variant — "record the reason and explicitly release
//! any held lease, but preserve `attempts`; emit `commented`, `updated` and,
//! when applicable, `released`, with no new `escalated` event or escalation
//! email" — is [`needs_human`]'s own branch, chosen from the task's state
//! under the lock. This handler therefore has one call and no second path:
//! whether an email is owed is decided by the tracker and carried out of the
//! commit by `finish`.
//!
//! [`require_holder`]: super::common::require_holder

use crate::mcp::tools::common::{begin_mutation, finish, resolve_task_for_mutation};
use crate::mcp::tools::{NeedsHumanInput, TaskOutput, non_empty};
use crate::mcp::{McpResult, SessionContext};
use crate::prelude::*;
use crate::tracker::{TaskDto, TrackerMutation, needs_human};

/// `{ task, reason }` → `{ task: Task }`, or `conflict`.
pub async fn handle(
    state: &AppState,
    ctx: &SessionContext,
    input: NeedsHumanInput,
) -> McpResult<TaskOutput> {
    // The reason is the whole point of the tool — it is what the person who
    // picks the task up reads — so an empty one is refused before the lock.
    non_empty("reason", &input.reason)?;

    let mut mutation = begin_mutation(state, ctx).await?;
    let handed = hand_over(&mut mutation, ctx, &input).await;
    let task = finish(state, mutation, handed).await?;

    Ok(TaskOutput { task })
}

/// The hand-over itself, inside the open mutation.
async fn hand_over(
    m: &mut TrackerMutation<'_>,
    ctx: &SessionContext,
    input: &NeedsHumanInput,
) -> McpResult<TaskDto> {
    let task = resolve_task_for_mutation(m, ctx, &input.task).await?;

    Ok(needs_human(m, &task, ctx.session_id, &input.reason).await?)
}
