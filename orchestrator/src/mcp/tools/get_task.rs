//! `get_task`: one task in full (`SPEC.md`, "MCP tool contracts" →
//! `get_task`).
//!
//! "Output `{ task: TaskDetail }` (same shape as REST)" — and the way to keep
//! it the same shape is to call the same loader: `load_task_detail` is what
//! `GET /projects/{pid}/tasks/{id}` uses, so a field added to the drawer's
//! payload reaches the agent without a second mapper to remember.
//!
//! A read and only a read (ADR 0030): no lock, no event, no session link and
//! no touch timestamp. Reading a task is not holding it.

use crate::mcp::tools::{TaskDetailOutput, TaskOnlyInput};
use crate::mcp::{McpError, McpResult, SessionContext};
use crate::prelude::*;
use crate::repositories::TaskRepository;

/// The rejection for a task reference that resolves to nothing in this
/// project.
///
/// One answer for both "no such task" and "a task of another project": the
/// caller's session is scoped to its project, and which other project holds a
/// UUID is not its to learn — the REST detail route answers the same way.
const NOT_FOUND: &str = "task not found";

/// The task with its comments, hand-offs, children and sessions.
pub async fn handle(
    state: &AppState,
    ctx: &SessionContext,
    input: TaskOnlyInput,
) -> McpResult<TaskDetailOutput> {
    let task_ref = input.task.parse()?;

    let task = TaskRepository::new(&state.pool)
        .load_task_detail(ctx.project_id, task_ref)
        .await?
        .ok_or_else(|| McpError::not_found(NOT_FOUND))?;

    debug!(
        session_id = %ctx.session_id,
        project_id = %ctx.project_id,
        task_id = %task.task.id,
        "task read",
    );

    Ok(TaskDetailOutput { task })
}
