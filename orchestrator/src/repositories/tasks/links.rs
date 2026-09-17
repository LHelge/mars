//! `task_sessions`: which sessions worked on which tasks.
//!
//! `docs/data-model.md`, `task_sessions` and ADR 0030. The link is history,
//! not configuration: it records that a session *changed* something, so it is
//! written by the transaction that made the change and by nothing else. Reads,
//! rejected operations and updates with no effective change create no link and
//! advance no timestamp — which is why the caller decides when to call this,
//! and why [`TaskRepository::update_task`] answers `Ok(None)` for a no-op
//! instead of quietly writing a link anyway.
//!
//! The upsert carries both halves of the documented behaviour: a repeated
//! touch keeps `first_touched_at` and moves `last_touched_at` only.

use sqlx::PgConnection;
use uuid::Uuid;

use crate::models::{Task, TaskSession};
use crate::prelude::*;
use crate::repositories::tasks::TaskRepository;

impl TaskRepository<'_> {
    /// Record that this session just changed this task.
    ///
    /// **Call only inside a [`TaskRepository::begin_mutation`] transaction**,
    /// the same one as the change: "the link for the directly changed task
    /// commits in the same transaction as the change and its events".
    ///
    /// The first touch inserts; every later one updates `last_touched_at` and
    /// leaves `first_touched_at` exactly as it was. `ON CONFLICT` rather than
    /// a read-then-write because the two are one statement and cannot
    /// interleave; the project lock already keeps this project's writers
    /// apart, and the upsert keeps the statement honest for the session-scoped
    /// writes that do not hold it.
    ///
    /// Both timestamps come from `NOW()`, which is the transaction's start
    /// time, so calling this twice in one transaction is idempotent down to
    /// the timestamp — a hand-off that comments, moves and links in one go
    /// records one touch, not three.
    pub async fn touch_task_session(
        &self,
        tx: &mut PgConnection,
        task_id: Uuid,
        session_id: Uuid,
    ) -> Result<TaskSession> {
        let link = sqlx::query_as!(
            TaskSession,
            r#"
            INSERT INTO task_sessions (task_id, session_id)
            VALUES ($1, $2)
            ON CONFLICT (task_id, session_id) DO UPDATE SET last_touched_at = NOW()
            RETURNING task_id, session_id, first_touched_at, last_touched_at
            "#,
            task_id,
            session_id,
        )
        .fetch_one(&mut *tx)
        .await?;

        debug!(task_id = %task_id, session_id = %session_id, "task session link touched");

        Ok(link)
    }

    /// The sessions that worked on this task, in the order they first did.
    ///
    /// `TaskDetail.sessions` (`SPEC.md`, "Tasks"). `session_id` breaks ties,
    /// so two links created in the same transaction still come back in a
    /// stable order.
    pub async fn list_task_sessions(&self, task_id: Uuid) -> Result<Vec<TaskSession>> {
        let links = sqlx::query_as!(
            TaskSession,
            r#"
            SELECT task_id, session_id, first_touched_at, last_touched_at
            FROM task_sessions
            WHERE task_id = $1
            ORDER BY first_touched_at, session_id
            "#,
            task_id,
        )
        .fetch_all(self.pool)
        .await?;

        Ok(links)
    }

    /// The tasks this session touched, most recently touched first.
    ///
    /// `GET /sessions/{id}/tasks` → "`Task[]` touched by this session"
    /// (`SPEC.md`, "Sessions"), served by `task_sessions_session_idx`. Not
    /// project-scoped: a session belongs to one project, and every task it can
    /// have touched belongs to the same one.
    pub async fn list_tasks_for_session(&self, session_id: Uuid) -> Result<Vec<Task>> {
        let tasks = sqlx::query_as!(
            Task,
            r#"
            SELECT t.id, t.project_id, t.number, t.title, t.description, t.state_id, t.priority,
                   t.blocked, t.labels, t.parent_id, t.assignee_user_id,
                   t.lease_holder_session_id, t.lease_since, t.attempts, t.needs_human_reason,
                   t.current_handoff_id, t.created_by_user_id, t.created_by_session_id,
                   t.created_at, t.updated_at, t.closed_at
            FROM task_sessions AS l
            JOIN tasks AS t ON t.id = l.task_id
            WHERE l.session_id = $1
            ORDER BY l.last_touched_at DESC, t.number
            "#,
            session_id,
        )
        .fetch_all(self.pool)
        .await?;

        Ok(tasks)
    }
}
