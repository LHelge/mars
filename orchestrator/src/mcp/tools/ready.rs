//! `ready`: the claimable tasks in the profile's served states (`SPEC.md`,
//! "MCP tool contracts" → `ready`).
//!
//! A read and only a read (ADR 0030): no project lock, no `task_events`, no
//! `task_sessions` row and no touch timestamp. Listing work is not working on
//! it, so this handler takes the pool rather than a `TrackerMutation`.
//!
//! The listing itself — the claimable query, the ordering, the excerpt and the
//! dependency count — belongs to `tracker::leases::ready_summaries`, which the
//! release endpoint and this tool share. What is left here is the tool
//! boundary: validating `limit`, reading the profile's served states, and the
//! one shortcut a profile serving nothing deserves.

use uuid::Uuid;

use crate::mcp::tools::{ReadyInput, ReadyOutput, validate_limit};
use crate::mcp::{McpResult, SessionContext};
use crate::prelude::*;
use crate::repositories::TaskRepository;
use crate::tracker::leases::ready_summaries;

/// The tasks this session's profile could claim now.
///
/// "Returns tasks in the calling profile's served states that are not blocked
/// and have no lease holder, ordered by `priority` then `number`. A profile
/// that serves no states gets an empty list" (`SPEC.md`, `ready`).
///
/// The served states are read outside any lock: a profile edit committed
/// between this read and the query would make the list one call stale, which
/// is no worse than the staleness every listing has — a task listed here can
/// be claimed by somebody else a moment later anyway, and `claim`'s conflict
/// is the answer to that.
pub async fn handle(
    state: &AppState,
    ctx: &SessionContext,
    input: ReadyInput,
) -> McpResult<ReadyOutput> {
    let limit = validate_limit(input.limit)?;

    let served: Vec<Uuid> = TaskRepository::new(&state.pool)
        .list_profile_states(ctx.profile.id)
        .await?
        .into_iter()
        .map(|state| state.id)
        .collect();

    // No served state can match no task, so the empty list is answered without
    // asking the database for it.
    if served.is_empty() {
        return Ok(ReadyOutput { tasks: Vec::new() });
    }

    let tasks = ready_summaries(&state.pool, ctx.project_id, &served, limit).await?;

    debug!(
        session_id = %ctx.session_id,
        project_id = %ctx.project_id,
        count = tasks.len(),
        "ready listed",
    );

    Ok(ReadyOutput { tasks })
}
