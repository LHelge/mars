//! `create_task`: file discovered work (`SPEC.md`, "MCP tool contracts" →
//! `create_task`).
//!
//! "Create a task when you discover work outside what you hold": a follow-up,
//! a bug or a sub-task, in the state the agent names or the project's default
//! one, with `blocks` prerequisites, a one-level parent and — the part only an
//! agent has — a record of which task the discovery came out of.
//!
//! **Nothing about a task is decided here.** The title, the priority, the
//! labels, the state name, the parent rules, the number, the edges, the
//! `blocked` recomputation, the events and the `task_sessions` link are
//! [`tracker::tasks::create_task`](crate::tracker::tasks::create_task)'s,
//! shared with `POST /projects/{pid}/tasks`, and the provenance rules are
//! [`resolve_origin`](crate::tracker::provenance::resolve_origin)'s, which
//! that function calls for a session's creation. What this handler owes is the
//! transport's three jobs:
//!
//! 1. the rejections that need no database — a title that is not one, a
//!    priority outside 0–3, a label that is not one, a `task` argument that is
//!    not a reference — refused *before* the project lock is taken, so a
//!    malformed call never queues behind another mutation;
//! 2. `parent`, which an agent may give as a per-project number while the
//!    tracker's input takes a UUID, resolved under the lock ([`resolve_parent`]);
//! 3. the creator: [`CreatedBy::Session`] from the [`SessionContext`] the
//!    bearer middleware resolved, never from an argument, so
//!    `created_by_session_id` is the session that actually called and
//!    `created_by_user_id` stays NULL.
//!
//! **Every failure creates nothing.** The whole creation — provenance
//! included — runs inside one [`TrackerMutation`], and `finish` rolls it back
//! on any rejection: no task row, no edges, no events, and `next_task_number`
//! where it was (`SPEC.md`: "any validation failure creates neither a task nor
//! edges").

use uuid::Uuid;

use crate::mcp::tools::common::{begin_mutation, finish};
use crate::mcp::tools::{CreateTaskInput, TaskArg, TaskOutput, validate_priority};
use crate::mcp::{McpError, McpResult, SessionContext};
use crate::models::{Label, TaskRef, TaskTitle};
use crate::prelude::*;
use crate::repositories::TaskRepository;
// The tracker's input shape shares its name with the tool's; the alias keeps
// both readable in one function.
use crate::tracker::tasks::{CreateTaskInput as NewTaskInput, CreatedBy, create_task};
use crate::tracker::{TaskDto, TrackerMutation};

/// What a `parent` naming no task of this project is told.
///
/// The argument is named, unlike the bare `task not found` a `task` argument
/// gets: a creation carries up to three task references, and "task not found"
/// alone would leave the agent guessing which of them it got wrong. A
/// `depends_on` entry is the tracker's own refusal and keeps the tracker's
/// wording, so REST and MCP answer that one alike.
pub const PARENT_NOT_FOUND: &str = "parent task not found";

/// The task references an input carries, parsed.
///
/// Kept apart from the rest of the input because these are the only fields
/// whose validity is decidable without the database: parsing them early is
/// what lets a malformed reference be refused before the lock, while
/// *resolving* them still happens inside it.
struct References {
    /// The parent, still a reference; resolved to a UUID under the lock.
    parent: Option<TaskRef>,
    /// The prerequisites, in the caller's order; the tracker resolves them.
    depends_on: Vec<TaskRef>,
    /// The named origin, if any; the provenance rules resolve it.
    discovered_from: Option<TaskRef>,
}

/// `{ title, description?, state?, priority?, labels?, parent?, depends_on?,
/// discovered_from? }` → `{ task: Task }`.
pub async fn handle(
    state: &AppState,
    ctx: &SessionContext,
    input: CreateTaskInput,
) -> McpResult<TaskOutput> {
    let references = validate(&input)?;

    let mut mutation = begin_mutation(state, ctx).await?;
    let created = create(&mut mutation, ctx, input, references).await;
    let task = finish(state, mutation, created).await?;

    Ok(TaskOutput { task })
}

/// The rejections that need neither the lock nor the database.
///
/// The title, the priority and the labels are checked through the same model
/// functions the creation itself uses ([`TaskTitle::parse`],
/// [`validate_priority`], [`Label::parse_list`]), so the message an agent
/// reads here is the message the tracker would have produced a moment later —
/// this only moves it in front of the lock. The parsed values are thrown away
/// rather than passed on: the tracker's input is the raw shape, and having one
/// place that turns it into a row is worth re-parsing three small fields.
///
/// There is deliberately no separate empty-title check: `title must be 1-200
/// characters` covers the blank title too, and a second wording of the same
/// rule would be one an agent could get from MCP and never from REST.
fn validate(input: &CreateTaskInput) -> McpResult<References> {
    TaskTitle::parse(&input.title).map_err(Error::from)?;

    if let Some(priority) = input.priority {
        validate_priority(priority)?;
    }

    if let Some(labels) = &input.labels {
        Label::parse_list(labels).map_err(Error::from)?;
    }

    Ok(References {
        parent: input.parent.as_ref().map(TaskArg::parse).transpose()?,
        depends_on: input
            .depends_on
            .iter()
            .flatten()
            .map(TaskArg::parse)
            .collect::<McpResult<Vec<_>>>()?,
        discovered_from: input
            .discovered_from
            .as_ref()
            .map(TaskArg::parse)
            .transpose()?,
    })
}

/// The creation itself, inside the open mutation.
///
/// One call to the tracker, with the parent turned into the UUID its input
/// takes. The provenance is not resolved here although the rules are
/// MCP-specific: the tracker runs [`resolve_origin`] for a
/// [`CreatedBy::Session`] creation, under this same lock and before the insert,
/// and a second copy of those rules in this module is exactly the thing that
/// could disagree with the stored edges.
///
/// [`resolve_origin`]: crate::tracker::provenance::resolve_origin
async fn create(
    m: &mut TrackerMutation<'_>,
    ctx: &SessionContext,
    input: CreateTaskInput,
    references: References,
) -> McpResult<TaskDto> {
    let parent = match references.parent {
        Some(reference) => Some(resolve_parent(m, reference).await?),
        None => None,
    };

    let created = create_task(
        m,
        NewTaskInput {
            title: input.title,
            description: input.description,
            state: input.state,
            priority: input.priority,
            labels: input.labels.unwrap_or_default(),
            parent,
            depends_on: references.depends_on,
            discovered_from: references.discovered_from,
            // From the bearer token's session, never from an argument
            // (`ARCHITECTURE.md`, "MCP design" → Authentication).
            created_by: CreatedBy::Session(ctx.session_id),
        },
    )
    .await?;

    Ok(created)
}

/// The UUID of the task a `parent` argument names, under this mutation's lock.
///
/// Scoped to the calling session's project in the `WHERE` clause, so a parent
/// in another project is indistinguishable from one that does not exist. The
/// row is read `FOR UPDATE` like every other task this mutation touches: the
/// parent the one-level rule is checked against is then the row the insert
/// hangs the child off.
///
/// Whether it *may* be a parent — top-level, of this project, not the task
/// itself — is the repository's rule and is left to it.
async fn resolve_parent(m: &mut TrackerMutation<'_>, reference: TaskRef) -> McpResult<Uuid> {
    let project_id = m.project_id();

    let parent = TaskRepository::new(m.pool())
        .find_task_for_update(m.conn(), project_id, reference)
        .await?
        .ok_or_else(|| McpError::not_found(PARENT_NOT_FOUND))?;

    Ok(parent.id)
}
