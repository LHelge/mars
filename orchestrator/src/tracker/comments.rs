//! Writing a comment on a task: the one code path `POST
//! /projects/{pid}/tasks/{id}/comments`, the MCP `comment` tool and every
//! system comment the orchestrator writes itself go through.
//!
//! `SPEC.md`, "Tasks" and "MCP tool contracts" → `comment`;
//! `docs/data-model.md`, `task_comments` and `task_sessions`; ADR 0027 (bodies
//! are stored and displayed unredacted) and ADR 0030 (the session link).
//!
//! Comments are the tracker's one genuinely open write. "Any session in the
//! project may comment on any task": there is no lease to hold and no
//! authorship to prove, because a comment is how a session that noticed
//! something tells whoever picks the task up next. Nothing here consults the
//! lease, and the caller is not expected to have checked one.
//!
//! What the comment owes is the same three things every tracker change owes,
//! in this order:
//!
//! 1. the row, validated by [`NewTaskComment::validate`] — a non-empty body,
//!    and exactly one author unless the comment is the orchestrator's own;
//! 2. one `commented` event carrying the actor, the task **unchanged** (a
//!    comment changes no task field, not even `updated_at`) and the comment
//!    itself;
//! 3. the `task_sessions` link, when the change is a session's — written on
//!    commit, so a refused comment leaves no trace of having been attempted.
//!
//! A system comment is the orchestrator speaking, so its event carries actor
//! `system` whoever's mutation it was written in: a reaper release and an
//! escalation both say what they did in the task's own thread, and neither is
//! the released session's doing.
//!
//! Bodies are content and are never logged, at any level (`CLAUDE.md`, rule 3).

use uuid::Uuid;

use crate::events::TaskActor;
use crate::models::{NewTaskComment, Task};
use crate::prelude::*;
use crate::repositories::TaskRepository;
use crate::tracker::{CommentDto, TrackerMutation};

/// Who wrote a comment (`docs/data-model.md`, `task_comments`).
///
/// The three author shapes the table allows, as one type, so a comment cannot
/// be built with two authors or with none: `author_user_id`,
/// `author_session_id` and `system` follow from the variant rather than from
/// three fields a caller sets by hand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentAuthor {
    /// A signed-in user, through REST.
    User(Uuid),
    /// An agent session, through MCP.
    Session(Uuid),
    /// The orchestrator itself: reaper releases, escalations, parent closure.
    System,
}

/// Write one comment inside an open mutation, with the event it owes.
///
/// `task` is the row as it is under this mutation's lock; the `commented`
/// payload describes it unchanged, because that is what it is.
///
/// An empty body — empty after trimming — is 400 `comment body must not be
/// empty`, raised by the model before any row is written. A task outside this
/// project is 404, decided in the repository's `WHERE` clause rather than
/// after the fact.
///
/// The returned [`CommentDto`] is the stored row, which is what the caller
/// answers 201 with and what the event carries.
pub async fn add_comment(
    m: &mut TrackerMutation<'_>,
    task: &Task,
    author: CommentAuthor,
    body: &str,
) -> Result<CommentDto> {
    let project_id = m.project_id();

    let new_comment = match author {
        CommentAuthor::User(user_id) => NewTaskComment::from_user(task.id, user_id, body),
        CommentAuthor::Session(session_id) => {
            NewTaskComment::from_session(task.id, session_id, body)
        }
        CommentAuthor::System => NewTaskComment::from_system(task.id, body),
    };

    let repository = TaskRepository::new(m.pool());
    let inserted = repository
        .insert_comment(m.conn(), project_id, &new_comment)
        .await?;
    let comment = CommentDto::from(inserted);

    let task_dto = repository
        .load_task_dto_in(m.conn(), project_id, task.id)
        .await?
        .ok_or(Error::NotFound)?;

    // A system comment is the orchestrator's, whoever's mutation carried it.
    let previous = match author {
        CommentAuthor::System => Some(m.set_actor(TaskActor::System)),
        _ => None,
    };
    m.emit_comment(&task_dto, &comment)?;
    if let Some(previous) = previous {
        m.set_actor(previous);
    }

    // After the actor is restored: a session that wrote a system comment still
    // worked on the task, and `task_sessions` is about who worked on it.
    m.touch_actor(task.id);

    // The ids and nothing else: the body is user or agent content.
    info!(
        project_id = %project_id,
        task_id = %task.id,
        comment_id = %comment.id,
        "task comment written",
    );

    Ok(comment)
}
