//! Taking the lease, listing what could be taken, and giving it back.
//!
//! `ARCHITECTURE.md`, "Task tracker" → "The lease is the worker", "Launching a
//! session for a task" and "Attempts and escalation"; `SPEC.md`, "Tasks" and
//! "MCP tool contracts"; `docs/data-model.md`, `tasks` and `task_sessions`.
//!
//! The state is the queue and the lease is the worker, so everything here is
//! about the lease — and the one move a lease ending can make by itself, which
//! is into the human state. Seven entry points, six of which write:
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
//! - [`release_by_agent`] — the MCP `release` tool. The caller must hold the
//!   lease; its reason becomes a comment; at `max_attempts` the lease is not
//!   handed back to the queue but to a person.
//! - [`release_leases_for_session`] — a session that ended or stalled, from
//!   the session hooks now and the stuck-task reaper later. The same rule,
//!   for every task the dead session still held, with actor `system`.
//! - [`needs_human`] — the MCP `needs_human` tool. An agent asking for a
//!   person whatever `attempts` says.
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
//! **Escalation is the one thing a release can do besides clear the lease.**
//! `ARCHITECTURE.md`, "Task tracker" → "Attempts and escalation": a release by
//! the agent or by the orchestrator that finds `attempts` at the project's
//! `max_attempts` moves the task to the project's human state instead of
//! leaving it in its queue, with a system comment, `needs_human_reason` and an
//! `escalated` event. A user's release never does. The wording of every text
//! it writes is `tracker::escalation`'s, and the email it makes due is
//! recorded on the mutation and sent by the caller after the commit.

use uuid::Uuid;

use crate::events::{TaskActor, TaskEventKind, TaskEventPayload};
use crate::models::{Task, TaskRef, TaskState, TaskStateKind};
use crate::prelude::*;
use crate::repositories::tasks::{StateFields, TaskSummaryRow};
use crate::repositories::{SessionRepository, TaskRepository};
use crate::tracker::dto::description_excerpt;
use crate::tracker::escalation::{
    attempt_limit_reason, escalation_comment, lease_released_comment, session_release_reason,
};
use crate::tracker::state::{StateChangeOptions, StateEventKind, change_state};
use crate::tracker::{
    CommentAuthor, Escalation, TaskDto, TaskSummary, TrackerMutation, add_comment,
};

/// The conflict every lost claim answers with (`SPEC.md`, `claim`).
const NOT_CLAIMABLE: &str = "task is not claimable";
/// The conflict a claim outside the profile's served states answers with.
const NOT_SERVED: &str = "task is not in a state this profile serves";
/// The conflict a release of an unheld task answers with (`SPEC.md`, "Tasks").
const NOT_HELD: &str = "task is not held";
/// The conflict an agent releasing a task it does not hold answers with
/// (`SPEC.md`, `release`).
const NOT_HELD_BY_SESSION: &str = "task is not held by this session";
/// The conflict `needs_human` answers when somebody else holds the task.
const HELD_BY_ANOTHER: &str = "task is held by another session";
/// `TaskEvent.reason` on a release a user made (`SPEC.md`, "TaskEvent").
const USER_RELEASE_REASON: &str = "user";
/// `TaskEvent.reason` on a release an agent made, by `release` or by
/// `needs_human` on a task already waiting for a person.
const AGENT_RELEASE_REASON: &str = "given_back";

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
    let project_id = m.project_id();

    let state_ids = TaskRepository::new(m.pool())
        .list_states_in(m.conn(), project_id)
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

    let dto = clear_lease(m, task, USER_RELEASE_REASON).await?;

    info!(
        project_id = %m.project_id(),
        task_id = %task.id,
        "task released by a user",
    );

    Ok(dto)
}

/// Clear a lease because the agent holding it gave the task back (MCP
/// `release`).
///
/// The caller must hold the lease — a session releasing somebody else's task
/// is [`Error::Conflict`] with [`NOT_HELD_BY_SESSION`], decided against the row
/// read under this mutation's lock — and its `reason` is written as a comment
/// by that session, so the next worker reads why the last one stopped
/// (`SPEC.md`, `release`).
///
/// Then the counter decides. Below the project's `max_attempts` this is an
/// ordinary release: the state stays, `attempts` stays, and `released` carries
/// `reason: "given_back"`. At the limit the task goes to a person instead
/// ([`escalate`]), because a fourth agent would only fail the way the first
/// three did (`ARCHITECTURE.md`, "Task tracker" → "Attempts and escalation").
/// Either way the session worked on the task, so the link is written.
pub async fn release_by_agent(
    m: &mut TrackerMutation<'_>,
    task: &Task,
    session_id: Uuid,
    reason: &str,
) -> Result<TaskDto> {
    if task.lease_holder_session_id != Some(session_id) {
        return Err(Error::Conflict(NOT_HELD_BY_SESSION.into()));
    }

    add_comment(m, task, CommentAuthor::Session(session_id), reason).await?;

    let dto = if at_attempt_limit(m, task) {
        let reason = attempt_limit_reason(task.attempts, m.project().max_attempts, reason);
        escalate(m, task, &reason, AGENT_RELEASE_REASON).await?
    } else {
        clear_lease(m, task, AGENT_RELEASE_REASON).await?
    };

    m.touch(task.id, session_id);

    info!(
        project_id = %m.project_id(),
        task_id = %task.id,
        session_id = %session_id,
        attempts = task.attempts,
        "task released by an agent",
    );

    Ok(dto)
}

/// Why the orchestrator, rather than a person or an agent, ended a lease.
///
/// The two reasons `SPEC.md`, "TaskEvent" gives a `released` event that nobody
/// asked for: the holder is `done` (`session_ended`) or the idle reaper marked
/// it `failed` with error `stalled` (`stalled`). Both mean the same thing to
/// the tracker — the worker is gone — and differ only in what the event and
/// the comment say (`ARCHITECTURE.md`, "Task tracker" → "Liveness comes from
/// the session, not from tool calls").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseReason {
    /// The holding session reached `done` or `failed` of its own accord.
    SessionEnded,
    /// The idle reaper found an ephemeral session silent and failed it.
    Stalled,
}

impl ReleaseReason {
    /// The `TaskEvent.reason` this writes (`SPEC.md`, "TaskEvent").
    pub fn event_reason(self) -> &'static str {
        match self {
            Self::SessionEnded => "session_ended",
            Self::Stalled => "stalled",
        }
    }

    /// How a sentence about the session ends: "session <id> *ended*".
    pub(crate) fn past_tense(self) -> &'static str {
        match self {
            Self::SessionEnded => "ended",
            Self::Stalled => "stalled",
        }
    }
}

/// Release every lease a dead session still holds.
///
/// The session hooks call this the moment a session becomes `done` or
/// `failed`, and the stuck-task reaper calls it again as the backstop
/// (`ARCHITECTURE.md`, "Task tracker" → "Liveness comes from the session, not
/// from tool calls"). Each task gets a system comment naming the holder and
/// what became of it, and then the same decision an agent's release makes:
/// back to its queue with `released`, or to a person when `attempts` has
/// reached the project's `max_attempts`.
///
/// Actor `system` throughout, and no `task_sessions` link: the orchestrator
/// releasing a lease is not the session having worked on the task, and the
/// link the claim wrote is already there (ADR 0030).
///
/// **It never touches the session row.** The holder is looked up on the pool
/// and the only rows locked are the project's and the tasks', so this composes
/// with the session-state writers that lock a session first, and keeps the
/// documented lock order without a lock of its own (ADR 0021). It may
/// therefore be called with the session already `done` or `failed`, which is
/// exactly when it is called.
///
/// A session holding nothing opens no mutation at all: no lock, no events, no
/// rows. The returned escalations are the emails the commit made due, for the
/// caller to send outside the transaction.
pub async fn release_leases_for_session(
    pool: &PgPool,
    session_id: Uuid,
    reason: ReleaseReason,
) -> Result<Vec<Escalation>> {
    let Some(session) = SessionRepository::new(pool).find(session_id).await? else {
        return Err(Error::NotFound);
    };

    let held = TaskRepository::new(pool)
        .list_by_lease_holder(session_id)
        .await?;
    if held.is_empty() {
        return Ok(Vec::new());
    }

    let mut m = TrackerMutation::begin(pool, session.project_id, TaskActor::System).await?;
    let project_id = m.project_id();

    for candidate in held {
        // The list was read before the lock, so the lease may have been given
        // back, taken over or escalated in between: the locked row decides.
        let Some(task) = TaskRepository::new(pool)
            .find_task_for_update(m.conn(), project_id, TaskRef::Id(candidate.id))
            .await?
        else {
            continue;
        };
        if task.lease_holder_session_id != Some(session_id) {
            continue;
        }

        let comment = lease_released_comment(session_id, reason);
        add_comment(&mut m, &task, CommentAuthor::System, &comment).await?;

        if at_attempt_limit(&m, &task) {
            let text = attempt_limit_reason(
                task.attempts,
                m.project().max_attempts,
                &session_release_reason(session_id, reason),
            );
            escalate(&mut m, &task, &text, reason.event_reason()).await?;
        } else {
            clear_lease(&mut m, &task, reason.event_reason()).await?;
        }

        info!(
            project_id = %project_id,
            task_id = %task.id,
            session_id = %session_id,
            reason = reason.event_reason(),
            "lease released by the orchestrator",
        );
    }

    Ok(m.commit().await?.escalations)
}

/// Hand a task to a person because an agent asked for one (MCP
/// `needs_human`).
///
/// The caller must hold the lease or the task must be unheld — somebody else's
/// task is [`Error::Conflict`] with [`HELD_BY_ANOTHER`] — and the `reason` is
/// written as that session's comment and stored as `needs_human_reason`
/// (`SPEC.md`, `needs_human`). `attempts` plays no part: an agent that knows a
/// person is needed says so on the first attempt.
///
/// A task already in the human state is not escalated twice. The reason is
/// recorded, any lease this session holds is explicitly released, `attempts`
/// is preserved, and the mutation emits `commented`, `updated` and — when
/// there was a lease — `released`, with no `escalated` event and no second
/// email. Either way the session worked on the task, so the link is written,
/// including for the unheld task this session never claimed.
pub async fn needs_human(
    m: &mut TrackerMutation<'_>,
    task: &Task,
    session_id: Uuid,
    reason: &str,
) -> Result<TaskDto> {
    if let Some(holder) = task.lease_holder_session_id
        && holder != session_id
    {
        return Err(Error::Conflict(HELD_BY_ANOTHER.into()));
    }

    add_comment(m, task, CommentAuthor::Session(session_id), reason).await?;

    let human = human_state(m).await?;
    let dto = if task.state_id == human.id {
        let release =
            (task.lease_holder_session_id == Some(session_id)).then_some(AGENT_RELEASE_REASON);
        record_reason(m, task, reason, release).await?
    } else {
        move_to_human(m, task, &human, reason).await?
    };

    m.touch(task.id, session_id);

    info!(
        project_id = %m.project_id(),
        task_id = %task.id,
        session_id = %session_id,
        "task handed to a person",
    );

    Ok(dto)
}

/// Whether this release is the one that runs out of attempts.
///
/// The comparison is against the project row read under this mutation's lock,
/// so a `max_attempts` a user changed between the claim and the release counts
/// as it stands now.
fn at_attempt_limit(m: &TrackerMutation<'_>, task: &Task) -> bool {
    task.attempts >= m.project().max_attempts
}

/// The escalating half of a release: a system comment, the move and the email.
///
/// `reason_text` is the whole explanation — the count and the last reason,
/// from [`attempt_limit_reason`] — and it is written three times over, as it
/// must be: into the comment thread, into `needs_human_reason` and into the
/// `escalated` event.
///
/// `release_reason` is what a `released` event says when there is no move to
/// make: a task *already* in the human state is not escalated a second time,
/// so the lease is simply cleared and the reason recorded, exactly as
/// `needs_human` does in that case (`SPEC.md`, `needs_human`). That is a user
/// having launched an agent on an escalated task and the agent failing again;
/// a second email would tell nobody anything new.
async fn escalate(
    m: &mut TrackerMutation<'_>,
    task: &Task,
    reason_text: &str,
    release_reason: &str,
) -> Result<TaskDto> {
    let human = human_state(m).await?;

    if task.state_id == human.id {
        return record_reason(m, task, reason_text, Some(release_reason)).await;
    }

    let comment = escalation_comment(&human.name, task.attempts, reason_text);
    add_comment(m, task, CommentAuthor::System, &comment).await?;

    move_to_human(m, task, &human, reason_text).await
}

/// The move itself: `escalated`, `needs_human_reason`, and the email it owes.
///
/// [`change_state`] clears the lease, resets `attempts` and writes the event;
/// the escalation is recorded afterwards, on the mutation, so that a rollback
/// sends nothing (`tracker::escalation`).
async fn move_to_human(
    m: &mut TrackerMutation<'_>,
    task: &Task,
    human: &TaskState,
    reason: &str,
) -> Result<TaskDto> {
    let result = change_state(
        m,
        task,
        human,
        StateChangeOptions {
            event: StateEventKind::Escalated {
                reason: reason.to_string(),
            },
            needs_human_reason: Some(reason.to_string()),
        },
    )
    .await?;

    m.record_escalation(Escalation {
        project_id: m.project_id(),
        project_name: m.project().name.clone(),
        task_id: task.id,
        task_number: task.number,
        task_title: task.title.clone(),
        assignee_user_id: task.assignee_user_id,
        reason: reason.to_string(),
    });

    Ok(result.task)
}

/// Record a new reason on a task that is already waiting for a person, and
/// release the lease when there is one to release.
///
/// Neither an escalation nor a hand-off: the task does not move, `attempts`
/// survives and no email is owed, so the change the board hears about is an
/// `updated` — followed by a `released`, because the lease ending is its own
/// fact and `SPEC.md`, `needs_human` requires it to be announced as one.
async fn record_reason(
    m: &mut TrackerMutation<'_>,
    task: &Task,
    reason: &str,
    release_reason: Option<&str>,
) -> Result<TaskDto> {
    let repository = TaskRepository::new(m.pool());
    let project_id = m.project_id();

    repository
        .set_task_state_fields(
            m.conn(),
            project_id,
            task.id,
            &StateFields {
                needs_human_reason: Some(Some(reason.to_string())),
                ..StateFields::default()
            },
        )
        .await?;

    let dto = repository
        .load_task_dto_in(m.conn(), project_id, task.id)
        .await?
        .ok_or(Error::NotFound)?;
    m.emit_task(TaskEventKind::Updated, &dto)?;

    match release_reason {
        Some(release_reason) => clear_lease(m, task, release_reason).await,
        None => Ok(dto),
    }
}

/// The two columns a release writes, and the event that announces them.
///
/// Shared by every release: the state, `closed_at` and `attempts` all stay as
/// they are — `attempts` deliberately, so that a later agent release still
/// escalates on the right count, since only a state change resets it. The
/// `released` event carries the mutation's actor and the caller's `reason`
/// (`SPEC.md`, "TaskEvent").
async fn clear_lease(m: &mut TrackerMutation<'_>, task: &Task, reason: &str) -> Result<TaskDto> {
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
    payload.reason = Some(reason.to_string());
    m.emit(TaskEventKind::Released, Some(dto.id), payload)?;

    Ok(dto)
}

/// The project's human state: the one state of kind `human`.
///
/// Resolved by kind and never by name, because a project may rename the column
/// (`ARCHITECTURE.md`, "Task tracker"). A project without one cannot exist —
/// the deletion refusals keep exactly one — so its absence is
/// [`Error::Internal`] with the fact logged, not a caller error.
async fn human_state(m: &mut TrackerMutation<'_>) -> Result<TaskState> {
    let project_id = m.project_id();

    TaskRepository::new(m.pool())
        .list_states_in(m.conn(), project_id)
        .await?
        .into_iter()
        .find(|state| state.kind == TaskStateKind::Human)
        .ok_or_else(|| {
            error!(project_id = %project_id, "project has no human state");
            Error::Internal("project has no human state".into())
        })
}
