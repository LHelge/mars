//! Adding and removing one dependency edge, with everything the edge owes.
//!
//! `SPEC.md`, "Tasks" (`POST` and `DELETE
//! /projects/{pid}/tasks/{id}/dependencies`) and "MCP tool contracts" →
//! `update` (`add_depends_on`, `remove_depends_on`); `ARCHITECTURE.md`, "Task
//! tracker" → "Blocked is stored"; `docs/data-model.md`,
//! `task_dependencies`.
//!
//! An edge is identified by `(task, depends_on, kind)`, so the three kinds are
//! independent facts about the same pair: `discovered_from` may be added
//! beside `blocks`, and removing the blocker leaves the provenance standing.
//! Only `blocks` means anything to the orchestrator, and both of the rules
//! that follow from that are applied here rather than in the repository, which
//! has no view of the graph:
//!
//! - **the cycle check** runs for `blocks` alone, under the project lock and
//!   before the insert, so two reciprocal edges requested at once cannot both
//!   be accepted: they serialise on the lock and the second one's walk sees the
//!   first edge (ADR 0021);
//! - **`blocked` is recomputed** for the dependant alone, for `blocks` alone,
//!   and each flip emits its own `blocked` or `unblocked` event
//!   ([`recompute_blocked`]).
//!
//! The event order inside one call is therefore `dependency_added` (or
//! `dependency_removed`) and then the flip, which is the order the two facts
//! became true: the edge exists, and because it does the task is blocked.
//!
//! Both functions take the two tasks as rows read under this mutation's lock,
//! never as references: a per-project number has to be resolved inside the lock
//! or a concurrent deletion could swap the identity out from under it, which is
//! what [`resolve_dependency`] is for. Neither opens a transaction; they are
//! steps in somebody else's mutation and commit or roll back with it.

use uuid::Uuid;

use crate::events::TaskEventKind;
use crate::models::{Task, TaskDependency, TaskDependencyKind, TaskRef};
use crate::prelude::*;
use crate::repositories::TaskRepository;
use crate::tracker::graph::{check_no_cycle, recompute_blocked};
use crate::tracker::{TaskDto, TrackerMutation};

/// What an edge with an end outside this project is told (400).
///
/// The message [`TaskRepository::insert_dependency`] gives the same case, so
/// the answer does not depend on which check noticed first.
pub(crate) const DEPENDENCY_SCOPE: &str = "dependency must reference tasks of the same project";

/// What a removal of an edge that is not there is told (404).
///
/// Named rather than the generic `not found`, because the two tasks were both
/// resolved before this point: the only thing missing is the edge, and a
/// caller that asked for it to be gone can read this one as "it already is"
/// instead of retrying against what it thinks is a vanished task (`SPEC.md`,
/// "Tasks"). An unknown project, `{id}` or `{dep}` keeps `not found`.
pub(crate) const DEPENDENCY_NOT_FOUND: &str = "dependency not found";

/// The task a `depends_on` value names, wherever it is.
///
/// A [`TaskRef::Number`] is a *per-project* number and is resolved inside this
/// project and nowhere else: a number another project carries names no task
/// here, and the caller answers 404. A [`TaskRef::Id`] is global, so a UUID
/// this project does not carry is looked up once more without the scope — and
/// when it names a task of another project it is returned all the same, so
/// that [`add_dependency`] refuses it with the documented 400 rather than
/// pretending it does not exist (`SPEC.md`, "Tasks").
///
/// Resolved under the lock, and `FOR NO KEY UPDATE` for the in-project case, because
/// the caller is about to act on the row it gets back.
pub async fn resolve_dependency(m: &mut TrackerMutation<'_>, reference: TaskRef) -> Result<Task> {
    let repository = TaskRepository::new(m.pool());
    let project_id = m.project_id();

    if let Some(task) = repository
        .find_task_for_update(m.conn(), project_id, reference)
        .await?
    {
        return Ok(task);
    }

    if let TaskRef::Id(id) = reference
        && let Some(elsewhere) = repository.find_task_in_any_project_in(m.conn(), id).await?
    {
        return Ok(elsewhere);
    }

    Err(Error::NotFound)
}

/// Add one edge from `task` to `depends_on`, and answer with the dependant.
///
/// In order, every step being a documented rule:
///
/// - **both ends belong to this project** — 400 [`DEPENDENCY_SCOPE`], which is
///   also what a UUID from another project gets, because the edge is illegal
///   either way;
/// - **a task cannot depend on itself** — 400, the model's own rule. Checked
///   before the cycle walk on purpose: the walk starts at `depends_on` and
///   would answer a self-edge with 409 instead of the 400 it is;
/// - **a `blocks` edge may close no cycle** — 409 `dependency would create a
///   cycle`. The other two kinds are provenance and commentary and are never
///   checked (`tracker::graph`);
/// - **the same edge twice** is 409 `dependency already exists`, kind and all,
///   from the primary key;
/// - **`dependency_added`** carries the dependant as it is *after* the insert;
/// - **`blocked`** is recomputed for the dependant, for a `blocks` edge alone,
///   and emits its flip after the edge's own event.
///
/// The returned [`TaskDto`] is read last, so its `depends_on` and its `blocked`
/// are both current — which is what the caller answers 200 with.
pub async fn add_dependency(
    m: &mut TrackerMutation<'_>,
    task: &Task,
    depends_on: &Task,
    kind: TaskDependencyKind,
) -> Result<TaskDto> {
    let project_id = m.project_id();
    if task.project_id != project_id || depends_on.project_id != project_id {
        return Err(Error::BadRequest(DEPENDENCY_SCOPE.into()));
    }

    // The model's self-dependency rule, before anything touches the graph.
    TaskDependency::new(task.id, depends_on.id, kind)?;

    if kind.is_blocking() {
        check_no_cycle(m, task.id, depends_on.id).await?;
    }

    let repository = TaskRepository::new(m.pool());
    repository
        .insert_dependency(m.conn(), project_id, task.id, depends_on.id, kind)
        .await?;

    let dependant = load(m, task.id).await?;
    m.emit_task(TaskEventKind::DependencyAdded, &dependant)?;

    if kind.is_blocking() {
        recompute_blocked(m, &[task.id]).await?;
    }

    m.touch_actor(task.id);

    info!(
        project_id = %project_id,
        task_id = %task.id,
        depends_on_task_id = %depends_on.id,
        kind = ?kind,
        "dependency added",
    );

    // Once more: the recomputation may have moved `blocked` since the event
    // payload was taken, and the caller answers with this.
    load(m, task.id).await
}

/// Remove one edge of exactly this kind, and answer with the dependant.
///
/// `kind` is part of the edge's identity, so this leaves any other edge
/// between the same pair standing — removing `blocks` from a pair that also
/// carries `discovered_from` keeps the provenance, which is the whole reason
/// the kind is in the primary key (`ARCHITECTURE.md`, "Task tracker" →
/// "Blocked is stored").
///
/// An edge that is not there is 404 [`DEPENDENCY_NOT_FOUND`]: nothing is
/// written, nothing is emitted, and the mutation rolls back. Otherwise
/// `dependency_removed` carries the dependant as it is after the delete, and a
/// `blocks` removal recomputes its `blocked` flag — which unblocks it when no
/// other open prerequisite and no open child remains, and leaves it alone when
/// one does.
pub async fn remove_dependency(
    m: &mut TrackerMutation<'_>,
    task: &Task,
    depends_on: &Task,
    kind: TaskDependencyKind,
) -> Result<TaskDto> {
    let project_id = m.project_id();
    if task.project_id != project_id || depends_on.project_id != project_id {
        return Err(Error::BadRequest(DEPENDENCY_SCOPE.into()));
    }

    let repository = TaskRepository::new(m.pool());
    let removed = repository
        .delete_dependency(m.conn(), project_id, task.id, depends_on.id, kind)
        .await?;
    if !removed {
        return Err(Error::Missing(DEPENDENCY_NOT_FOUND.into()));
    }

    let dependant = load(m, task.id).await?;
    m.emit_task(TaskEventKind::DependencyRemoved, &dependant)?;

    if kind.is_blocking() {
        recompute_blocked(m, &[task.id]).await?;
    }

    m.touch_actor(task.id);

    info!(
        project_id = %project_id,
        task_id = %task.id,
        depends_on_task_id = %depends_on.id,
        kind = ?kind,
        "dependency removed",
    );

    load(m, task.id).await
}

/// This mutation's view of a task of this project, which must be there.
///
/// Both functions read the dependant twice — once for the event payload and
/// once for the answer — and the row is under this mutation's own lock, so a
/// miss is [`Error::Internal`]'s territory rather than a 404 the caller could
/// have caused. [`Error::NotFound`] is what it maps to all the same, as it does
/// everywhere else in the tracker.
async fn load(m: &mut TrackerMutation<'_>, task_id: Uuid) -> Result<TaskDto> {
    let repository = TaskRepository::new(m.pool());
    let project_id = m.project_id();

    repository
        .load_task_dto_in(m.conn(), project_id, task_id)
        .await?
        .ok_or(Error::NotFound)
}
