//! What a move into the human state owes an email.
//!
//! `ARCHITECTURE.md`, "Task tracker" → "Notification": every move into the
//! human state, by the `needs_human` tool or by the reaper, sends one email
//! naming the project, the task, the reason and a link to the task.
//!
//! Sending it is not part of the mutation. Email is one of the operations the
//! same section keeps outside the tracker transaction — "engine, email and
//! model operations run outside the tracker transaction" — so the mutation
//! only *records* that an email is owed, and the caller sends it after
//! [`TrackerMutation::commit`](super::TrackerMutation::commit) hands it back.
//! A rolled-back mutation therefore sends nothing, which is the same promise
//! `pg_notify` makes for events (ADR 0028).
//!
//! That is why this struct is a copy and not a pair of ids: the sender must
//! not have to reopen a transaction to render the message, so everything the
//! email needs travels with it. Recipient *selection* — the assignee, or every
//! admin, minus the users whose `notify_email` is off — is the sender's, since
//! it reads the `users` table and the mutation holds no lock on it.
//!
//! The struct and the wording live here; the sender is a later task. The texts
//! are the contract — a person reading a `needs_human` card reads exactly these
//! sentences — so they are built here, once, rather than formatted at each of
//! the three call sites in `tracker::leases`.

use uuid::Uuid;

// The crate convention (`CLAUDE.md`, "Backend conventions").
#[allow(unused_imports)]
use crate::prelude::*;
use crate::tracker::leases::ReleaseReason;

/// One escalation email owed by a committed mutation.
///
/// Queued with
/// [`TrackerMutation::record_escalation`](super::TrackerMutation::record_escalation)
/// and returned by
/// [`MutationOutcome`](super::MutationOutcome) once the transaction has
/// committed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Escalation {
    /// The project the task belongs to; also what a link to the task is built
    /// from.
    pub project_id: Uuid,
    /// The project's name, as the email names it.
    pub project_name: String,
    /// The escalated task.
    pub task_id: Uuid,
    /// The task's per-project number, which is how people refer to it.
    pub task_number: i32,
    /// The task's title.
    pub task_title: String,
    /// The assignee, who is the sole recipient when there is one; otherwise
    /// the email goes to every admin.
    pub assignee_user_id: Option<Uuid>,
    /// Why the task needs a human: the release reason, or the reaper's.
    pub reason: String,
}

/// The system comment a release by the orchestrator writes on the task.
///
/// `ARCHITECTURE.md`, "Task tracker" → "Liveness comes from the session, not
/// from tool calls": a session ending, and the stuck-task reaper behind it,
/// release every lease the dead session held "and write a system comment on
/// the task". This is that comment, and it names the session so the thread
/// says *whose* lease was taken away.
pub(crate) fn lease_released_comment(session_id: Uuid, reason: ReleaseReason) -> String {
    format!(
        "Lease released by the orchestrator: holder session {session_id} {}.",
        reason.past_tense()
    )
}

/// What an escalation from such a release names as its last reason.
///
/// An agent's own `release` carries the agent's words; nobody spoke for the
/// session that simply stopped, so the orchestrator says what happened to it.
pub(crate) fn session_release_reason(session_id: Uuid, reason: ReleaseReason) -> String {
    format!("session {session_id} {}", reason.past_tense())
}

/// `needs_human_reason`, and the `escalated` event's reason, when the attempt
/// limit is what escalated the task.
///
/// `ARCHITECTURE.md`, "Task tracker" → "Attempts and escalation": the count
/// and the reason of the release that reached it, so the card says both why a
/// person is needed now and what the last attempt made of it.
pub(crate) fn attempt_limit_reason(attempts: i16, max_attempts: i16, last_reason: &str) -> String {
    format!("attempt limit reached ({attempts}/{max_attempts}): {last_reason}")
}

/// The system comment that records the escalation in the task's own thread.
pub(crate) fn escalation_comment(human_state: &str, attempts: i16, reason: &str) -> String {
    format!("Escalated to {human_state} after {attempts} attempts. {reason}")
}
