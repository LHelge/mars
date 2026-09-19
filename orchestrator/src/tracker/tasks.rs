//! Creating a task: the one code path `POST /projects/{pid}/tasks` and the
//! MCP `create_task` tool both go through (`SPEC.md`, "Tasks";
//! `ARCHITECTURE.md`, "Task tracker" → "Parents" and "Blocked is stored").
//!
//! Everything a creation owes happens inside one [`TrackerMutation`], so the
//! row, its dependency edges, the `blocked` flags they flip and every event
//! that describes them commit together or not at all (ADR 0021, 0028):
//!
//! ```text
//! resolve the state (400) → resolve the provenance (400/404, sessions only)
//!   → insert the row with its allocated number
//!   → per dependency: cycle check (409) → insert the `blocks` edge (400)
//!   → insert the `discovered_from` edge, if there is one to record
//!   → emit `created` (the task *after* the edges)
//!   → emit one `dependency_added` per edge, in `depends_on` order, then one
//!     for the `discovered_from` edge
//!   → recompute `blocked` for the task and its parent, emitting the flips
//! ```
//!
//! The provenance is resolved *before* the insert although its edge is written
//! after it: what it validates — the origin exists, it is held by this caller,
//! or the caller holds exactly one task to infer it from — decides whether
//! there is to be a task at all, and a rejected creation leaves nothing behind
//! (`ARCHITECTURE.md`, "Task tracker" → "Discovery provenance").
//!
//! The `created` payload is loaded after the edges are inserted rather than
//! straight from the insert, because `SPEC.md`, "TaskEvent" says the payload
//! is "the full task after the change" and a task created with prerequisites
//! has them from the moment it exists. The `blocked` flag is the one thing
//! that is *not* folded into it: it is stored, it is recomputed by
//! [`recompute_blocked`], and each flip is its own event, so a task created
//! behind an open prerequisite emits `created`, `dependency_added` and then
//! `blocked` — three facts in the order they became true.
//!
//! No route, no tool and no lock handling here: the caller opens the mutation,
//! this fills it, and the caller commits it.
//!
//! The same module carries the other two whole-task changes, on the same
//! terms: [`update_task`], which is `PUT /projects/{pid}/tasks/{id}` and the
//! MCP `update` tool, and [`delete_task`], which is `DELETE` on the same path.
//! Each one's own doc comment gives its order; what they share is that the
//! caller supplies the row read under the lock and gets back a description of
//! what moved, never a transaction of their own.

use chrono::Utc;
use uuid::Uuid;

use crate::events::TaskEventKind;
use crate::models::{
    Label, NewTask, Priority, Task, TaskDependencyKind, TaskRef, TaskStateKind, TaskTitle,
    TaskUpdate,
};
use crate::prelude::*;
use crate::repositories::TaskRepository;
use crate::repositories::foreign_key_violation;
use crate::repositories::tasks::StateFields;
// What a `depends_on` entry naming nothing in this project is told (400).
// Creation resolves every entry inside this project, so an id from another
// project and an id from nowhere get the same answer here — the message the
// dependency endpoint gives an out-of-project end, stated once.
use crate::tracker::dependencies::DEPENDENCY_SCOPE;
use crate::tracker::graph::{capture_before_delete, check_no_cycle, recompute_blocked};
use crate::tracker::provenance::resolve_origin;
use crate::tracker::state::{StateChangeOptions, StateEventKind, change_state, resolve_state};
use crate::tracker::{TaskDto, TrackerMutation};

/// Who is creating the task (`docs/data-model.md`, `tasks`).
///
/// `tasks` carries `created_by_user_id` and `created_by_session_id`, exactly
/// one of which is set; this is that column pair as a type, so a creation
/// cannot claim both authors or neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreatedBy {
    /// A signed-in user, through REST.
    User(Uuid),
    /// An agent session, through MCP.
    Session(Uuid),
}

/// Everything a caller supplies to create a task.
///
/// The un-validated shape: `title`, `state` and `labels` are still raw here
/// and are validated by the model and by [`resolve_state`] under the project
/// lock, so the REST body and the MCP tool arguments can be mapped onto this
/// without either of them re-deriving a rule.
///
/// `depends_on` is a list of [`TaskRef`] rather than of UUIDs because both
/// callers accept a task's per-project number as well as its UUID
/// (`SPEC.md`, "Tasks"), and a number is only resolvable inside the project
/// lock — which is where [`create_task`] resolves it.
#[derive(Debug, Clone)]
pub struct CreateTaskInput {
    /// The title; trimmed and length-checked by the model.
    pub title: String,
    /// The description, or `None` for the empty one.
    pub description: Option<String>,
    /// The state's *name*, or `None` for the project's default queue state.
    pub state: Option<String>,
    /// 0 (critical) to 3 (low), or `None` for the default 2.
    pub priority: Option<i16>,
    /// The labels; normalised (trimmed, deduplicated, pattern-checked) by the
    /// model.
    pub labels: Vec<String>,
    /// The parent, which must be a different top-level task of this project.
    pub parent: Option<Uuid>,
    /// The prerequisites, each becoming a `blocks` edge, in this order.
    pub depends_on: Vec<TaskRef>,
    /// The task this one was discovered from, for a session's creation.
    ///
    /// Only the MCP tool supplies it, because only a session has an origin to
    /// name (`SPEC.md`, `create_task`); REST has no such input and a
    /// [`CreatedBy::User`] creation ignores whatever is here. Omitted, it is
    /// inferred from what the session holds — see
    /// [`resolve_origin`](crate::tracker::provenance::resolve_origin).
    pub discovered_from: Option<TaskRef>,
    /// Whose creation this is.
    pub created_by: CreatedBy,
}

/// Create a task inside an open mutation, with its edges and its events.
///
/// The order is the module doc's, and every step is a documented rule rather
/// than an implementation detail:
///
/// - the **state** is resolved by name through [`resolve_state`], which is 400
///   with the project's valid names when the name is not one of them
///   (`SPEC.md`, "Tasks": "400 for an unknown state"); `None` lands the task
///   in the project's default queue state (`docs/data-model.md`,
///   `task_states`). A task created directly in a terminal state is closed on
///   the spot, because `closed_at` is what "a terminal state closes the task"
///   means and nothing else would ever set it for a task that was never open.
/// - the **number** comes from the project's counter inside this transaction
///   and is never reused ([`TaskRepository::insert_task`]).
/// - the **parent** rules are the repository's, checked under this lock:
///   a parent that is not a top-level task of this project is 400.
/// - each **dependency** is proved acyclic before it is inserted (409) and
///   proved to be a task of this project by the insert (400). Repeated
///   entries — including the same task named once by number and once by UUID —
///   are inserted once rather than answered with the primary key's `dependency
///   already exists`: a caller asking twice for the edge it wants gets it.
/// - the **provenance** of a session's creation is [`resolve_origin`]'s: the
///   sole held task when nothing is named, the named one when it is held, 400
///   when several are held and none is named, 404 when the named one is not a
///   task of this project. A user's creation has none — REST has no such
///   input — and the field is ignored for it rather than refused.
/// - **`blocked`** is recomputed for the new task and its parent, and each
///   flip emits its own event ([`recompute_blocked`]). The `discovered_from`
///   edge takes no part in it: it is informational, and only `blocks` blocks.
///
/// The returned [`TaskDto`] is read after all of that, so its `blocked` and
/// `depends_on` are what the caller's 201 body should show. Any failure leaves
/// the mutation to roll back: no task, no edges, no events.
pub async fn create_task(m: &mut TrackerMutation<'_>, input: CreateTaskInput) -> Result<TaskDto> {
    let project_id = m.project_id();

    let state = match input.state.as_deref() {
        Some(name) => Some(resolve_state(m, name).await?),
        None => None,
    };

    // Under the lock and before the row: an ambiguous or unfounded origin
    // fails the creation whole.
    let origin = match input.created_by {
        CreatedBy::Session(session_id) => {
            resolve_origin(m, session_id, input.discovered_from, input.parent).await?
        }
        CreatedBy::User(_) => None,
    };

    let mut new_task = NewTask::new(project_id, &input.title)?;
    new_task.description = input.description.unwrap_or_default();
    new_task.priority = match input.priority {
        Some(priority) => Priority::try_from(priority)?,
        None => Priority::default(),
    };
    new_task.labels = Label::parse_list(&input.labels)?;
    new_task.parent_id = input.parent;
    new_task.state_id = state.as_ref().map(|state| state.id);
    match input.created_by {
        CreatedBy::User(user_id) => new_task.created_by_user_id = Some(user_id),
        CreatedBy::Session(session_id) => new_task.created_by_session_id = Some(session_id),
    }

    let repository = TaskRepository::new(m.pool());
    let inserted = repository
        .insert_task(m.conn(), project_id, &new_task)
        .await?;

    if state.map(|state| state.kind) == Some(TaskStateKind::Terminal) {
        repository
            .set_task_state_fields(
                m.conn(),
                project_id,
                inserted.id,
                &StateFields {
                    closed_at: Some(Some(Utc::now())),
                    ..StateFields::default()
                },
            )
            .await?;
    }

    let mut edges: Vec<Uuid> = Vec::with_capacity(input.depends_on.len());
    for reference in &input.depends_on {
        let prerequisite = repository
            .find_task_for_update(m.conn(), project_id, *reference)
            .await?
            .ok_or_else(|| Error::BadRequest(DEPENDENCY_SCOPE.into()))?;

        if edges.contains(&prerequisite.id) {
            continue;
        }

        check_no_cycle(m, inserted.id, prerequisite.id).await?;
        repository
            .insert_dependency(
                m.conn(),
                project_id,
                inserted.id,
                prerequisite.id,
                TaskDependencyKind::Blocks,
            )
            .await?;

        edges.push(prerequisite.id);
    }

    // The provenance edge, after the prerequisites and beside them: the same
    // pair may carry both kinds, and a `blocks` entry naming the origin does
    // not replace the record of where the task came from (`docs/data-model.md`,
    // `task_dependencies`).
    if let Some(origin) = origin {
        repository
            .insert_dependency(
                m.conn(),
                project_id,
                inserted.id,
                origin,
                TaskDependencyKind::DiscoveredFrom,
            )
            .await?;
    }

    // After the edges: the payload of `created` describes the task as it is
    // once the creation is complete, prerequisites and all.
    let created = repository
        .load_task_dto_in(m.conn(), project_id, inserted.id)
        .await?
        .ok_or(Error::NotFound)?;

    m.emit_task(TaskEventKind::Created, &created)?;
    for _ in &edges {
        m.emit_task(TaskEventKind::DependencyAdded, &created)?;
    }
    if origin.is_some() {
        m.emit_task(TaskEventKind::DependencyAdded, &created)?;
    }

    let mut affected = vec![inserted.id];
    if let Some(parent_id) = inserted.parent_id {
        affected.push(parent_id);
    }
    recompute_blocked(m, &affected).await?;

    m.touch_actor(inserted.id);
    if let CreatedBy::Session(session_id) = input.created_by {
        // Creating a task is the session having worked on it, whatever actor
        // the mutation carries (`docs/data-model.md`, `task_sessions`; ADR
        // 0030). Recorded once: `touch` deduplicates the pair `touch_actor`
        // just wrote.
        m.touch(inserted.id, session_id);
    }

    info!(
        project_id = %project_id,
        task_id = %inserted.id,
        number = inserted.number,
        dependencies = edges.len(),
        discovered_from = origin.is_some(),
        "task created",
    );

    // Read once more: `recompute_blocked` may have flipped `blocked` since the
    // payload above was taken, and the caller answers with this.
    repository
        .load_task_dto_in(m.conn(), project_id, inserted.id)
        .await?
        .ok_or(Error::NotFound)
}

/// What a `PUT` supplied, un-validated.
///
/// The sibling of [`CreateTaskInput`] for the edit surface `SPEC.md`, "Tasks"
/// gives `PUT /projects/{pid}/tasks/{id}` and the MCP `update` tool: the REST
/// body and the tool arguments both map onto this, and every rule — the
/// title, the priority, the labels, the state name, the parent — is applied
/// once, under the project lock, by [`update_task`].
///
/// **`Option<Option<T>>` is not an accident.** `parent_id` and
/// `assignee_user_id` are nullable columns a caller may want to *clear*, so
/// "the field was absent" and "the field was `null`" have to be different
/// values: `None` leaves the column alone, `Some(None)` writes NULL and
/// `Some(Some(id))` writes the id. The REST body says the same thing with
/// `#[serde(default, deserialize_with = "double_option")]`.
#[derive(Debug, Clone, Default)]
pub struct UpdateTaskInput {
    /// The new title; trimmed and length-checked by the model.
    pub title: Option<String>,
    /// The new description; the empty string is a legitimate value.
    pub description: Option<String>,
    /// 0 (critical) to 3 (low).
    pub priority: Option<i16>,
    /// The whole new label set, replacing the stored one.
    pub labels: Option<Vec<String>>,
    /// The new parent, or `Some(None)` to make the task top-level again.
    pub parent: Option<Option<Uuid>>,
    /// The new assignee, or `Some(None)` to unassign.
    pub assignee_user_id: Option<Option<Uuid>>,
    /// The state's *name*; a different one is a hand-off, the current one is a
    /// state no-op (`SPEC.md`, "Tasks").
    pub state: Option<String>,
    /// Written alongside a state change, for the callers that move a task into
    /// the human state with a reason. Without `state` it does nothing: the
    /// column is [`change_state`]'s to write, and there is no state change to
    /// attach it to.
    pub needs_human_reason: Option<String>,
}

/// What an update did.
#[derive(Debug, Clone, PartialEq)]
pub struct UpdateOutcome {
    /// The task as it stands after everything the update asked for.
    pub task: TaskDto,
    /// Whether anything actually moved — a column, the state, or both.
    ///
    /// `false` is the documented no-op: a body naming only values the task
    /// already had writes nothing, emits nothing and leaves the lease,
    /// `attempts` and `closed_at` exactly as they were (`SPEC.md`, "Tasks":
    /// "a request with no effective changes emits no task event"). The caller
    /// still answers 200 with [`UpdateOutcome::task`].
    pub changed: bool,
}

/// What an `assignee_user_id` naming nobody is told (400).
///
/// The column is a foreign key to `users`, so the refusal arrives as a foreign
/// key violation rather than as a check this could run itself: running one
/// would be a read the write then repeats, and a user deleted between the two
/// would still land here.
const UNKNOWN_ASSIGNEE: &str = "unknown assignee";

/// Apply an update to a task inside an open mutation, with its events.
///
/// `task` must be the row as it is under this mutation's lock (read with
/// `TaskRepository::find_task_for_update`). The order is `SPEC.md`, "Tasks"
/// read from the top:
///
/// ```text
/// update the columns (400: title, priority, label, parent rules, assignee)
///   → emit `updated` if a column actually moved
///   → recompute `blocked` for the old and the new parent, emitting the flips
///   → resolve the state name (400) and hand the task off, which emits
///     `state_changed` and the flips crossing the terminal line owes
/// ```
///
/// **Events, in order**: `updated`, then a re-parenting's parent flips, then
/// `state_changed`, then the flips the state change caused. The frontend only
/// refreshes its snapshot when an event arrives (ADR 0022), so the order is
/// about a readable history rather than about correctness; what it says is
/// "the fields changed, then the task moved, and here is what that moved
/// downstream". Nothing at all is emitted when nothing changed, and
/// `task_sessions` is likewise only touched then (ADR 0030).
///
/// **A state no-op is not a refusal**: assigning the state the task is already
/// in preserves the lease, `attempts` and `closed_at` and emits no state
/// event, while the other supplied fields still apply ([`change_state`]).
///
/// The returned [`TaskDto`] is read after everything, so its `state`,
/// `blocked` and `closed_at` are what the caller's 200 body should show.
pub async fn update_task(
    m: &mut TrackerMutation<'_>,
    task: &Task,
    input: UpdateTaskInput,
) -> Result<UpdateOutcome> {
    let project_id = m.project_id();

    let update = TaskUpdate {
        title: input.title.as_deref().map(TaskTitle::parse).transpose()?,
        description: input.description,
        priority: input.priority.map(Priority::try_from).transpose()?,
        labels: input.labels.as_deref().map(Label::parse_list).transpose()?,
        assignee_user_id: input.assignee_user_id,
        parent_id: input.parent,
    };

    // (1) The columns. The repository decides what "changed" means — against
    // the row under the lock, not against the number of rows the statement
    // touched — and runs the one-level parent rules when the parent moves.
    let updated = TaskRepository::new(m.pool())
        .update_task(m.conn(), project_id, task.id, &update)
        .await
        .map_err(unknown_assignee)?;

    // (2) `updated`, before anything a re-parenting or a hand-off sets off.
    if updated.is_some() {
        let dto = load(m, task.id).await?;
        m.emit_task(TaskEventKind::Updated, &dto)?;
    }

    // (3) A child that moved changed what blocks each of the two parents: the
    // old one lost an open child, the new one may have gained one. The task's
    // own flag cannot move — a parent never blocked its children.
    if let Some(moved) = updated
        .as_ref()
        .filter(|row| row.parent_id != task.parent_id)
    {
        let affected: Vec<Uuid> = [task.parent_id, moved.parent_id]
            .into_iter()
            .flatten()
            .collect();
        recompute_blocked(m, &affected).await?;
    }

    // (4) The state, as a hand-off. A user may set any state and is not bound
    // by leases (`SPEC.md`, "Tasks"), so there is nothing to check here that
    // [`resolve_state`] does not.
    let state_changed = match input.state.as_deref() {
        Some(name) => {
            let target = resolve_state(m, name).await?;
            // The row the move is applied to is the one the column update left
            // behind, so a re-parenting in the same request is already stored
            // when the parent-closure logic reads it.
            let current = updated.as_ref().unwrap_or(task);
            change_state(
                m,
                current,
                &target,
                StateChangeOptions {
                    event: StateEventKind::StateChanged,
                    needs_human_reason: input.needs_human_reason,
                },
            )
            .await?
            .changed
        }
        None => false,
    };

    let changed = updated.is_some() || state_changed;
    if changed {
        m.touch_actor(task.id);

        info!(
            project_id = %project_id,
            task_id = %task.id,
            fields = updated.is_some(),
            state = state_changed,
            "task updated",
        );
    }

    // Read once more: the state change and the recomputations may have moved
    // `blocked`, `closed_at` and the lease since the event payloads were
    // taken, and the caller answers with this.
    Ok(UpdateOutcome {
        task: load(m, task.id).await?,
        changed,
    })
}

/// Delete a task inside an open mutation, with everything the deletion owes.
///
/// `task` must be the row as it is under this mutation's lock. The order is
/// `docs/data-model.md`, `tasks`, which exists because the cascades destroy
/// the evidence:
///
/// ```text
/// capture the incoming edges, the parent and the children
///   → delete the row (edges, comments, hand-offs and links cascade;
///     the children's `parent_id` becomes NULL)
///   → emit `dependency_removed` per captured `(dependant, kind)`
///   → recompute `blocked` for the dependants and the parent, emitting flips
///   → emit `deleted` last, carrying the original UUID
/// ```
///
/// **The children get no event.** They lose a parent, not a prerequisite, and
/// a parent never blocked its children, so their `blocked` flag cannot have
/// moved; they simply become top-level tasks. **The parent is not closed** by
/// losing its last open child either: closure is triggered by a state change
/// (`ARCHITECTURE.md`, "Task tracker" → "Parents"), so what the parent gets
/// here is an `unblocked` event and a person to decide the rest.
///
/// **`deleted` keeps the task's identity**: the id lives on in `task_events`,
/// which has no foreign key to `tasks` for exactly this reason, and the
/// earlier events about the task keep carrying it (ADR 0022). Its payload is
/// the actor and nothing else — there is no task left to describe — and
/// nothing links the deleted task to a session: the `task_sessions` rows went
/// with the row, and a link written now would point at nothing.
///
/// **No git lock is taken here.** Hand-off ref cleanup is added by the Code
/// hand-offs epic before this function is called, outside the project lock and
/// therefore outside this mutation (`ARCHITECTURE.md`, "Task tracker": any git
/// lock before any database lock).
pub async fn delete_task(m: &mut TrackerMutation<'_>, task: &Task) -> Result<()> {
    let project_id = m.project_id();

    let capture = capture_before_delete(m, task.id).await?;

    if !TaskRepository::new(m.pool())
        .delete_task(m.conn(), project_id, task.id)
        .await?
    {
        return Err(Error::NotFound);
    }

    // One event per edge that disappeared, kind and all, carrying the
    // dependant as it is now that the edge is gone.
    for (dependant_id, kind) in capture.dependants.clone() {
        let Some(dependant) = TaskRepository::new(m.pool())
            .load_task_dto_in(m.conn(), project_id, dependant_id)
            .await?
        else {
            continue;
        };

        debug!(
            project_id = %project_id,
            task_id = %dependant_id,
            depends_on_task_id = %task.id,
            kind = ?kind,
            "dependency removed with its prerequisite",
        );
        m.emit_task(TaskEventKind::DependencyRemoved, &dependant)?;
    }

    recompute_blocked(m, &capture.to_recompute()).await?;

    m.emit_deleted(task.id)?;

    info!(
        project_id = %project_id,
        task_id = %task.id,
        number = task.number,
        dependants = capture.dependants.len(),
        children = capture.children.len(),
        "task deleted",
    );

    Ok(())
}

/// This mutation's view of a task of this project, which must be there.
///
/// The row is under this mutation's own lock, so a miss is a broken invariant
/// rather than something the caller did; [`Error::NotFound`] is what it maps
/// to all the same, as it does everywhere else in the tracker.
async fn load(m: &mut TrackerMutation<'_>, task_id: Uuid) -> Result<TaskDto> {
    let project_id = m.project_id();

    TaskRepository::new(m.pool())
        .load_task_dto_in(m.conn(), project_id, task_id)
        .await?
        .ok_or(Error::NotFound)
}

/// Turn the assignee foreign key's complaint into the documented 400.
///
/// Every other failure passes through untouched, including the parent rules'
/// own [`Error::BadRequest`]s, which the repository already phrases.
fn unknown_assignee(error: Error) -> Error {
    if let Error::Database(ref db_error) = error
        && let Some("tasks_assignee_user_id_fkey") = foreign_key_violation(db_error)
    {
        return Error::BadRequest(UNKNOWN_ASSIGNEE.into());
    }

    error
}
