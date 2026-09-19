//! `task_comments`: the agent-to-agent and human-to-agent channel.
//!
//! `docs/data-model.md`, `task_comments` and `SPEC.md`, "Tasks". The author
//! rule — exactly one author column when `system` is false, none when it is
//! true — is enforced at insert time rather than by a constraint, because
//! either author may later become NULL through `ON DELETE SET NULL` and a
//! table check would then refuse the deletion.
//!
//! Comment bodies are content, never a log line: nothing here logs a body at
//! any level (`CLAUDE.md`, rule 3, and "Backend conventions").

use uuid::Uuid;

use crate::models::{NewTaskComment, TaskComment};
use crate::prelude::*;
use crate::repositories::tasks::{TaskRepository, task_in_project};
use crate::tracker::Locked;

impl TaskRepository<'_> {
    /// Insert a comment on a task of this project.
    ///
    /// Written under the token, together with the `commented` event it owes
    /// the board and, where a session wrote it, the session link the mutation
    /// records.
    ///
    /// [`NewTaskComment::validate`] decides the body and the authorship, so a
    /// comment with two authors, no author, or an author while `system` is
    /// [`TaskError::InvalidCommentAuthor`] (400) before any row is written. A
    /// task outside this project is [`Error::NotFound`], the answer every
    /// out-of-scope id gets.
    ///
    /// [`TaskError::InvalidCommentAuthor`]: crate::models::TaskError::InvalidCommentAuthor
    pub async fn insert_comment(
        &self,
        mut tx: Locked<'_>,
        project_id: Uuid,
        comment: &NewTaskComment,
    ) -> Result<TaskComment> {
        comment.validate()?;

        if !task_in_project(tx.reborrow(), project_id, comment.task_id).await? {
            return Err(Error::NotFound);
        }

        let inserted = sqlx::query_as!(
            TaskComment,
            r#"
            INSERT INTO task_comments (id, task_id, author_user_id, author_session_id, system, body)
            VALUES ($1, $2, $3, $4, $5, $6)
            RETURNING id, task_id, author_user_id, author_session_id, system, body, created_at
            "#,
            comment.id,
            comment.task_id,
            comment.author_user_id,
            comment.author_session_id,
            comment.system,
            comment.body,
        )
        .fetch_one(&mut *tx)
        .await?;

        // The ids and nothing else: the body is user or agent content.
        debug!(
            project_id = %project_id,
            task_id = %comment.task_id,
            comment_id = %inserted.id,
            "task comment inserted",
        );

        Ok(inserted)
    }

    /// One comment of this project, read on the mutation's connection.
    ///
    /// What a launch for a task needs and nothing more: the body of the
    /// comment a hand-off was published with, read under the same lock the
    /// hand-off was selected under, so the generated task message quotes the
    /// text the session is actually starting from (`SPEC.md`, "Sessions").
    /// The project scope is the join, as everywhere else here.
    ///
    /// Nothing about the body is logged (rule 3).
    pub async fn find_comment_in(
        &self,
        mut tx: Locked<'_>,
        project_id: Uuid,
        id: Uuid,
    ) -> Result<Option<TaskComment>> {
        let comment = sqlx::query_as!(
            TaskComment,
            r#"
            SELECT c.id, c.task_id, c.author_user_id, c.author_session_id, c.system, c.body,
                   c.created_at
            FROM task_comments AS c
            JOIN tasks AS t ON t.id = c.task_id
            WHERE c.id = $1 AND t.project_id = $2
            "#,
            id,
            project_id,
        )
        .fetch_optional(&mut *tx)
        .await?;

        Ok(comment)
    }

    /// A task's comments, oldest first.
    ///
    /// `TaskDetail.comments` (`SPEC.md`, "Tasks"), in the order
    /// `task_comments_task_idx (task_id, created_at)` carries. `id` breaks
    /// ties, so two comments written in the same transaction — and therefore
    /// sharing `NOW()` — still come back in a stable order.
    pub async fn list_comments(&self, project_id: Uuid, task_id: Uuid) -> Result<Vec<TaskComment>> {
        let comments = sqlx::query_as!(
            TaskComment,
            r#"
            SELECT c.id, c.task_id, c.author_user_id, c.author_session_id, c.system, c.body,
                   c.created_at
            FROM task_comments AS c
            JOIN tasks AS t ON t.id = c.task_id
            WHERE c.task_id = $1 AND t.project_id = $2
            ORDER BY c.created_at, c.id
            "#,
            task_id,
            project_id,
        )
        .fetch_all(self.pool)
        .await?;

        Ok(comments)
    }
}
