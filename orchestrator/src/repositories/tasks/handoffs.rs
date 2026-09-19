//! `task_handoffs`: the immutable record of code passed between workers.
//!
//! `docs/data-model.md`, `task_handoffs` and `SPEC.md`, "Code hand-offs and
//! review" (ADR 0018). A row is written once and never changed — there is no
//! update and no delete here, only an insert and two reads — so everything
//! that could be wrong about it has to be wrong at insert time.
//!
//! [`NewTaskHandoff::validate`] decides what one record can answer alone: a
//! full object id, one creating actor, a source branch, and review fields that
//! match the status. What it cannot answer is whether the rows it points at
//! are this project's, so that is checked here: the task, the comment that
//! belongs to that task, and every session id it carries.
//!
//! What is deliberately *not* here: which record may be forwarded.
//! "Forwarding requires `handoff_id` to equal the task's current hand-off"
//! (`SPEC.md`) is a comparison against `tasks.current_handoff_id` that the
//! hand-off epic makes under the project lock, together with the git ref, the
//! state move and the lease release it publishes with.

use uuid::Uuid;

use crate::models::{NewTaskHandoff, ReviewStatus, TaskHandoff};
use crate::prelude::*;
use crate::repositories::tasks::{TaskRepository, task_in_project};
use crate::tracker::Locked;

impl TaskRepository<'_> {
    /// Insert a hand-off record.
    ///
    /// Written under the token, together with its comment, the state change,
    /// the lease release and the events, all of which commit together
    /// (`SPEC.md`, "Code hand-offs and review"). The git ref that retains the
    /// commit is prepared *before* the mutation opens — the publication path
    /// validates under the git lock first and needs the token only for the
    /// database half — because a git lock is never taken from inside a
    /// database one (`ARCHITECTURE.md`, "Task tracker").
    ///
    /// The model's own rules run first. Then: the task must belong to this
    /// project ([`Error::NotFound`]); the comment must belong to that task
    /// ([`Error::BadRequest`], `comment must belong to this task`); and every
    /// session named — the source, the creator and the reviewer — must belong
    /// to this project ([`Error::BadRequest`], `sessions must belong to this
    /// project`).
    ///
    /// `review_status` is a `TEXT` column with a `CHECK`, so it is bound as
    /// its stored string and read back through the type override, the same way
    /// [`SecretUsePurpose`](crate::models::SecretUsePurpose) is.
    pub async fn insert_handoff(
        &self,
        mut tx: Locked<'_>,
        project_id: Uuid,
        handoff: &NewTaskHandoff,
    ) -> Result<TaskHandoff> {
        handoff.validate()?;

        if !task_in_project(tx.reborrow(), project_id, handoff.task_id).await? {
            return Err(Error::NotFound);
        }

        let comment_on_task = sqlx::query_scalar!(
            r#"
            SELECT EXISTS (
                SELECT 1 FROM task_comments WHERE id = $1 AND task_id = $2
            ) AS "exists!"
            "#,
            handoff.comment_id,
            handoff.task_id,
        )
        .fetch_one(&mut *tx)
        .await?;
        if !comment_on_task {
            return Err(Error::BadRequest("comment must belong to this task".into()));
        }

        // Counted over the distinct ids, so one session named twice — the
        // creator forwarding its own hand-off, say — is not mistaken for two.
        let mut sessions: Vec<Uuid> = [
            handoff.source_session_id,
            handoff.created_by_session_id,
            handoff.reviewed_by_session_id,
        ]
        .into_iter()
        .flatten()
        .collect();
        sessions.sort_unstable();
        sessions.dedup();

        if !sessions.is_empty() {
            let in_project = sqlx::query_scalar!(
                r#"
                SELECT COUNT(*) AS "count!"
                FROM sessions
                WHERE project_id = $1 AND id = ANY ($2)
                "#,
                project_id,
                &sessions[..],
            )
            .fetch_one(&mut *tx)
            .await?;

            if in_project != sessions.len() as i64 {
                return Err(Error::BadRequest(
                    "sessions must belong to this project".into(),
                ));
            }
        }

        let inserted = sqlx::query_as!(
            TaskHandoff,
            r#"
            INSERT INTO task_handoffs (
                id, task_id, source_session_id, source_branch, commit, comment_id, review_status,
                reviewed_by_user_id, reviewed_by_session_id, reviewed_at, created_by_user_id,
                created_by_session_id
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
            RETURNING id, task_id, source_session_id, source_branch, commit, comment_id,
                      review_status AS "review_status: ReviewStatus", reviewed_by_user_id,
                      reviewed_by_session_id, reviewed_at, created_by_user_id,
                      created_by_session_id, created_at
            "#,
            handoff.id,
            handoff.task_id,
            handoff.source_session_id,
            handoff.source_branch,
            handoff.commit,
            handoff.comment_id,
            handoff.review_status.as_str(),
            handoff.reviewed_by_user_id,
            handoff.reviewed_by_session_id,
            handoff.reviewed_at,
            handoff.created_by_user_id,
            handoff.created_by_session_id,
        )
        .fetch_one(&mut *tx)
        .await?;

        debug!(
            project_id = %project_id,
            task_id = %handoff.task_id,
            handoff_id = %inserted.id,
            review_status = %inserted.review_status,
            "hand-off inserted",
        );

        Ok(inserted)
    }

    /// The hand-off with this id in this project, or `None`.
    ///
    /// The project is derived from the task the record belongs to
    /// (`docs/data-model.md`, `task_handoffs`), so the scope is a join rather
    /// than a column. `GET /projects/{pid}/git/diff?handoff_id=` and the merge
    /// form that names a hand-off both start here: "a hand-off must belong to
    /// the URL project" (`SPEC.md`, "Git").
    pub async fn find_handoff(&self, project_id: Uuid, id: Uuid) -> Result<Option<TaskHandoff>> {
        let handoff = sqlx::query_as!(
            TaskHandoff,
            r#"
            SELECT h.id, h.task_id, h.source_session_id, h.source_branch, h.commit, h.comment_id,
                   h.review_status AS "review_status: ReviewStatus", h.reviewed_by_user_id,
                   h.reviewed_by_session_id, h.reviewed_at, h.created_by_user_id,
                   h.created_by_session_id, h.created_at
            FROM task_handoffs AS h
            JOIN tasks AS t ON t.id = h.task_id
            WHERE h.id = $1 AND t.project_id = $2
            "#,
            id,
            project_id,
        )
        .fetch_optional(self.pool)
        .await?;

        Ok(handoff)
    }

    /// A task's hand-offs, oldest first.
    ///
    /// `TaskDetail.handoffs`, which "are ordered oldest first" (`SPEC.md`,
    /// "Tasks"), in the order `task_handoffs_task_idx (task_id, created_at)`
    /// carries; `id` breaks ties for stability. The *current* record is not
    /// the last of these — it is whichever `tasks.current_handoff_id` names,
    /// "never by timestamp" (`docs/data-model.md`).
    pub async fn list_handoffs(&self, project_id: Uuid, task_id: Uuid) -> Result<Vec<TaskHandoff>> {
        let handoffs = sqlx::query_as!(
            TaskHandoff,
            r#"
            SELECT h.id, h.task_id, h.source_session_id, h.source_branch, h.commit, h.comment_id,
                   h.review_status AS "review_status: ReviewStatus", h.reviewed_by_user_id,
                   h.reviewed_by_session_id, h.reviewed_at, h.created_by_user_id,
                   h.created_by_session_id, h.created_at
            FROM task_handoffs AS h
            JOIN tasks AS t ON t.id = h.task_id
            WHERE h.task_id = $1 AND t.project_id = $2
            ORDER BY h.created_at, h.id
            "#,
            task_id,
            project_id,
        )
        .fetch_all(self.pool)
        .await?;

        Ok(handoffs)
    }
}
