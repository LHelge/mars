//! Taking the lease, listing what could be taken, and giving it back.
//!
//! `ARCHITECTURE.md`, "Task tracker" → "The lease is the worker", "Launching a
//! session for a task" and "Attempts and escalation"; `SPEC.md`, "Tasks" and
//! "MCP tool contracts"; `docs/data-model.md`, `tasks` and `task_sessions`.
//!
//! The state is the queue and the lease is the worker, so everything here is
//! about the lease and nothing here moves a task between states. Four
//! entry points, three of which write:
//!
//! - [`claim_for_profile`] — the MCP `claim` tool. The task must be in one of
//!   the states the calling profile serves, and then the atomic statement
//!   decides: exactly one of two concurrent claims wins.
//! - [`claim_for_launch`] — `POST /projects/{pid}/sessions` with a `task_id`.
//!   The served states do not apply, because the user chose this task for this
//!   session; every non-terminal state qualifies, the human state included.
//! - [`ready_summaries`] — the MCP `ready` tool. A lock-free read that writes
//!   nothing at all: no event, no session link, no touch timestamp (ADR 0030).
//! - [`release_by_user`] — `POST /projects/{pid}/tasks/{id}/release`. Clears
//!   the lease and stops there: the state stays, `attempts` stays, and a user
//!   release never escalates and never comments.
//!
//! **The policy is a list of state ids, never a check in Rust.** Both claims
//! end in the same statement ([`TaskRepository::claim`]) with a different
//! `state_ids`, so the conditions that decide a claim — the project, the
//! states, `NOT blocked`, `lease_holder_session_id IS NULL` — are evaluated
//! once, in one statement, and zero rows is the single conflict answer to all
//! of them. The project lock the [`TrackerMutation`] holds is what serialises
//! two claims within a project; the null check is what keeps the statement
//! honest even without it.
//!
//! **What escalation is not here.** An agent's `release` and the reaper's do
//! escalate once `attempts` reaches the project's `max_attempts`, and they are
//! `tracker::escalation`'s; this module holds the user's release, which is the
//! one that never does.

use uuid::Uuid;

use crate::events::{TaskActor, TaskEventKind, TaskEventPayload};
use crate::models::{Task, TaskStateKind};
use crate::prelude::*;
use crate::repositories::TaskRepository;
use crate::repositories::tasks::{StateFields, TaskSummaryRow};
use crate::tracker::dto::description_excerpt;
use crate::tracker::{TaskDto, TaskSummary, TrackerMutation};

/// The conflict every lost claim answers with (`SPEC.md`, `claim`).
const NOT_CLAIMABLE: &str = "task is not claimable";
/// The conflict a claim outside the profile's served states answers with.
const NOT_SERVED: &str = "task is not in a state this profile serves";
/// The conflict a release of an unheld task answers with (`SPEC.md`, "Tasks").
const NOT_HELD: &str = "task is not held";
/// `TaskEvent.reason` on a release a user made (`SPEC.md`, "TaskEvent").
const USER_RELEASE_REASON: &str = "user";

/// How many tasks `ready` lists when the caller names no limit.
pub const READY_DEFAULT_LIMIT: i64 = 20;
/// The smallest explicit `ready` limit.
pub const READY_MIN_LIMIT: i64 = 1;
/// The largest explicit `ready` limit.
pub const READY_MAX_LIMIT: i64 = 100;

/// What an out-of-range `ready` limit is told; never clamped.
const LIMIT_OUT_OF_RANGE: &str = "limit must be an integer from 1 through 100";

/// Claim a task for a session working a profile's queues (MCP `claim`).
///
/// `task` is the row as it is under this mutation's lock; `served_state_ids`
/// are the profile's served states
/// ([`TaskRepository::list_profile_states`](crate::repositories::TaskRepository::list_profile_states)),
/// which are always queue states, so a task waiting for a person is never
/// served and is refused with [`NOT_SERVED`] rather than with the generic
/// conflict — the agent is told *why* the task is not for it.
///
/// Everything else is the statement's: a blocked task, a held task (including
/// one this very session already holds) and a task that moved out of the
/// served states since it was read all return zero rows, and all answer
/// [`Error::Conflict`] with [`NOT_CLAIMABLE`].
///
/// On success the `claimed` event carries actor `session` and the task as it
/// is after the claim, and the session is linked to the task — a claim is the
/// session working on it (`docs/data-model.md`, `task_sessions`).
pub async fn claim_for_profile(
    m: &mut TrackerMutation<'_>,
    task: &Task,
    session_id: Uuid,
    served_state_ids: &[Uuid],
) -> Result<TaskDto> {
    if !served_state_ids.contains(&task.state_id) {
        return Err(Error::Conflict(NOT_SERVED.into()));
    }

    // The claim is the session's own work, whoever opened the mutation, so the
    // event says `session` by construction rather than by the caller
    // remembering to open it with that actor.
    let caller = m.set_actor(TaskActor::Session { session_id });
    let claimed = claim(m, task, session_id, served_state_ids).await;
    m.set_actor(caller);

    claimed
}

/// Claim a task because a user launched a session for it
/// (`POST /projects/{pid}/sessions` with `task_id`).
///
/// The profile's served states are deliberately ignored: the user chose this
/// task for this session, so the only question is whether the task is free to
/// be worked — unheld, unblocked and not already finished (`ARCHITECTURE.md`,
/// "Task tracker" → "Launching a session for a task"). Every non-terminal
/// state of the project therefore qualifies, the human state included, and a
/// terminal, blocked or held task is [`Error::Conflict`] with
/// [`NOT_CLAIMABLE`] — the 409 `SPEC.md` gives that endpoint.
///
/// The `claimed` event carries the mutation's actor, which is the launching
/// user, and the session is linked to the task. The caller runs this inside
/// the transaction that inserts the session row, so the session need only
/// exist on this connection, not be committed.
pub async fn claim_for_launch(
    m: &mut TrackerMutation<'_>,
    task: &Task,
    session_id: Uuid,
) -> Result<TaskDto> {
    let state_ids = TaskRepository::new(m.pool())
        .list_states(m.project_id())
        .await?
        .into_iter()
        .filter(|state| state.kind != TaskStateKind::Terminal)
        .map(|state| state.id)
        .collect::<Vec<_>>();

    claim(m, task, session_id, &state_ids).await
}

/// The statement, its conflict, its event and its link: the half both claims
/// share.
async fn claim(
    m: &mut TrackerMutation<'_>,
    task: &Task,
    session_id: Uuid,
    state_ids: &[Uuid],
) -> Result<TaskDto> {
    let repository = TaskRepository::new(m.pool());
    let project_id = m.project_id();

    if repository
        .claim(m.conn(), project_id, task.id, session_id, state_ids)
        .await?
        .is_none()
    {
        return Err(Error::Conflict(NOT_CLAIMABLE.into()));
    }

    let dto = repository
        .load_task_dto_in(m.conn(), project_id, task.id)
        .await?
        .ok_or(Error::NotFound)?;

    m.emit_task(TaskEventKind::Claimed, &dto)?;
    m.touch(task.id, session_id);

    info!(
        project_id = %project_id,
        task_id = %task.id,
        session_id = %session_id,
        attempts = dto.attempts,
        "task claimed",
    );

    Ok(dto)
}

/// The tasks a profile serving `served_state_ids` could claim now (MCP
/// `ready`).
///
/// A read and only a read: no lock, no event, no session link and no touch
/// timestamp, because listing work is not working on it (ADR 0021, ADR 0030).
/// It therefore takes the pool rather than a mutation, and a task it lists may
/// of course be claimed by somebody else a moment later — which is what
/// [`claim_for_profile`]'s conflict is for.
///
/// `limit` is an integer from 1 through 100; anything else is
/// [`Error::BadRequest`] and is never clamped (`SPEC.md`, `ready`). The MCP
/// boundary rejects a non-integer before it reaches here; this is the same
/// range check for every other caller.
///
/// A profile serving no states gets an empty list.
pub async fn ready_summaries(
    pool: &PgPool,
    project_id: Uuid,
    served_state_ids: &[Uuid],
    limit: i64,
) -> Result<Vec<TaskSummary>> {
    if !(READY_MIN_LIMIT..=READY_MAX_LIMIT).contains(&limit) {
        return Err(Error::BadRequest(LIMIT_OUT_OF_RANGE.into()));
    }

    let rows = TaskRepository::new(pool)
        .list_claimable(project_id, served_state_ids, limit)
        .await?;

    Ok(rows.into_iter().map(summary).collect())
}

/// One claimable row as the `ready` tool sends it.
fn summary(row: TaskSummaryRow) -> TaskSummary {
    TaskSummary {
        id: row.id,
        number: row.number,
        title: row.title,
        state: row.state,
        priority: row.priority,
        labels: row.labels,
        description_excerpt: description_excerpt(&row.description),
        attempts: row.attempts,
        depends_on_count: row.depends_on_count,
    }
}

/// Clear a lease because a user said so
/// (`POST /projects/{pid}/tasks/{id}/release`).
///
/// "A release clears the lease and keeps the state... A release by a user
/// never escalates" (`docs/data-model.md`, `tasks`). So this writes two
/// columns and nothing else: the state, `closed_at` and `attempts` all stay as
/// they are — `attempts` deliberately, so that a later agent release still
/// escalates on the right count, since only a state change resets it.
///
/// A task nobody holds is [`Error::Conflict`] with [`NOT_HELD`] (409), decided
/// against the row read under this mutation's lock. A holder whose session has
/// already ended is released like any other: the lease is a row, not a live
/// connection, and the reaper simply finds nothing left to do.
///
/// The `released` event carries the mutation's actor — the user — and
/// `reason: "user"` (`SPEC.md`, "TaskEvent"). No comment is written and no
/// session link is created: a user's change is not session work.
pub async fn release_by_user(m: &mut TrackerMutation<'_>, task: &Task) -> Result<TaskDto> {
    if task.lease_holder_session_id.is_none() {
        return Err(Error::Conflict(NOT_HELD.into()));
    }

    let repository = TaskRepository::new(m.pool());
    let project_id = m.project_id();

    repository
        .set_task_state_fields(
            m.conn(),
            project_id,
            task.id,
            &StateFields {
                lease: Some(None),
                ..StateFields::default()
            },
        )
        .await?;

    let dto = repository
        .load_task_dto_in(m.conn(), project_id, task.id)
        .await?
        .ok_or(Error::NotFound)?;

    let mut payload = TaskEventPayload::new(m.actor());
    payload.task = Some(dto.clone());
    payload.reason = Some(USER_RELEASE_REASON.to_string());
    m.emit(TaskEventKind::Released, Some(dto.id), payload)?;

    info!(
        project_id = %project_id,
        task_id = %task.id,
        "task released by a user",
    );

    Ok(dto)
}
