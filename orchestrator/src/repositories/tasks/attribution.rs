//! Which hand-offs pinned a set of commits: the database half of attributing
//! an integration head's history to tasks and sessions (`ARCHITECTURE.md`,
//! "Git model", History; `SPEC.md`, "Git": `HistoryEntry`).
//!
//! The git half — which commits an entry, or a range, brought in — is
//! `git::history`; the composition of the two is
//! [`crate::git::GitService::attribute_commits`].

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::prelude::*;
use crate::repositories::tasks::TaskRepository;

/// One `task_handoffs` row of this project whose commit was asked about, with
/// its task and the session it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitHandoff {
    /// The pinned commit, a full object id.
    pub commit: String,
    /// The hand-off record.
    pub handoff_id: Uuid,
    /// When it was recorded: a forward copies its commit, so one commit can
    /// have several records, and the newest is the one a listing names.
    pub created_at: DateTime<Utc>,
    /// The task it belongs to.
    pub task_id: Uuid,
    /// That task's per-project number.
    pub task_number: i32,
    /// That task's title.
    pub task_title: String,
    /// The session the code came from, or `None` once it was deleted.
    pub source_session_id: Option<Uuid>,
    /// That session's title, when it has one.
    pub source_session_title: Option<String>,
}

impl TaskRepository<'_> {
    /// Every hand-off of `project_id` that pins one of `commits`, oldest
    /// first.
    ///
    /// A plain read with no lock: hand-off rows are immutable once inserted
    /// (`docs/data-model.md`, `task_handoffs`), and a record published a
    /// moment later names a commit the listing it would be added to already
    /// described. The scope is the task's project, in the `WHERE` clause.
    pub async fn handoffs_for_commits(
        &self,
        project_id: Uuid,
        commits: &[String],
    ) -> Result<Vec<CommitHandoff>> {
        if commits.is_empty() {
            return Ok(Vec::new());
        }

        let rows = sqlx::query_as!(
            CommitHandoff,
            r#"
            SELECT h.commit, h.id AS handoff_id, h.created_at,
                   t.id AS task_id, t.number AS task_number, t.title AS task_title,
                   h.source_session_id, s.title AS "source_session_title?"
            FROM task_handoffs AS h
            JOIN tasks AS t ON t.id = h.task_id
            LEFT JOIN sessions AS s ON s.id = h.source_session_id
            WHERE t.project_id = $1 AND h.commit = ANY($2)
            ORDER BY h.created_at, h.id
            "#,
            project_id,
            commits,
        )
        .fetch_all(self.pool)
        .await?;

        Ok(rows)
    }
}
