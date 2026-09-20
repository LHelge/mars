//! `claim`: take a task's lease (`SPEC.md`, "MCP tool contracts" → `claim`).
//!
//! Two conflicts, and they say different things. A task outside the states
//! this profile serves is "task is not in a state this profile serves": the
//! agent is told the task is not *for* it, which no amount of retrying will
//! change. Anything else the atomic statement refuses — a task somebody else
//! holds, a blocked one, one this very session already holds, one that moved
//! between the read and the update — is "task is not claimable", and the right
//! answer to that is another task.
//!
//! Both come from [`claim_for_profile`] with the messages `SPEC.md` quotes, so
//! this handler constructs neither: it resolves the task under the lock, reads
//! the profile's served states in the same transaction, and lets the tracker
//! decide.
//!
//! **Nothing is done to the session's checkout.** "Claiming in an existing
//! session never resets its checkout": the returned task carries the current
//! hand-off, and fetching `refs/handoffs/<handoff.id>` is the agent's own
//! move.

use crate::mcp::tools::common::{begin_mutation, finish, resolve_task_for_mutation};
use crate::mcp::tools::{TaskOnlyInput, TaskOutput};
use crate::mcp::{McpResult, SessionContext};
use crate::prelude::*;
use crate::repositories::TaskRepository;
use crate::tracker::{TaskDto, TrackerMutation, claim_for_profile};

/// `{ task }` → `{ task: Task }`, or `conflict`.
pub async fn handle(
    state: &AppState,
    ctx: &SessionContext,
    input: TaskOnlyInput,
) -> McpResult<TaskOutput> {
    let mut mutation = begin_mutation(state, ctx).await?;
    let claimed = claim(&mut mutation, ctx, &input).await;
    let task = finish(state, mutation, claimed).await?;

    Ok(TaskOutput { task })
}

/// The claim itself, inside the open mutation.
///
/// The served states are read through the mutation's connection rather than
/// from the pool: `profile_states` is edited under the same project lock, so a
/// list read outside it could be one a concurrent edit has already replaced —
/// and a pool read here would make the mutation hold two connections
/// (`docs/data-model.md`, "Tracker mutation transactions").
async fn claim(
    m: &mut TrackerMutation<'_>,
    ctx: &SessionContext,
    input: &TaskOnlyInput,
) -> McpResult<TaskDto> {
    let task = resolve_task_for_mutation(m, ctx, &input.task).await?;

    let project_id = m.project_id();
    let served: Vec<_> = TaskRepository::new(m.pool())
        .list_profile_states_in(m.conn(), project_id, ctx.profile.id)
        .await?
        .into_iter()
        .map(|state| state.id)
        .collect();

    Ok(claim_for_profile(m, &task, ctx.session_id, &served).await?)
}
