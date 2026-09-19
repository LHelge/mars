//! The dependency-graph rules every other tracker mutation leans on:
//! maintaining the stored `blocked` flag, refusing a `blocks` edge that would
//! close a cycle, and reading what a deletion is about to destroy.
//!
//! `ARCHITECTURE.md`, "Task tracker" → "Blocked is stored" and "Parents";
//! `docs/data-model.md`, `tasks.blocked` and `task_dependencies`.
//!
//! **`blocked` is stored, not derived at read time.** It is recomputed in the
//! same transaction that adds or removes a `blocks` edge, moves a task into or
//! out of a terminal state, creates, deletes, re-parents or moves a child, or
//! deletes a prerequisite — and each flip emits a `blocked` or `unblocked`
//! event, so the board learns about it the same way it learns about everything
//! else. The value itself is purely a function of the graph around the task:
//! "any `blocks` prerequisite in a non-terminal state, or any child in a
//! non-terminal state". The task's own state takes no part, so a terminal task
//! with an open child is `blocked = true`; nothing consults the flag except
//! claimability, which a terminal task fails on its state anyway.
//!
//! **Cycles are refused, but only among `blocks` edges.** They are the only
//! kind that can deadlock a project: a ring of `blocks` edges is a set of
//! tasks none of which can ever be claimed. `discovered_from` and `related`
//! are provenance and commentary, and a ring of either is merely a fact about
//! how work was found. The check is a recursive CTE run *under the project
//! lock and before the insert*, which is what makes two reciprocal edges
//! inserted at once impossible: they serialise on the lock, and the second
//! one's walk sees the first edge (ADR 0021).
//!
//! Every function here takes `&mut TrackerMutation` and opens no transaction
//! of its own. They are steps inside somebody else's mutation — the one that
//! added the edge, changed the state or deleted the task — and their rows,
//! their events and that mutation's own commit or roll back together.

use std::collections::HashSet;

use uuid::Uuid;

use crate::events::TaskEventKind;
use crate::models::TaskDependencyKind;
use crate::prelude::*;
use crate::repositories::TaskRepository;
use crate::tracker::TrackerMutation;

/// The message a refused `blocks` edge carries.
///
/// `SPEC.md`, "Tasks": adding a dependency that would create a cycle is 409.
const CYCLE: &str = "dependency would create a cycle";

/// One task whose `blocked` flag changed, and what it changed to.
///
/// Returned so the caller can see what its change did downstream — a test
/// asserts on it, and a caller that has to act on a newly claimable task has
/// the list without re-reading. A task whose recomputed value equals the
/// stored one is *not* here: nothing changed and nothing was emitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockedFlip {
    /// The task whose flag moved.
    pub task_id: Uuid,
    /// Its new value: `true` for a `blocked` event, `false` for `unblocked`.
    pub blocked: bool,
}

/// Recompute `blocked` for these tasks, and emit an event per flip.
///
/// The one way the flag is ever written. For each id, in the order given, the
/// flag is evaluated from the task's non-terminal `blocks` prerequisites and
/// its non-terminal children; a value that differs from the stored one is
/// written — with `updated_at` — and emitted as `blocked` or `unblocked`
/// carrying the task *as it is after the write*, so the payload shows the new
/// flag. A value that matches writes nothing and emits nothing, which is what
/// makes calling this after every graph change cheap enough to do
/// unconditionally.
///
/// Repeated ids are recomputed once: the set to recompute after a state change
/// is "dependants plus parent", and a task can be both.
///
/// An id that is no longer a task of this project — a dependant that a cascade
/// took with its prerequisite — is skipped rather than failing: the caller
/// assembles the set from edges that its own change is in the middle of
/// erasing.
///
/// An empty set is a no-op, down to not touching the database.
pub async fn recompute_blocked(
    m: &mut TrackerMutation<'_>,
    task_ids: &[Uuid],
) -> Result<Vec<BlockedFlip>> {
    if task_ids.is_empty() {
        return Ok(Vec::new());
    }

    let repository = TaskRepository::new(m.pool());
    let project_id = m.project_id();

    let states = repository
        .compute_blocked(m.conn(), project_id, task_ids)
        .await?;

    let mut flips = Vec::new();
    let mut seen = HashSet::with_capacity(task_ids.len());

    for task_id in task_ids {
        if !seen.insert(*task_id) {
            continue;
        }

        let Some(state) = states.iter().find(|state| state.task_id == *task_id) else {
            // Not a task of this project any more, or never was.
            continue;
        };

        if state.stored == state.computed {
            continue;
        }

        let changed = repository
            .set_blocked(m.conn(), project_id, *task_id, state.computed)
            .await?;
        if !changed {
            continue;
        }

        let task = repository
            .load_task_dto_in(m.conn(), project_id, *task_id)
            .await?
            .ok_or(Error::NotFound)?;

        let kind = if state.computed {
            TaskEventKind::Blocked
        } else {
            TaskEventKind::Unblocked
        };
        m.emit_task(kind, &task)?;

        flips.push(BlockedFlip {
            task_id: *task_id,
            blocked: state.computed,
        });
    }

    Ok(flips)
}

/// The tasks held up by this one: those with a `blocks` edge on it.
///
/// Read under the lock, because the caller is about to change whether this
/// task blocks them.
pub async fn dependants_of(m: &mut TrackerMutation<'_>, task_id: Uuid) -> Result<Vec<Uuid>> {
    let repository = TaskRepository::new(m.pool());
    let project_id = m.project_id();

    repository
        .list_dependants_in_tx(m.conn(), project_id, task_id, TaskDependencyKind::Blocks)
        .await
}

/// Everything whose `blocked` flag a state change of this task can move: its
/// `blocks` dependants and its parent.
///
/// The set to recompute after a task enters *or* leaves a terminal state —
/// both directions, because the flag flips both ways and the events it owes
/// are symmetrical. The parent is in it because a non-terminal child blocks
/// its parent exactly as an open prerequisite blocks a dependant
/// (`ARCHITECTURE.md`, "Task tracker" → "Parents").
///
/// Deduplicated, with the dependants first and the parent last: a task that is
/// both a dependant and the parent is recomputed once, and emits one event.
pub async fn affected_by_state_change(
    m: &mut TrackerMutation<'_>,
    task_id: Uuid,
) -> Result<Vec<Uuid>> {
    let mut affected = dependants_of(m, task_id).await?;

    let repository = TaskRepository::new(m.pool());
    let project_id = m.project_id();

    if let Some(parent_id) = repository
        .parent_of_in_tx(m.conn(), project_id, task_id)
        .await?
        && !affected.contains(&parent_id)
    {
        affected.push(parent_id);
    }

    Ok(affected)
}

/// Refuse a `blocks` edge that would close a cycle.
///
/// Called with the edge the caller is about to insert — `task_id` depends on
/// `depends_on` — under the project lock and *before* the insert, which is the
/// order `docs/data-model.md` prescribes and the reason two reciprocal edges
/// can never both pass: the second mutation waits at the lock and then sees
/// the first edge (ADR 0021).
///
/// The walk starts at `depends_on` and follows `depends_on_task_id` through
/// `blocks` edges only; arriving at `task_id` means the new edge would close a
/// ring, and the answer is [`Error::Conflict`] — 409, as `SPEC.md`, "Tasks"
/// gives it. `depends_on` is in the set the walk starts from, so a self-edge
/// is refused here, as a cycle, before the table's `CHECK (task_id <>
/// depends_on_task_id)` has anything to say about it.
///
/// Nothing else is checked: that both ends are tasks of the same project is
/// `TaskRepository::insert_dependency`'s, and the other two edge kinds are
/// never passed here at all, as start or as path.
pub async fn check_no_cycle(
    m: &mut TrackerMutation<'_>,
    task_id: Uuid,
    depends_on: Uuid,
) -> Result<()> {
    let repository = TaskRepository::new(m.pool());

    if repository
        .blocks_reaches(m.conn(), depends_on, task_id)
        .await?
    {
        return Err(Error::Conflict(CYCLE.into()));
    }

    Ok(())
}

/// What a task's deletion is about to destroy, read while it is still there.
///
/// `ON DELETE CASCADE` takes the task's dependency edges with it and `ON
/// DELETE SET NULL` takes its children's `parent_id`, so after the delete
/// there is no way to find out who was waiting on it or who belonged to it.
/// The deleting mutation therefore captures this first, deletes, and then —
/// from the same transaction — emits `dependency_removed` per surviving
/// dependant per removed kind and recomputes `blocked` for `dependants ∪
/// {parent}` (`docs/data-model.md`, `tasks`).
///
/// The children need no recomputation: they lose a parent, not a prerequisite,
/// and a parent never blocked its children in the first place. They are
/// captured because the deletion has to *describe* what it did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletionCapture {
    /// Every incoming edge, as `(dependant, kind)` — one entry per kind, so a
    /// pair joined by both `blocks` and `discovered_from` appears twice and
    /// each gets its own `dependency_removed`.
    pub dependants: Vec<(Uuid, TaskDependencyKind)>,
    /// The parent that is about to lose a child, if there is one.
    pub parent_id: Option<Uuid>,
    /// The children that are about to lose their parent.
    pub children: Vec<Uuid>,
}

impl DeletionCapture {
    /// The set to recompute once the deletion has cascaded: every dependant,
    /// of any kind, plus the parent, deduplicated.
    ///
    /// The non-blocking kinds are in it harmlessly — recomputing a task whose
    /// flag cannot have moved writes nothing and emits nothing — and leaving
    /// them in keeps the caller from having to filter a list it is already
    /// walking for its `dependency_removed` events.
    pub fn to_recompute(&self) -> Vec<Uuid> {
        let mut ids: Vec<Uuid> = Vec::with_capacity(self.dependants.len() + 1);
        for (task_id, _) in &self.dependants {
            if !ids.contains(task_id) {
                ids.push(*task_id);
            }
        }
        if let Some(parent_id) = self.parent_id
            && !ids.contains(&parent_id)
        {
            ids.push(parent_id);
        }

        ids
    }
}

/// Read a task's incoming edges, parent and children before it is deleted.
///
/// Every kind of incoming edge, not only `blocks`: `dependency_removed` is
/// owed for each one that disappears, and provenance disappears as surely as a
/// blocker does.
pub async fn capture_before_delete(
    m: &mut TrackerMutation<'_>,
    task_id: Uuid,
) -> Result<DeletionCapture> {
    let repository = TaskRepository::new(m.pool());
    let project_id = m.project_id();

    let dependants = repository
        .list_incoming_dependencies_in_tx(m.conn(), project_id, task_id)
        .await?
        .into_iter()
        .map(|edge| (edge.task_id, edge.kind))
        .collect();

    let parent_id = repository
        .parent_of_in_tx(m.conn(), project_id, task_id)
        .await?;

    let children = repository
        .list_child_ids_in_tx(m.conn(), project_id, task_id)
        .await?;

    Ok(DeletionCapture {
        dependants,
        parent_id,
        children,
    })
}
