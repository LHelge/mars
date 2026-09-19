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
//! Only the struct lives here; the sender is a later task.

use uuid::Uuid;

// The crate convention (`CLAUDE.md`, "Backend conventions").
#[allow(unused_imports)]
use crate::prelude::*;

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
