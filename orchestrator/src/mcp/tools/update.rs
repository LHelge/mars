//! `update`: the agent's hand-off tool (`SPEC.md`, "MCP tool contracts" →
//! `update`; "Code hand-offs and review").
//!
//! One tool, two paths, and which one a call takes is decided by a single
//! field:
//!
//! - **without `handoff`** it is the ordinary tracker update the REST `PUT`
//!   makes — [`begin_mutation`], the task under the lock, the authority check,
//!   [`update_task`], commit — with the project row lock taken first and the
//!   task row lock second (ADR 0021);
//! - **with `handoff`** it is a code publication, and the order is inverted by
//!   the documents: the project **git** lock first, the sync and the
//!   `refs/handoffs/<id>` pin inside it, and only then the tracker
//!   transaction (`ARCHITECTURE.md`, "Task tracker" → "Code hand-offs": any
//!   git lock before any database lock). This module never takes the git lock
//!   itself and never holds a mutation across git work: it hands the whole
//!   ordering to [`HandoffService::update_with_handoff`], which is the one
//!   code path the REST `PUT` uses for the same body.
//!
//! **Both paths run the same tracker code.** The fields, the state move, the
//! re-parenting and the `blocks` edges are one [`UpdateTaskInput`] either way,
//! applied by [`update_task`] — inside this module's mutation on the first
//! path and inside the service's on the second — so a hand-off cannot apply a
//! field change differently from an ordinary update.
//!
//! **Nothing is resolved before the lock that the lock is supposed to
//! decide.** The `task`, the `parent` and each `add_depends_on` or
//! `remove_depends_on` entry are *parsed* here, because a malformed reference
//! is the caller's mistake and needs no database at all, and are *resolved*
//! inside whichever transaction is about to act on them: a per-project number
//! read outside the lock can name a different task by the time the change
//! lands.
//!
//! **The authority rule is the transport's.** "The caller must hold the lease,
//! except for `title`, `description`, `labels`, `add_depends_on` and
//! `remove_depends_on` on tasks the caller created that nobody holds." The
//! tracker has no opinion about who may edit a task — a user is not
//! lease-bound at all — so [`authorise`] is where an agent's five-field
//! exception lives, and its refusal is the tracker's own
//! [`NOT_HELD_BY_SESSION`], the message `claim`, `release` and `needs_human`
//! already answer with.

use crate::mcp::tools::common::{begin_mutation, finish, require_holder, resolve_ref_for_mutation};
use crate::mcp::tools::{TaskArg, TaskOutput, UpdateInput, validate_handoff, validate_priority};
use crate::mcp::{McpError, McpResult, SessionContext};
use crate::models::{HandoffCaller, HandoffInput, Task, TaskError, TaskRef};
use crate::prelude::*;
use crate::tracker::graph::CYCLE;
use crate::tracker::handoffs::HandoffService;
use crate::tracker::{TaskDto, TrackerMutation, UpdateTaskInput, update_task};

/// What an `update` that names nothing at all is told.
///
/// Every field is optional, so an empty call is well-formed JSON that asks for
/// nothing; answering it with the task as it is would hide the mistake, and
/// writing an `updated` event for it would be a lie.
const NO_FIELDS: &str = "update requires at least one field";

/// `{ task, ... }` → `{ task: Task }`.
pub async fn handle(
    state: &AppState,
    ctx: &SessionContext,
    input: UpdateInput,
) -> McpResult<TaskOutput> {
    let parsed = Parsed::from_input(ctx, input)?;

    let task = match parsed.handoff {
        Some(handoff) => published(state, ctx, parsed.task, parsed.update, handoff).await?,
        None => plain(state, ctx, parsed.task, parsed.update).await?,
    };

    Ok(TaskOutput { task })
}

/// An `update` whose every lock-free rule has passed.
///
/// Built before either path takes anything, so a call that was never going to
/// work queues behind no other mutation, syncs no branch and pins no ref
/// (ADR 0030).
struct Parsed {
    /// The task to change, as a reference the transaction will resolve.
    task: TaskRef,
    /// The field, state and dependency changes, for whichever path applies
    /// them.
    update: UpdateTaskInput,
    /// The publication, if this update is one.
    handoff: Option<HandoffInput>,
}

impl Parsed {
    /// Every rule that needs neither the database nor the git repository.
    ///
    /// In order: something to do at all, the priority range, the hand-off
    /// input rules — which is where a `source_session_id` an MCP caller must
    /// not supply is refused, before any git work — and the rule that a
    /// hand-off travels with a move to another state. The state *name* is not
    /// resolved here: it is looked up under the project lock, so that a
    /// concurrent rename either precedes this update or follows it.
    fn from_input(ctx: &SessionContext, input: UpdateInput) -> McpResult<Self> {
        if !names_a_field(&input) {
            return Err(McpError::invalid_argument(NO_FIELDS));
        }

        if let Some(priority) = input.priority {
            validate_priority(priority)?;
        }

        if let Some(handoff) = &input.handoff {
            // The messages are the REST ones, which is what the contract asks
            // for: "the exact 400 and 409 messages are returned here".
            validate_handoff(handoff, ctx.session_id)?;

            if input.state.is_none() {
                return Err(McpError::from(Error::from(
                    TaskError::HandoffRequiresStateChange,
                )));
            }
        }

        let parent = match input.parent {
            Some(Some(arg)) => Some(Some(arg.parse()?)),
            Some(None) => Some(None),
            None => None,
        };

        Ok(Parsed {
            task: input.task.parse()?,
            update: UpdateTaskInput {
                title: input.title,
                description: input.description,
                priority: input.priority,
                labels: input.labels,
                parent,
                // Neither is an agent's to set: an assignee is a person's
                // decision, and `needs_human_reason` belongs to `needs_human`.
                assignee_user_id: None,
                state: input.state,
                needs_human_reason: None,
                add_depends_on: parse_all(input.add_depends_on)?,
                remove_depends_on: parse_all(input.remove_depends_on)?,
            },
            handoff: input.handoff,
        })
    }
}

/// Whether the call asks for anything at all.
fn names_a_field(input: &UpdateInput) -> bool {
    input.state.is_some()
        || input.title.is_some()
        || input.description.is_some()
        || input.priority.is_some()
        || input.labels.is_some()
        || input.parent.is_some()
        || input.add_depends_on.is_some()
        || input.remove_depends_on.is_some()
        || input.handoff.is_some()
}

/// A list of task arguments, each parsed, in the caller's order.
fn parse_all(args: Option<Vec<TaskArg>>) -> McpResult<Vec<TaskRef>> {
    args.unwrap_or_default()
        .iter()
        .map(|arg| arg.parse())
        .collect()
}

/// The path without a hand-off: one mutation, the project row locked, the task
/// row locked inside it.
async fn plain(
    state: &AppState,
    ctx: &SessionContext,
    task: TaskRef,
    update: UpdateTaskInput,
) -> McpResult<TaskDto> {
    let mut mutation = begin_mutation(state, ctx).await?;
    let outcome = apply(&mut mutation, ctx, task, update).await;

    finish(state, mutation, outcome).await
}

/// The change itself, inside the open mutation.
///
/// The `task` reference is resolved here rather than by the caller for the
/// reason `resolve_task_for_mutation` gives, and the authority check reads the
/// row that comes back: the lease holder and the creator are both columns, and
/// deciding against anything but the locked row would be deciding against a
/// task that may already have moved on.
async fn apply(
    m: &mut TrackerMutation<'_>,
    ctx: &SessionContext,
    task: TaskRef,
    update: UpdateTaskInput,
) -> McpResult<TaskDto> {
    let task = resolve_ref_for_mutation(m, ctx, task).await?;
    authorise(&task, ctx, &update)?;

    let outcome = update_task(m, &task, update)
        .await
        .map_err(cycle_is_invalid)?;

    Ok(outcome.task)
}

/// The path with a hand-off: the service owns the git lock, the ordering and
/// every rule, and this maps its answer.
///
/// No mutation is opened here and no repository is read: doing either would
/// put a database lock in front of the git lock the service is about to take
/// (`ARCHITECTURE.md`, "Task tracker" → "Code hand-offs"). The caller is a
/// [`HandoffCaller::Session`], which is what makes the revision's source
/// session the calling one and what makes the service enforce the lease —
/// publishing needs no git-tool permission, because it only syncs and retains
/// committed work inside the project.
async fn published(
    state: &AppState,
    ctx: &SessionContext,
    task: TaskRef,
    update: UpdateTaskInput,
    handoff: HandoffInput,
) -> McpResult<TaskDto> {
    let published = HandoffService::from_state(state)
        .update_with_handoff(
            ctx.project_id,
            task,
            update,
            handoff,
            HandoffCaller::Session {
                session_id: ctx.session_id,
            },
        )
        .await
        .map_err(cycle_is_invalid)?;

    Ok(published)
}

/// Who may change what, on top of the tracker's own rules.
///
/// The holder may change anything. A session that *created* a task nobody
/// holds may change the five fields the contract names and nothing else — not
/// the state, not the priority, not the parent, and not by handing off — so
/// that an agent can correct and cross-link the work it has discovered without
/// taking a lease on it. Everything else is the refusal the other lease tools
/// give, in the same words.
fn authorise(task: &Task, ctx: &SessionContext, update: &UpdateTaskInput) -> McpResult<()> {
    let creator_of_an_unheld_task = task.created_by_session_id == Some(ctx.session_id)
        && task.lease_holder_session_id.is_none();

    if creator_of_an_unheld_task && creator_editable_only(update) {
        return Ok(());
    }

    require_holder(task, ctx)
}

/// Whether the update stays inside `title`, `description`, `labels`,
/// `add_depends_on` and `remove_depends_on`.
///
/// A hand-off cannot get here: [`published`] never calls this, and the service
/// applies the lease rule of its own.
fn creator_editable_only(update: &UpdateTaskInput) -> bool {
    update.state.is_none() && update.priority.is_none() && update.parent.is_none()
}

/// The one refusal whose code differs between REST and MCP.
///
/// A `blocks` edge that would close a cycle is 409 over HTTP (`SPEC.md`,
/// "Tasks") and `invalid_argument` here: "`add_depends_on` creates `blocks`
/// dependencies; a cycle returns `invalid_argument`". The graph check is the
/// same one either way, so the code is changed at this boundary rather than by
/// a second check that could disagree with it.
fn cycle_is_invalid(error: Error) -> McpError {
    match &error {
        Error::Conflict(message) if message == CYCLE => McpError::invalid_argument(CYCLE),
        _ => McpError::from(error),
    }
}
