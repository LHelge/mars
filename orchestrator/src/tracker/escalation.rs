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
//! The struct, the wording and the sender all live here. The texts are the
//! contract — a person reading a `needs_human` card reads exactly these
//! sentences — so they are built here, once, rather than formatted at each of
//! the three call sites in `tracker::leases`, and [`notify`] is the one place
//! that turns them into messages, so the MCP tools, the session hooks and the
//! reaper cannot disagree about who is told.

use uuid::Uuid;

use crate::email::EmailMessage;
use crate::models::User;
use crate::prelude::*;
use crate::repositories::UserRepository;
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

/// Send the escalation emails a committed mutation made due.
///
/// The sender half of the section quoted above: for each escalation, resolve
/// the recipients and send one [`EmailMessage::escalation`] to each.
///
/// **It is called only after the commit.** The signature is what enforces
/// that: it takes the [`AppState`] and *owned* escalations, not a
/// [`TrackerMutation`](super::TrackerMutation), so there is no way to reach it
/// while the project lock is held — [`super::commit_and_notify`] is the pairing
/// every caller uses.
///
/// **Nothing here can fail the tracker operation.** The change has already
/// committed by the time this runs, so there is nothing left to undo and no
/// answer a failure could usefully change; it returns `()` and a failed
/// delivery is logged and stepped over, the next recipient still being told.
/// That is also why a recipient lookup that fails only skips its own
/// escalation.
///
/// The message body is never logged here. With `LogEmailClient` the client
/// itself writes it, which is the one sanctioned exception (ADR 0026,
/// `CLAUDE.md` rule 3); the fields below are ids, never an address or a body.
pub async fn notify(state: &AppState, escalations: Vec<Escalation>) {
    for escalation in escalations {
        let recipients = match recipients(state, &escalation).await {
            Ok(recipients) => recipients,
            Err(error) => {
                error!(
                    project_id = %escalation.project_id,
                    task_id = %escalation.task_id,
                    error = %error,
                    "the escalation recipients could not be read",
                );
                continue;
            }
        };

        if recipients.is_empty() {
            // Legitimate: an unassigned task in an installation where every
            // administrator turned email off. Nothing is owed, so this is not
            // a warning.
            debug!(
                project_id = %escalation.project_id,
                task_id = %escalation.task_id,
                "no escalation recipients",
            );
            continue;
        }

        let link = task_link(&state.config.public_url, &escalation);

        for recipient in recipients {
            let message = EmailMessage::escalation(
                &recipient.email,
                &escalation.project_name,
                escalation.task_number,
                &escalation.task_title,
                &escalation.reason,
                &link,
            );

            if let Err(error) = state.email.send(message).await {
                // The recipient is named by id: an address is personal data
                // and a log line is not the place for it.
                error!(
                    project_id = %escalation.project_id,
                    task_id = %escalation.task_id,
                    recipient = %recipient.id,
                    error = %error,
                    "escalation email failed",
                );
            }
        }
    }
}

/// Who is told about this escalation.
///
/// `ARCHITECTURE.md`, "Task tracker" → "Notification": the assignee if there
/// is one, otherwise every administrator, and never a user whose
/// `notify_email` is off.
///
/// An assignee who opted out therefore receives nothing *and* sends the
/// escalation nowhere else: the fallback is for a task nobody owns, not a way
/// around one owner's choice. An `assignee_user_id` whose row is gone — the
/// user was deleted between the claim and the commit — is the unassigned case,
/// because the task is now effectively nobody's.
async fn recipients(state: &AppState, escalation: &Escalation) -> Result<Vec<User>> {
    let users = UserRepository::new(&state.pool);

    if let Some(assignee_id) = escalation.assignee_user_id
        && let Some(assignee) = users.find(assignee_id).await?
    {
        return Ok(if assignee.notify_email {
            vec![assignee]
        } else {
            Vec::new()
        });
    }

    users.list_admin_recipients().await
}

/// The link the email carries: the task's board page (`SPEC.md`, "Frontend").
///
/// `Config` already strips trailing slashes from `PUBLIC_URL`, so the join is
/// a plain `format!`; the trim is repeated here only so the function is
/// correct for whatever string it is handed, which is what its unit test
/// passes it.
fn task_link(public_url: &str, escalation: &Escalation) -> String {
    format!(
        "{}/projects/{}/tasks/{}",
        public_url.trim_end_matches('/'),
        escalation.project_id,
        escalation.task_number,
    )
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

#[cfg(test)]
mod tests {
    use super::*;

    fn escalation() -> Escalation {
        Escalation {
            project_id: Uuid::nil(),
            project_name: "Apollo".to_string(),
            task_id: Uuid::nil(),
            task_number: 17,
            task_title: "Rotate the deploy key".to_string(),
            assignee_user_id: None,
            reason: "attempt limit reached".to_string(),
        }
    }

    #[test]
    fn the_link_is_the_task_board_route_under_the_public_url() {
        assert_eq!(
            task_link("https://mars.example.invalid", &escalation()),
            "https://mars.example.invalid/projects/00000000-0000-0000-0000-000000000000/tasks/17",
        );
    }

    #[test]
    fn a_trailing_slash_on_the_public_url_is_trimmed() {
        assert_eq!(
            task_link("https://mars.example.invalid/", &escalation()),
            task_link("https://mars.example.invalid", &escalation()),
        );
    }
}
