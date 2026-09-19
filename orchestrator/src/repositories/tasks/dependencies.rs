//! `task_dependencies`: the edges between tasks, and the two reads the
//! `blocked` flag is recomputed from.
//!
//! `docs/data-model.md`, `task_dependencies` and `SPEC.md`, "Tasks". The edge
//! is identified by `(task_id, depends_on_task_id, kind)`, so the same pair
//! may carry a `blocks` edge and the `discovered_from` edge that records where
//! the task came from, and removing the blocker leaves the provenance intact.
//!
//! One rule here has no constraint behind it: both tasks must belong to the
//! same project. A cross-table check needs a trigger, so it is a repository
//! check made under the project lock, like the parent rules next door.
//!
//! What is deliberately *not* here: the cycle check and the `blocked`
//! recomputation. Adding a `blocks` edge must first prove the edge creates no
//! cycle, with a recursive CTE under the project lock, and must afterwards
//! recompute `blocked` for the dependant and emit the events that follow — all
//! in this transaction. Both are compositions the tracker epic owns;
//! [`TaskRepository::list_dependants`] is the read they walk.

use uuid::Uuid;

use crate::models::{TaskDependency, TaskDependencyKind};
use crate::prelude::*;
use crate::repositories::tasks::TaskRepository;
use crate::repositories::unique_violation;
use crate::tracker::Locked;

impl TaskRepository<'_> {
    /// Insert one dependency edge.
    ///
    /// For a `blocks` edge, called after the cycle check and before the
    /// `blocked` recomputation the same mutation owes.
    ///
    /// A task cannot depend on itself ([`TaskError::SelfDependency`], 400,
    /// which `CHECK (task_id <> depends_on_task_id)` also refuses). Both ends
    /// must be tasks of `project_id`: [`Error::BadRequest`] with `dependency
    /// must reference tasks of the same project`, which is also the answer an
    /// id from another project gets, so nothing leaks about whether it exists.
    /// The same edge twice is [`Error::Conflict`] with `dependency already
    /// exists` — and *the same edge* means the same kind too, so adding
    /// `discovered_from` to a pair that already blocks succeeds.
    ///
    /// [`TaskError::SelfDependency`]: crate::models::TaskError::SelfDependency
    pub async fn insert_dependency(
        &self,
        mut tx: Locked<'_>,
        project_id: Uuid,
        task_id: Uuid,
        depends_on_task_id: Uuid,
        kind: TaskDependencyKind,
    ) -> Result<TaskDependency> {
        let edge = TaskDependency::new(task_id, depends_on_task_id, kind)?;

        let ends = [task_id, depends_on_task_id];
        let in_project = sqlx::query_scalar!(
            r#"SELECT COUNT(*) AS "count!" FROM tasks WHERE project_id = $1 AND id = ANY ($2)"#,
            project_id,
            &ends[..],
        )
        .fetch_one(&mut *tx)
        .await?;

        if in_project != ends.len() as i64 {
            return Err(Error::BadRequest(
                "dependency must reference tasks of the same project".into(),
            ));
        }

        sqlx::query!(
            "INSERT INTO task_dependencies (task_id, depends_on_task_id, kind) VALUES ($1, $2, $3)",
            task_id,
            depends_on_task_id,
            kind as TaskDependencyKind,
        )
        .execute(&mut *tx)
        .await
        .map_err(map_dependency_error)?;

        debug!(
            project_id = %project_id,
            task_id = %task_id,
            depends_on_task_id = %depends_on_task_id,
            kind = ?kind,
            "dependency inserted",
        );

        Ok(edge)
    }

    /// Remove one dependency edge; `false` when it was not there.
    ///
    /// The mutation then recomputes the dependant's `blocked` flag and emits
    /// `dependency_removed` (`SPEC.md`, "Tasks").
    ///
    /// `kind` is part of the identity, so this removes that kind and leaves
    /// any other edge between the same pair standing — `DELETE
    /// /projects/{pid}/tasks/{id}/dependencies/{dep}?kind=blocks` "removes
    /// only that kind". The project scope is joined in rather than assumed:
    /// an edge of another project is not this caller's to remove.
    pub async fn delete_dependency(
        &self,
        mut tx: Locked<'_>,
        project_id: Uuid,
        task_id: Uuid,
        depends_on_task_id: Uuid,
        kind: TaskDependencyKind,
    ) -> Result<bool> {
        let deleted = sqlx::query!(
            r#"
            DELETE FROM task_dependencies AS d
            USING tasks AS t
            WHERE d.task_id = $1
              AND d.depends_on_task_id = $2
              AND d.kind = $3
              AND t.id = d.task_id
              AND t.project_id = $4
            "#,
            task_id,
            depends_on_task_id,
            kind as TaskDependencyKind,
            project_id,
        )
        .execute(&mut *tx)
        .await?
        .rows_affected()
            > 0;

        if deleted {
            debug!(
                project_id = %project_id,
                task_id = %task_id,
                depends_on_task_id = %depends_on_task_id,
                kind = ?kind,
                "dependency deleted",
            );
        }

        Ok(deleted)
    }

    /// Everything this task depends on, of every kind.
    ///
    /// `Task.depends_on`, which "lists every outgoing dependency with its
    /// kind" (`SPEC.md`, "Tasks"). Ordered by the other end and then the kind,
    /// so a pair carrying two edges lists them together and the order is
    /// stable between reads.
    pub async fn list_dependencies(
        &self,
        project_id: Uuid,
        task_id: Uuid,
    ) -> Result<Vec<TaskDependency>> {
        let edges = sqlx::query_as!(
            TaskDependency,
            r#"
            SELECT d.task_id, d.depends_on_task_id, d.kind AS "kind: TaskDependencyKind"
            FROM task_dependencies AS d
            JOIN tasks AS t ON t.id = d.task_id
            WHERE d.task_id = $1 AND t.project_id = $2
            ORDER BY d.depends_on_task_id, d.kind
            "#,
            task_id,
            project_id,
        )
        .fetch_all(self.pool)
        .await?;

        Ok(edges)
    }

    /// The tasks with an edge of this kind pointing at this one.
    ///
    /// "Who is waiting on me", served by
    /// `task_dependencies_depends_on_idx`. With
    /// [`TaskDependencyKind::Blocks`] it is both `Task.blocks` — the list
    /// `SPEC.md` puts on the task — and the set whose `blocked` flags are
    /// recomputed whenever this task enters or leaves a terminal state, or is
    /// about to be deleted.
    ///
    /// Read on the pool, so a recomputation reads it through its own
    /// transaction instead when the answer has to be authoritative.
    pub async fn list_dependants(
        &self,
        project_id: Uuid,
        task_id: Uuid,
        kind: TaskDependencyKind,
    ) -> Result<Vec<Uuid>> {
        let dependants = sqlx::query_scalar!(
            r#"
            SELECT d.task_id
            FROM task_dependencies AS d
            JOIN tasks AS t ON t.id = d.task_id
            WHERE d.depends_on_task_id = $1 AND d.kind = $2 AND t.project_id = $3
            ORDER BY d.task_id
            "#,
            task_id,
            kind as TaskDependencyKind,
            project_id,
        )
        .fetch_all(self.pool)
        .await?;

        Ok(dependants)
    }

    /// [`TaskRepository::list_dependants`] under the caller's lock.
    ///
    /// The recomputation that follows a terminal move, and the capture a
    /// deletion makes before the cascades erase the edges, both need the list
    /// as it is inside the mutation, not as it was when the pool answered —
    /// which is why this one takes the token and its sibling does not.
    pub async fn list_dependants_in_tx(
        &self,
        mut tx: Locked<'_>,
        project_id: Uuid,
        task_id: Uuid,
        kind: TaskDependencyKind,
    ) -> Result<Vec<Uuid>> {
        let dependants = sqlx::query_scalar!(
            r#"
            SELECT d.task_id
            FROM task_dependencies AS d
            JOIN tasks AS t ON t.id = d.task_id
            WHERE d.depends_on_task_id = $1 AND d.kind = $2 AND t.project_id = $3
            ORDER BY d.task_id
            "#,
            task_id,
            kind as TaskDependencyKind,
            project_id,
        )
        .fetch_all(&mut *tx)
        .await?;

        Ok(dependants)
    }
}

/// Map the `task_dependencies` primary key to the documented conflict.
///
/// `(task_id, depends_on_task_id, kind)` is the whole key, so this fires only
/// for an edge that is there already, kind and all.
fn map_dependency_error(err: sqlx::Error) -> Error {
    if let Some("task_dependencies_pkey") = unique_violation(&err) {
        return Error::Conflict("dependency already exists".into());
    }

    Error::from(err)
}
