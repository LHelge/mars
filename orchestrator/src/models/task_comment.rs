//! Task comments: the agent-to-agent and human-to-agent channel.
//!
//! `docs/data-model.md`, `task_comments` and `SPEC.md`, "Tasks". Exactly one
//! author column is set when `system` is false and none when it is true; the
//! rule is enforced here at insert time because either author may later become
//! NULL through `ON DELETE SET NULL`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::models::task::{TaskError, TaskResult};
// The crate convention (`CLAUDE.md`, "Backend conventions"); see `task.rs`.
#[allow(unused_imports)]
use crate::prelude::*;

/// A `task_comments` row, column for column (`docs/data-model.md`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct TaskComment {
    pub id: Uuid,
    pub task_id: Uuid,
    pub author_user_id: Option<Uuid>,
    pub author_session_id: Option<Uuid>,
    pub system: bool,
    pub body: String,
    pub created_at: DateTime<Utc>,
}

/// The caller-supplied half of a new comment.
///
/// The id is generated up front: a hand-off names its comment, so the id has to
/// be known before either row is inserted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTaskComment {
    pub id: Uuid,
    pub task_id: Uuid,
    pub author_user_id: Option<Uuid>,
    pub author_session_id: Option<Uuid>,
    pub system: bool,
    pub body: String,
}

impl NewTaskComment {
    /// A comment written by a signed-in user.
    pub fn from_user(task_id: Uuid, author_user_id: Uuid, body: impl Into<String>) -> Self {
        Self {
            id: Uuid::now_v7(),
            task_id,
            author_user_id: Some(author_user_id),
            author_session_id: None,
            system: false,
            body: body.into(),
        }
    }

    /// A comment written by an agent session over MCP.
    pub fn from_session(task_id: Uuid, author_session_id: Uuid, body: impl Into<String>) -> Self {
        Self {
            id: Uuid::now_v7(),
            task_id,
            author_user_id: None,
            author_session_id: Some(author_session_id),
            system: false,
            body: body.into(),
        }
    }

    /// A comment the orchestrator writes itself: reaper releases, escalations.
    pub fn from_system(task_id: Uuid, body: impl Into<String>) -> Self {
        Self {
            id: Uuid::now_v7(),
            task_id,
            author_user_id: None,
            author_session_id: None,
            system: true,
            body: body.into(),
        }
    }

    /// A non-empty body, and an authorship that matches `system`.
    pub fn validate(&self) -> TaskResult<()> {
        if self.body.trim().is_empty() {
            return Err(TaskError::EmptyComment);
        }

        let authors = usize::from(self.author_user_id.is_some())
            + usize::from(self.author_session_id.is_some());
        let expected = if self.system { 0 } else { 1 };
        if authors != expected {
            return Err(TaskError::InvalidCommentAuthor);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_constructors_produce_valid_comments() {
        let task_id = Uuid::new_v4();
        assert!(
            NewTaskComment::from_user(task_id, Uuid::new_v4(), "looks good")
                .validate()
                .is_ok()
        );
        assert!(
            NewTaskComment::from_session(task_id, Uuid::new_v4(), "pushed a fix")
                .validate()
                .is_ok()
        );
        assert!(
            NewTaskComment::from_system(task_id, "released: the session ended")
                .validate()
                .is_ok()
        );
    }

    #[test]
    fn a_system_comment_has_no_author() {
        let task_id = Uuid::new_v4();
        let comment = NewTaskComment::from_system(task_id, "stalled");
        assert!(comment.system);
        assert!(comment.author_user_id.is_none());
        assert!(comment.author_session_id.is_none());
    }

    #[test]
    fn an_empty_body_is_rejected() {
        let mut comment = NewTaskComment::from_user(Uuid::new_v4(), Uuid::new_v4(), "");
        assert_eq!(comment.validate(), Err(TaskError::EmptyComment));

        comment.body = "  \t\n ".to_string();
        assert_eq!(comment.validate(), Err(TaskError::EmptyComment));

        comment.body = " x ".to_string();
        assert!(comment.validate().is_ok());
    }

    #[test]
    fn a_non_system_comment_needs_exactly_one_author() {
        let mut comment = NewTaskComment::from_user(Uuid::new_v4(), Uuid::new_v4(), "hello");

        comment.author_session_id = Some(Uuid::new_v4());
        assert_eq!(comment.validate(), Err(TaskError::InvalidCommentAuthor));

        comment.author_user_id = None;
        assert!(comment.validate().is_ok());

        comment.author_session_id = None;
        assert_eq!(comment.validate(), Err(TaskError::InvalidCommentAuthor));
    }

    #[test]
    fn a_system_comment_rejects_any_author() {
        let mut comment = NewTaskComment::from_system(Uuid::new_v4(), "escalated");

        comment.author_user_id = Some(Uuid::new_v4());
        assert_eq!(comment.validate(), Err(TaskError::InvalidCommentAuthor));

        comment.author_user_id = None;
        comment.author_session_id = Some(Uuid::new_v4());
        assert_eq!(comment.validate(), Err(TaskError::InvalidCommentAuthor));
    }
}
