//! What every tool that *changes* the tracker does the same way.
//!
//! `SPEC.md`, "MCP tool contracts": "Successful tracker changes commit their
//! `TaskEvent` rows and the calling session's link to the directly changed
//! task together... Rejected tracker operations and updates with no effective
//! changes likewise leave tracker history and links unchanged." That is one
//! shape, repeated by `claim`, `release`, `comment`, `needs_human`, `update`
//! and `create_task`:
//!
//! 1. [`begin_mutation`] — one [`TrackerMutation`] with the project row locked
//!    and actor `session`, so every event the change emits names the caller by
//!    construction (`ARCHITECTURE.md`, "Task tracker" → "One mutation at a
//!    time per project"; ADR 0021);
//! 2. [`resolve_task_for_mutation`] — the `task` argument turned into the row
//!    as it is *inside* that lock, never a pool read;
//! 3. the tracker operation itself, which is the epic that owns the rules;
//! 4. [`finish`] — commit and send the escalation emails the change made due,
//!    or roll the whole thing back.
//!
//! [`require_holder`] and [`task_output`] are the two pieces of step 3 that
//! are the transport's rather than the tracker's: who may act on a held task,
//! and the `{ task }` body the contract answers with.
//!
//! Nothing here sends email, touches git or talks to the engine: the project
//! lock is held across steps 2 and 3, and [`finish`] is what closes it before
//! the first email goes out.

use uuid::Uuid;

use crate::events::TaskActor;
use crate::mcp::tools::{TaskArg, TaskOutput};
use crate::mcp::{McpError, McpResult, SessionContext};
use crate::models::Task;
use crate::prelude::*;
use crate::repositories::TaskRepository;
use crate::tracker::leases::NOT_HELD_BY_SESSION;
use crate::tracker::{TrackerMutation, commit_and_notify};

/// What a `task` argument that names no task of this project is answered
/// with.
///
/// The reference itself was well formed — a malformed one is
/// `invalid_argument` from [`TaskArg::parse`] — so the agent is told the task
/// is not there rather than that its argument was wrong.
pub const TASK_NOT_FOUND: &str = "task not found";

/// Open the tracker mutation a tool's change runs in.
///
/// The actor is the calling session, taken from the [`SessionContext`] the
/// bearer middleware resolved and never from an argument, so an agent cannot
/// attribute a change to a session whose token it does not hold
/// (`ARCHITECTURE.md`, "MCP design" → Authentication).
///
/// It waits for any other mutation of the same project, which is the queue the
/// tracker is specified to have; an unknown project cannot occur, because the
/// project comes from the session row.
pub async fn begin_mutation<'a>(
    state: &'a AppState,
    ctx: &SessionContext,
) -> McpResult<TrackerMutation<'a>> {
    let mutation = TrackerMutation::begin(
        &state.pool,
        ctx.project_id,
        TaskActor::Session {
            session_id: ctx.session_id,
        },
    )
    .await?;

    Ok(mutation)
}

/// The task a `task` argument names, as the row is under this mutation's lock.
///
/// Both halves matter. The *scope* is the calling session's project, in the
/// `WHERE` clause rather than in a check afterwards, so a task of another
/// project is indistinguishable from one that does not exist. The *lock* is
/// `SELECT ... FOR UPDATE` on the row, taken after the project row, which is
/// the documented order (ADR 0021): the row a tool validates against is then
/// the row it changes, and a per-project number resolved outside the lock
/// cannot name a different task by the time the change lands.
pub async fn resolve_task_for_mutation(
    m: &mut TrackerMutation<'_>,
    ctx: &SessionContext,
    arg: &TaskArg,
) -> McpResult<Task> {
    let reference = arg.parse()?;
    let project_id = ctx.project_id;

    TaskRepository::new(m.pool())
        .find_task_for_update(m.conn(), project_id, reference)
        .await?
        .ok_or_else(|| McpError::not_found(TASK_NOT_FOUND))
}

/// Refuse a caller that does not hold the task's lease.
///
/// `SPEC.md` requires it of `release` and of most of `update`: "The caller
/// must hold the lease." The message is the tracker's own
/// [`NOT_HELD_BY_SESSION`], not a second wording of the same refusal — a
/// release checks here and the tracker checks again under its own rules, and
/// an agent must not be able to tell the two apart.
///
/// A task nobody holds fails this too: an unheld task is not this session's to
/// hand back. `needs_human` is the documented exception and does not call
/// this.
pub fn require_holder(task: &Task, ctx: &SessionContext) -> McpResult<()> {
    if task.lease_holder_session_id != Some(ctx.session_id) {
        return Err(McpError::conflict(NOT_HELD_BY_SESSION));
    }

    Ok(())
}

/// The `{ task: Task }` body, loaded inside the transaction.
///
/// The REST `Task` shape, hand-off, dependencies and all, read through the
/// mutation's own connection *before* it commits — the pool would still answer
/// with the row as it was. A tool that already has the [`TaskDto`] the tracker
/// returned wraps that one instead; this is for the paths that changed a task
/// through several tracker calls and want the final row once.
///
/// [`TASK_NOT_FOUND`] cannot happen here in practice — the task was locked a
/// moment ago in the same transaction — and is answered rather than panicked.
pub async fn task_output(m: &mut TrackerMutation<'_>, task_id: Uuid) -> McpResult<TaskOutput> {
    let project_id = m.project_id();

    let task = TaskRepository::new(m.pool())
        .load_task_dto_in(m.conn(), project_id, task_id)
        .await?
        .ok_or_else(|| McpError::not_found(TASK_NOT_FOUND))?;

    Ok(TaskOutput { task })
}

/// Commit the mutation and send what it made due, or roll it back.
///
/// The second half of the contract quoted in the module doc. On success the
/// rows, the events and the `task_sessions` links commit together and the
/// escalation emails go out *after* the transaction closes
/// ([`commit_and_notify`]). On failure the transaction is rolled back
/// explicitly, so the tool's rejection leaves no row change, no event, no link
/// and no email — and the project lock is released before the error travels
/// back up.
///
/// A rollback that itself fails is logged and the caller still receives the
/// original rejection: the transaction is going away with the connection
/// either way, and answering with the second error would hide the first.
pub async fn finish<T>(
    state: &AppState,
    m: TrackerMutation<'_>,
    outcome: McpResult<T>,
) -> McpResult<T> {
    match outcome {
        Ok(value) => {
            commit_and_notify(m, state).await?;
            Ok(value)
        }
        Err(err) => {
            if let Err(error) = m.no_change().await {
                error!(error = ?error, "a rejected mcp tool call failed to roll back");
            }
            Err(err)
        }
    }
}
