//! Creating a task: the one code path `POST /projects/{pid}/tasks` and the
//! MCP `create_task` tool both go through (`SPEC.md`, "Tasks";
//! `ARCHITECTURE.md`, "Task tracker" → "Parents" and "Blocked is stored").
//!
//! Everything a creation owes happens inside one [`TrackerMutation`], so the
//! row, its dependency edges, the `blocked` flags they flip and every event
//! that describes them commit together or not at all (ADR 0021, 0028):
//!
//! ```text
//! resolve the state (400) → insert the row with its allocated number
//!   → per dependency: cycle check (409) → insert the `blocks` edge (400)
//!   → emit `created` (the task *after* the edges)
//!   → emit one `dependency_added` per edge, in `depends_on` order
//!   → recompute `blocked` for the task and its parent, emitting the flips
//! ```
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

use chrono::Utc;
use uuid::Uuid;

use crate::events::TaskEventKind;
use crate::models::{Label, NewTask, Priority, TaskDependencyKind, TaskRef, TaskStateKind};
use crate::prelude::*;
use crate::repositories::TaskRepository;
use crate::repositories::tasks::StateFields;
use crate::tracker::graph::{check_no_cycle, recompute_blocked};
use crate::tracker::state::resolve_state;
use crate::tracker::{TaskDto, TrackerMutation};

/// What a `depends_on` entry naming nothing in this project is told (400).
///
/// The same message [`TaskRepository::insert_dependency`] gives an id from
/// another project, and for the same reason: nothing leaks about whether the
/// task exists elsewhere.
const DEPENDENCY_SCOPE: &str = "dependency must reference tasks of the same project";

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
/// - **`blocked`** is recomputed for the new task and its parent, and each
///   flip emits its own event ([`recompute_blocked`]).
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

    let mut affected = vec![inserted.id];
    if let Some(parent_id) = inserted.parent_id {
        affected.push(parent_id);
    }
    recompute_blocked(m, &affected).await?;

    m.touch_actor(inserted.id);

    info!(
        project_id = %project_id,
        task_id = %inserted.id,
        number = inserted.number,
        dependencies = edges.len(),
        "task created",
    );

    // Read once more: `recompute_blocked` may have flipped `blocked` since the
    // payload above was taken, and the caller answers with this.
    repository
        .load_task_dto_in(m.conn(), project_id, inserted.id)
        .await?
        .ok_or(Error::NotFound)
}
