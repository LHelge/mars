//! The SQL the dependency graph is made of: what `blocked` evaluates to, the
//! write that stores it, the reachability question the cycle check asks, and
//! the three reads a deletion takes before the cascades erase them.
//!
//! `docs/data-model.md`, `tasks.blocked` ("True while any `blocks` dependency
//! or any child is in a non-terminal state") and `task_dependencies` ("cycles
//! among `blocks` edges are rejected in the repository with a recursive CTE
//! after locking the project row and before insert").
//!
//! Every helper here takes the [`Locked`] token, because every one of them is
//! either a write or a read whose answer the caller immediately acts on: a
//! recomputation from a stale prerequisite set, or a cycle check against a
//! graph another mutation is still changing, is worse than no check at all.
//! The compositions on top — which set to recompute, which events a flip owes
//! the stream — are `tracker::graph`'s.

use uuid::Uuid;

use crate::models::{TaskDependency, TaskDependencyKind};
use crate::prelude::*;
use crate::repositories::tasks::TaskRepository;
use crate::tracker::Locked;

/// What `blocked` is, and what it says it is, for one task.
///
/// Both halves in one row so the caller can tell a flip from a no-op without
/// a second read: `stored` is the column, `computed` is what the graph says it
/// should be, and they differ exactly when an event is owed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockedState {
    /// The task the row is about.
    pub task_id: Uuid,
    /// `tasks.blocked` as it stands in this transaction.
    pub stored: bool,
    /// What the prerequisites and children make it.
    pub computed: bool,
}

impl TaskRepository<'_> {
    /// Evaluate `blocked` for a whole set of tasks in one statement.
    ///
    /// The definition, straight from `docs/data-model.md`: a task is blocked
    /// while any task it has a `blocks` edge to, or any of its children, is in
    /// a state whose kind is not `terminal`. The other dependency kinds take
    /// no part, and neither does the task's own kind — a terminal task with an
    /// open child is `blocked = true`, which is harmless because the flag is
    /// only ever consulted for claimability.
    ///
    /// Ids that are not tasks of this project are simply absent from the
    /// answer; the caller skips them rather than failing, because a set to
    /// recompute is assembled from edges that a cascade may already have taken
    /// with it.
    ///
    /// One statement for the set, however large: the recomputation after a
    /// terminal move can touch every dependant of a task, and a query per edge
    /// would make a fan-out of fifty into fifty round trips under the project
    /// lock.
    pub async fn compute_blocked(
        &self,
        mut tx: Locked<'_>,
        project_id: Uuid,
        task_ids: &[Uuid],
    ) -> Result<Vec<BlockedState>> {
        if task_ids.is_empty() {
            return Ok(Vec::new());
        }

        let rows = sqlx::query!(
            r#"
            SELECT t.id AS "task_id!",
                   t.blocked AS "stored!",
                   (
                       EXISTS (
                           SELECT 1
                           FROM task_dependencies AS d
                           JOIN tasks AS p ON p.id = d.depends_on_task_id
                           JOIN task_states AS ps ON ps.id = p.state_id
                           WHERE d.task_id = t.id
                             AND d.kind = 'blocks'::task_dependency_kind
                             AND ps.kind <> 'terminal'::task_state_kind
                       )
                       OR EXISTS (
                           SELECT 1
                           FROM tasks AS c
                           JOIN task_states AS cs ON cs.id = c.state_id
                           WHERE c.parent_id = t.id
                             AND cs.kind <> 'terminal'::task_state_kind
                       )
                   ) AS "computed!"
            FROM tasks AS t
            WHERE t.project_id = $1 AND t.id = ANY ($2)
            "#,
            project_id,
            task_ids,
        )
        .fetch_all(&mut *tx)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| BlockedState {
                task_id: row.task_id,
                stored: row.stored,
                computed: row.computed,
            })
            .collect())
    }

    /// Store a recomputed `blocked` flag; `false` when it was already that.
    ///
    /// The `blocked <> $3` in the `WHERE` clause is what makes the answer
    /// meaningful: the row — and `updated_at` with it — is only touched when
    /// the value actually flips, so a recomputation that changes nothing
    /// leaves no trace and owes the stream no event.
    pub async fn set_blocked(
        &self,
        mut tx: Locked<'_>,
        project_id: Uuid,
        task_id: Uuid,
        blocked: bool,
    ) -> Result<bool> {
        let changed = sqlx::query!(
            r#"
            UPDATE tasks
            SET blocked = $3, updated_at = NOW()
            WHERE id = $1 AND project_id = $2 AND blocked <> $3
            "#,
            task_id,
            project_id,
            blocked,
        )
        .execute(&mut *tx)
        .await?
        .rows_affected()
            > 0;

        if changed {
            debug!(
                project_id = %project_id,
                task_id = %task_id,
                blocked,
                "task blocked flag recomputed",
            );
        }

        Ok(changed)
    }

    /// Has this task any child in a non-terminal state?
    ///
    /// The question automatic parent closure turns on: "when the last
    /// non-terminal child of a non-terminal parent enters a terminal state,
    /// the same transaction moves the parent" (`docs/data-model.md`, `tasks`;
    /// `ARCHITECTURE.md`, "Task tracker" → "Parents"). It is deliberately not
    /// the parent's `blocked` flag, which is also true while a `blocks`
    /// prerequisite is open — a parent whose children have all closed is
    /// closed even if something else still blocks it.
    ///
    /// Takes the token because the caller asks it in the middle of closing one
    /// of those children: the answer must include the row this transaction has
    /// just written and nobody else's uncommitted ones.
    pub async fn has_open_children_in_tx(
        &self,
        mut tx: Locked<'_>,
        project_id: Uuid,
        parent_id: Uuid,
    ) -> Result<bool> {
        let open = sqlx::query_scalar!(
            r#"
            SELECT EXISTS (
                SELECT 1
                FROM tasks AS c
                JOIN task_states AS cs ON cs.id = c.state_id
                WHERE c.project_id = $1
                  AND c.parent_id = $2
                  AND cs.kind <> 'terminal'::task_state_kind
            ) AS "open!"
            "#,
            project_id,
            parent_id,
        )
        .fetch_one(&mut *tx)
        .await?;

        Ok(open)
    }

    /// Is `target` reachable from `from` by following `blocks` edges?
    ///
    /// The cycle check's one question, as the recursive CTE
    /// `docs/data-model.md` prescribes: start at `from`, follow
    /// `depends_on_task_id`, and see whether the walk arrives at `target`.
    /// `from` is in the set it starts from, so `from == target` answers `true`
    /// — a self-edge is a cycle of length one and is refused here rather than
    /// by the table's `CHECK`.
    ///
    /// Only `blocks` edges are traversed. `discovered_from` and `related` are
    /// provenance and commentary; a ring of them is not a deadlock and is not
    /// this query's business.
    ///
    /// Takes the token because the answer is only true for as long as the
    /// project lock is held: two reciprocal edges inserted at once would both
    /// pass a check made outside it (ADR 0021).
    pub async fn blocks_reaches(
        &self,
        mut tx: Locked<'_>,
        from: Uuid,
        target: Uuid,
    ) -> Result<bool> {
        let reaches = sqlx::query_scalar!(
            r#"
            WITH RECURSIVE reach(id) AS (
                SELECT $1::uuid
                UNION
                SELECT d.depends_on_task_id
                FROM task_dependencies AS d
                JOIN reach AS r ON d.task_id = r.id
                WHERE d.kind = 'blocks'::task_dependency_kind
            )
            SELECT EXISTS (SELECT 1 FROM reach WHERE id = $2) AS "reaches!"
            "#,
            from,
            target,
        )
        .fetch_one(&mut *tx)
        .await?;

        Ok(reaches)
    }

    /// Every edge pointing *at* this task, of every kind.
    ///
    /// What a deletion has to read before it happens: `ON DELETE CASCADE`
    /// takes these rows with the task, and afterwards there is no way to find
    /// out who was waiting on it or why. Both the dependant and the kind are
    /// returned, because `dependency_removed` is emitted per surviving
    /// dependant *per removed kind* (`SPEC.md`, "Tasks").
    ///
    /// Served by `task_dependencies_depends_on_idx`, and ordered by the
    /// dependant and then the kind so a pair carrying two edges lists them
    /// together.
    pub async fn list_incoming_dependencies_in_tx(
        &self,
        mut tx: Locked<'_>,
        project_id: Uuid,
        task_id: Uuid,
    ) -> Result<Vec<TaskDependency>> {
        let edges = sqlx::query_as!(
            TaskDependency,
            r#"
            SELECT d.task_id, d.depends_on_task_id, d.kind AS "kind: TaskDependencyKind"
            FROM task_dependencies AS d
            JOIN tasks AS t ON t.id = d.task_id
            WHERE d.depends_on_task_id = $1 AND t.project_id = $2
            ORDER BY d.task_id, d.kind
            "#,
            task_id,
            project_id,
        )
        .fetch_all(&mut *tx)
        .await?;

        Ok(edges)
    }

    /// This task's parent, read under the lock.
    ///
    /// `None` for a top-level task and for an id this project does not carry;
    /// the caller treats both the same way, since a parent that is not there
    /// is nothing to recompute either way.
    pub async fn parent_of_in_tx(
        &self,
        mut tx: Locked<'_>,
        project_id: Uuid,
        task_id: Uuid,
    ) -> Result<Option<Uuid>> {
        let parent = sqlx::query_scalar!(
            "SELECT parent_id FROM tasks WHERE id = $1 AND project_id = $2",
            task_id,
            project_id,
        )
        .fetch_optional(&mut *tx)
        .await?
        .flatten();

        Ok(parent)
    }

    /// This task's children, read under the lock, in board order.
    ///
    /// [`TaskRepository::list_children`](super::TaskRepository::list_children)
    /// answers the same question on the pool for a detail read; this one is
    /// for a mutation that is about to change what the answer is.
    pub async fn list_child_ids_in_tx(
        &self,
        mut tx: Locked<'_>,
        project_id: Uuid,
        parent_id: Uuid,
    ) -> Result<Vec<Uuid>> {
        let children = sqlx::query_scalar!(
            r#"
            SELECT id
            FROM tasks
            WHERE project_id = $1 AND parent_id = $2
            ORDER BY priority, number
            "#,
            project_id,
            parent_id,
        )
        .fetch_all(&mut *tx)
        .await?;

        Ok(children)
    }
}
