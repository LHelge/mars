//! Dropping a task's current hand-off: a user saying "do not build on this".
//!
//! `SPEC.md`, "Code hand-offs and review" (`POST
//! /projects/{pid}/tasks/{id}/drop-handoff`); `ARCHITECTURE.md`, "Task
//! tracker" → "Code hand-offs"; `docs/data-model.md`, `tasks`.
//!
//! The one writer that sets `tasks.current_handoff_id` back to `NULL`. It
//! exists for recovery: a hand-off that is known to be bad — built on the
//! wrong base, merged and since reverted — would otherwise be the base of the
//! task's next launch, because a launch starts from the current hand-off when
//! there is one. With the pointer cleared that launch starts from the
//! project's default branch instead.
//!
//! What it does not do is as much the contract as what it does:
//!
//! - **the history stays.** The `task_handoffs` rows and their
//!   `refs/handoffs/<id>` refs are untouched; only the pointer moves, so the
//!   dropped work can still be read and fetched.
//! - **nothing else about the task moves.** State, lease, `attempts`, `rounds`
//!   and `closed_at` are exactly what they were. A held task may have its
//!   hand-off dropped, the holder keeps its lease, and its checkout is left as
//!   it is, as for any hand-off change a running session did not make.
//! - **users only.** There is no MCP tool: discarding committed work is a
//!   human decision, as the rest of guided rollback is.
//!
//! Like every hand-off change it needs a non-empty comment saying why, written
//! as the user's comment. The events are `updated`, carrying the task without
//! its hand-off, and then `commented` — the order a publication gives its state
//! event and its comment.
//!
//! [`drop_handoff`] takes the mutation and a task id rather than opening one,
//! so a caller that drops the hand-offs of many tasks — a rollback of the
//! default branch — does it inside one mutation, under one project lock, and
//! either all of them commit or none do.

use uuid::Uuid;

use crate::events::TaskEventKind;
use crate::models::TaskRef;
use crate::prelude::*;
use crate::repositories::TaskRepository;
use crate::repositories::tasks::StateFields;
use crate::tracker::{CommentAuthor, TaskDto, TrackerMutation, add_comment};

/// What dropping the hand-off of a task that has none is told (409).
pub const NO_CURRENT_HANDOFF: &str = "task has no current hand-off";

/// Clear the current hand-off of task `task_id`, with the user's `comment`.
///
/// The task is read under the mutation's lock here, so the caller passes an id
/// and not a row that may be stale. Answers the task as it is afterwards.
///
/// 404 for a task that is not this project's; 409 [`NO_CURRENT_HANDOFF`] for a
/// task without a current hand-off; 400 `comment body must not be empty` for
/// an empty comment. Any refusal leaves the mutation's earlier writes to the
/// caller, who rolls the whole mutation back on the error.
pub async fn drop_handoff(
    m: &mut TrackerMutation<'_>,
    task_id: Uuid,
    user_id: Uuid,
    comment: &str,
) -> Result<TaskDto> {
    let project_id = m.project_id();
    let repository = TaskRepository::new(m.pool());

    let task = repository
        .find_task_for_update(m.conn(), project_id, TaskRef::Id(task_id))
        .await?
        .ok_or(Error::NotFound)?;

    let Some(dropped) = task.current_handoff_id else {
        return Err(Error::Conflict(NO_CURRENT_HANDOFF.into()));
    };

    // The pointer and nothing else: every other column is left alone.
    repository
        .set_task_state_fields(
            m.conn(),
            project_id,
            task.id,
            &StateFields {
                current_handoff_id: Some(None),
                ..StateFields::default()
            },
        )
        .await?;
    m.touch_actor(task.id);

    let dto = repository
        .load_task_dto_in(m.conn(), project_id, task.id)
        .await?
        .ok_or(Error::NotFound)?;
    m.emit_task(TaskEventKind::Updated, &dto)?;

    // After the `updated` event, as a publication's comment follows its state
    // event; an empty body is refused here and rolls everything back.
    add_comment(m, &task, CommentAuthor::User(user_id), comment).await?;

    info!(
        project_id = %project_id,
        task_id = %task.id,
        handoff_id = %dropped,
        "current hand-off dropped",
    );

    Ok(dto)
}
