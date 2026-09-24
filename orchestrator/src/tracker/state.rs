//! Moving a task between states: the one function every caller goes through.
//!
//! `ARCHITECTURE.md`, "Task tracker" → "The lease is the worker", "Parents",
//! "Blocked is stored" and "Attempts and escalation"; `SPEC.md`, "Tasks" and
//! "TaskEvent"; `docs/data-model.md`, `tasks`.
//!
//! A user dragging a card, an agent calling `update`, the reaper running out
//! of attempts and the orchestrator closing a parent all mean the same thing
//! by "move this task", and they all mean it *inside* somebody's tracker
//! mutation. [`change_state`] is that meaning, once:
//!
//! - **a different state is a hand-off.** It clears the lease — whoever held
//!   it, because a user is not bound by one — resets `attempts` to zero, sets
//!   `closed_at` when the target is terminal and clears it when it is not, and
//!   leaves `current_handoff_id` alone: a plain move neither publishes code
//!   nor withdraws it (`docs/data-model.md`, `tasks`). A move *out of* the
//!   project's human state also resets `rounds` to zero, which is the fresh
//!   allowance a person handing an escalated task back gives it; a revision
//!   publication then adds its own round on top ([`RoundsWrite`];
//!   `ARCHITECTURE.md`, "Task tracker" → "Rounds"; ADR 0046).
//! - **the current state is a no-op.** Nothing is written and nothing is
//!   emitted; the lease, `attempts` and `closed_at` survive. The caller's
//!   *other* field changes still apply — that is its own `update`, not this
//!   function's business — which is why this answers with
//!   [`StateChangeResult::changed`] rather than an error.
//! - **crossing the terminal line moves the graph.** Entering or leaving a
//!   terminal state changes what this task blocks, so its dependants and its
//!   parent are recomputed in the same transaction and each flip emits
//!   `blocked` or `unblocked` (`tracker::graph`).
//! - **the last open child closes its parent.** Still in the same
//!   transaction, with actor `system`, into the project's terminal state with
//!   the lowest position. One level only, because nesting is one level:
//!   a parent has no parent, so nothing recurses past it.
//!
//! The event order inside one call is the order the board replays: the task's
//! own `state_changed` (or `escalated`), then its dependants' and parent's
//! `blocked`/`unblocked`, then the parent's system `state_changed`, then the
//! parent's dependants' flips. A caller that also writes a comment — a
//! release with a reason, `needs_human` — emits `commented` *before* calling
//! this, so the comment precedes the state event in the stream.
//!
//! What is deliberately not here: publication and email. The Code hand-offs
//! epic wraps this function to insert the hand-off row, and the escalation
//! paths call `TrackerMutation::record_escalation` after it returns, because
//! email has no place inside the project lock.

use chrono::Utc;
use uuid::Uuid;

use crate::events::{TaskActor, TaskEventKind};
use crate::models::{Task, TaskRef, TaskState, TaskStateKind};
use crate::prelude::*;
use crate::repositories::TaskRepository;
use crate::repositories::tasks::StateFields;
use crate::tracker::graph::{affected_by_state_change, recompute_blocked};
use crate::tracker::{TaskDto, TrackerMutation};

/// Which event a move announces itself with.
///
/// The move is identical either way — the same columns, the same recompute,
/// the same parent closure — and only the row in `task_events` differs, so
/// this is a choice the caller makes rather than a second code path
/// (`SPEC.md`, "TaskEvent"). A user dragging a card into `needs_human` over
/// REST is a `state_changed`; only the escalation paths, which also owe an
/// email, use [`StateEventKind::Escalated`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum StateEventKind {
    /// An ordinary hand-off: `state_changed` with `from` and `to`.
    #[default]
    StateChanged,
    /// A move into the human state the orchestrator or an agent asked for:
    /// `escalated`, with the reason alongside `from` and `to`.
    Escalated {
        /// Why a human is needed; carried in the event payload.
        reason: String,
    },
}

impl StateEventKind {
    /// The `task_events.kind` this choice writes.
    fn kind(&self) -> TaskEventKind {
        match self {
            Self::StateChanged => TaskEventKind::StateChanged,
            Self::Escalated { .. } => TaskEventKind::Escalated,
        }
    }

    /// The payload's `reason`, which only an escalation carries.
    fn reason(&self) -> Option<&str> {
        match self {
            Self::StateChanged => None,
            Self::Escalated { reason } => Some(reason.as_str()),
        }
    }
}

/// The two things a caller can vary about a state change.
///
/// Everything else — the lease, `attempts`, `closed_at`, the recomputation,
/// the parent — follows from the target state and is not negotiable.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StateChangeOptions {
    /// `state_changed` or `escalated`.
    pub event: StateEventKind,
    /// `Some` writes `needs_human_reason` (including `Some(String::new())`
    /// for an empty one); `None` leaves the column exactly as it is, which is
    /// what an ordinary move into or out of any state does.
    pub needs_human_reason: Option<String>,
}

/// How a move writes `tasks.rounds` (`ARCHITECTURE.md`, "Task tracker" →
/// "Rounds"; ADR 0046).
///
/// Crate-private and beside [`StateChangeOptions`] rather than in it, because
/// only the hand-off path publishes a revision and nothing outside the crate
/// may claim to: a round is counted by publishing code, never by asking for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum RoundsWrite {
    /// Every ordinary move: reset to 0 when the task leaves the human state,
    /// untouched otherwise.
    #[default]
    Rule,
    /// A revision publication: one round more than the rule leaves, so a
    /// revision out of the human state is the first round of a fresh allowance.
    Revision,
}

impl RoundsWrite {
    /// The value to write, or `None` to leave the column alone.
    fn value(self, task: &Task, from: &TaskState) -> Option<i16> {
        let leaving_human = from.kind == TaskStateKind::Human;
        match self {
            Self::Rule => leaving_human.then_some(0),
            Self::Revision => {
                let base = if leaving_human { 0 } else { task.rounds };
                Some(base.saturating_add(1))
            }
        }
    }
}

/// What a state change did.
#[derive(Debug, Clone, PartialEq)]
pub struct StateChangeResult {
    /// `false` when the task was already in the target state: nothing was
    /// written and nothing emitted.
    pub changed: bool,
    /// The task as it stands now — moved, or untouched for a no-op — ready to
    /// be answered with or handed to the caller's own event.
    pub task: TaskDto,
}

/// Move `task` into `target`, with everything that implies.
///
/// `task` must be the row as it is under this mutation's lock (read with
/// `TaskRepository::find_task_for_update`), and `target` a state of the same
/// project; a state from another project is [`Error::BadRequest`] from the
/// write underneath. The caller has already decided that the move is allowed —
/// whether a lease is required, whether a hand-off is owed — because those
/// rules differ per transport and this one does not.
pub async fn change_state(
    m: &mut TrackerMutation<'_>,
    task: &Task,
    target: &TaskState,
    opts: StateChangeOptions,
) -> Result<StateChangeResult> {
    change_state_with(m, task, target, opts, RoundsWrite::Rule).await
}

/// [`change_state`], with the hand-off path's say over `rounds`.
///
/// The one addition is [`RoundsWrite`]: [`RoundsWrite::Revision`] is what a
/// revision publication passes, and it is written in the same statement as the
/// move so that the `state_changed` (or `escalated`) payload already carries
/// the new count.
pub(crate) async fn change_state_with(
    m: &mut TrackerMutation<'_>,
    task: &Task,
    target: &TaskState,
    opts: StateChangeOptions,
    rounds: RoundsWrite,
) -> Result<StateChangeResult> {
    let repository = TaskRepository::new(m.pool());
    let project_id = m.project_id();

    // The no-op: the caller assigned the state the task is already in. Not an
    // error, and not a write — the lease, `attempts` and `closed_at` are
    // exactly as claimable and as closed as they were a moment ago.
    if task.state_id == target.id {
        let dto = repository
            .load_task_dto_in(m.conn(), project_id, task.id)
            .await?
            .ok_or(Error::NotFound)?;

        return Ok(StateChangeResult {
            changed: false,
            task: dto,
        });
    }

    let from = state_of(m, task.state_id).await?;
    let result = apply(m, task, &from, target, &opts, rounds).await?;

    // Only a move *into* terminal can close a parent. Terminal → terminal
    // leaves the parent where it already is: closed with its last child, or
    // deliberately reopened by a user, and never reopened or reclosed by us.
    if target.kind == TaskStateKind::Terminal && from.kind != TaskStateKind::Terminal {
        close_parent_if_last_child(m, task).await?;
    }

    Ok(result)
}

/// The state with this id in this project.
///
/// Read through the mutation's own connection. The row cannot change
/// underneath this call either way — `task_states` is only ever written by a
/// tracker mutation and this one holds the project lock — but a pool read here
/// would make the mutation hold two pooled connections at once, so enough
/// concurrent mutations on distinct projects would each hold one and wait for
/// a second (`docs/data-model.md`, "Tracker mutation transactions").
async fn state_of(m: &mut TrackerMutation<'_>, state_id: Uuid) -> Result<TaskState> {
    let project_id = m.project_id();

    TaskRepository::new(m.pool())
        .find_state_in(m.conn(), project_id, state_id)
        .await?
        .ok_or_else(|| {
            error!(
                project_id = %m.project_id(),
                state_id = %state_id,
                "task references a state its project does not have",
            );
            Error::Internal("task state is missing".into())
        })
}

/// The write, the event and the recomputation: one task's move, without the
/// parent.
///
/// Split out so that closing a parent is a second call to *this* and not a
/// recursive call to [`change_state`] — the recursion would be bounded at one
/// level by the nesting rule, and saying so in the type is better than
/// trusting it.
async fn apply(
    m: &mut TrackerMutation<'_>,
    task: &Task,
    from: &TaskState,
    target: &TaskState,
    opts: &StateChangeOptions,
    rounds: RoundsWrite,
) -> Result<StateChangeResult> {
    let repository = TaskRepository::new(m.pool());
    let project_id = m.project_id();

    let terminal = target.kind == TaskStateKind::Terminal;
    let fields = StateFields {
        state_id: Some(target.id),
        // Whoever held it, the hand-off ends their hold.
        lease: Some(None),
        // The attempt counter counts claims *in one state*.
        attempts: Some(0),
        // The round counter counts revisions since the human state.
        rounds: rounds.value(task, from),
        // Entering a terminal state always stamps it anew, including from
        // another terminal state: `done` → `cancelled` is a closure of its
        // own. Leaving one clears it, which is what reopening means.
        closed_at: Some(terminal.then(Utc::now)),
        needs_human_reason: opts.needs_human_reason.clone().map(Some),
        // A plain move neither publishes code nor withdraws it, and the flag
        // is `tracker::graph`'s to write.
        current_handoff_id: None,
        blocked: None,
    };

    repository
        .set_task_state_fields(m.conn(), project_id, task.id, &fields)
        .await?;
    m.touch_actor(task.id);

    let dto = repository
        .load_task_dto_in(m.conn(), project_id, task.id)
        .await?
        .ok_or(Error::NotFound)?;

    m.emit_state(
        opts.event.kind(),
        &dto,
        &from.name,
        &target.name,
        opts.event.reason(),
    )?;

    // What this task blocks only changed if it crossed the terminal line.
    if terminal != (from.kind == TaskStateKind::Terminal) {
        let affected = affected_by_state_change(m, task.id).await?;
        recompute_blocked(m, &affected).await?;
    }

    Ok(StateChangeResult {
        changed: true,
        task: dto,
    })
}

/// Close the parent when this closure was its last open child.
///
/// `ARCHITECTURE.md`, "Task tracker" → "Parents": the same transaction moves
/// a non-terminal parent whose children have all reached a terminal state
/// into the project's terminal state with the lowest position, clears its
/// lease and writes `state_changed` with actor `system`. Nothing here is
/// conditional on the parent's `blocked` flag: a parent still waiting on a
/// `blocks` prerequisite has no open *children* and closes all the same.
///
/// An already-terminal parent is left alone, and a parent is never reopened
/// when a child leaves a terminal state — that path recomputes its `blocked`
/// flag and stops there.
async fn close_parent_if_last_child(m: &mut TrackerMutation<'_>, task: &Task) -> Result<()> {
    let repository = TaskRepository::new(m.pool());
    let project_id = m.project_id();

    let Some(parent_id) = repository
        .parent_of_in_tx(m.conn(), project_id, task.id)
        .await?
    else {
        return Ok(());
    };

    let Some(parent) = repository
        .find_task_for_update(m.conn(), project_id, TaskRef::Id(parent_id))
        .await?
    else {
        return Ok(());
    };

    let parent_state = state_of(m, parent.state_id).await?;
    if parent_state.kind == TaskStateKind::Terminal {
        return Ok(());
    }

    if repository
        .has_open_children_in_tx(m.conn(), project_id, parent_id)
        .await?
    {
        return Ok(());
    }

    let target = lowest_terminal_state(m).await?;

    // The parent's closure is the orchestrator's doing, whoever closed the
    // child, so the event and its dependants' flips carry actor `system`.
    let caller = m.set_actor(TaskActor::System);
    let closed = apply(
        m,
        &parent,
        &parent_state,
        &target,
        &StateChangeOptions::default(),
        RoundsWrite::Rule,
    )
    .await;
    m.set_actor(caller);
    closed?;

    debug!(
        project_id = %project_id,
        task_id = %parent_id,
        "parent closed with its last open child",
    );

    Ok(())
}

/// The project's terminal state with the lowest position.
///
/// "The project's first terminal state" is a position, not a name: a project
/// that renamed or reordered its terminal columns closes parents into
/// whichever one now sits leftmost (`SPEC.md`, "Tasks").
pub(crate) async fn lowest_terminal_state(m: &mut TrackerMutation<'_>) -> Result<TaskState> {
    let project_id = m.project_id();

    TaskRepository::new(m.pool())
        .list_states_in(m.conn(), project_id)
        .await?
        .into_iter()
        // `list_states_in` is already ordered by position, then name.
        .find(|state| state.kind == TaskStateKind::Terminal)
        .ok_or_else(|| {
            // Unreachable by construction: a project keeps at least one
            // terminal state, and the child just moved into one.
            error!(project_id = %project_id, "project has no terminal state");
            Error::Internal("project has no terminal state".into())
        })
}

/// The state this name refers to in the mutation's project.
///
/// Every route and every tool resolves a state name through here, so an
/// unknown one always answers the same way: [`Error::BadRequest`] — 400 over
/// REST, `invalid_argument` over MCP — naming the valid states in board order,
/// as `SPEC.md`, `update_task` requires.
///
/// Both reads go through the mutation's connection: it already holds one
/// pooled connection with the project locked, and a second acquired here is
/// what makes concurrent mutations on distinct projects queue for the pool
/// instead of for their own project's lock.
pub async fn resolve_state(m: &mut TrackerMutation<'_>, name: &str) -> Result<TaskState> {
    let project_id = m.project_id();
    let repository = TaskRepository::new(m.pool());

    if let Some(state) = repository
        .find_state_by_name_in(m.conn(), project_id, name)
        .await?
    {
        return Ok(state);
    }

    let states = repository.list_states_in(m.conn(), project_id).await?;

    Err(unknown_state(name, &states))
}

/// The same question, without a mutation.
///
/// A board filter (`GET /projects/{pid}/tasks?state=...`) translates a state
/// name to an id and changes nothing, so it has no lock to ask under — and it
/// reads the pool for that reason. It owes the caller the identical message,
/// which is why both forms build it with [`unknown_state`].
pub async fn resolve_state_in_pool(
    pool: &PgPool,
    project_id: Uuid,
    name: &str,
) -> Result<TaskState> {
    let repository = TaskRepository::new(pool);

    if let Some(state) = repository.find_state_by_name(project_id, name).await? {
        return Ok(state);
    }

    let states = repository.list_states(project_id).await?;

    Err(unknown_state(name, &states))
}

/// The one place the unknown-state message is built, for both resolutions.
fn unknown_state(name: &str, states: &[TaskState]) -> Error {
    let valid = states
        .iter()
        .map(|state| state.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");

    Error::BadRequest(format!(
        "unknown state \"{name}\"; valid states are: {valid}"
    ))
}
