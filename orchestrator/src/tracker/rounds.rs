//! The round limit: where a send-back goes once a task has gone round too
//! often.
//!
//! `ARCHITECTURE.md`, "Task tracker" → "Rounds"; `docs/data-model.md`,
//! `tasks`; ADR 0046. `tasks.rounds` counts the revision hand-offs published
//! since the task last left the human state — the counting itself is
//! `tracker::state`'s [`RoundsWrite`](crate::tracker::state::RoundsWrite) —
//! and this module is the one place that decides what a **send-back** does
//! with that count: a forward recording `changes_requested`, or the
//! auto-merge job's move to a conflict state.
//!
//! The rule, once: a send-back by a session or by the system, of a task whose
//! `rounds` has reached the project's `max_rounds`, moves the task to the
//! human state instead of the state it asked for, with `needs_human_reason`
//! `round limit reached (<rounds>/<max_rounds>): <comment>`, an `escalated`
//! event rather than `state_changed`, and the escalation email every move into
//! the human state owes (sent by [`commit_and_notify`](super::commit_and_notify)
//! after the commit). A user's send-back is never redirected: the limit
//! exists to stop automation, and a person asking for changes has decided.
//!
//! Two entry points, for the two shapes of caller:
//!
//! - [`send_back_redirect`] only *decides*. The hand-off path uses it, because
//!   its move goes through `update_task` together with the hand-off record and
//!   the caller's other field changes, and it needs the decision before that.
//! - [`send_back`] decides and moves. It is what a caller whose send-back is a
//!   plain state change — the auto-merge job's conflict — calls, after writing
//!   its own comment.

use crate::events::TaskActor;
use crate::models::{Task, TaskState, TaskStateKind};
use crate::prelude::*;
use crate::tracker::escalation::Escalation;
use crate::tracker::leases::human_state;
use crate::tracker::state::{StateChangeOptions, StateChangeResult, StateEventKind, change_state};
use crate::tracker::{TaskDto, TrackerMutation};

/// A send-back the round limit sends to the human state instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoundLimitRedirect {
    /// The project's human state, which the task moves to.
    pub human: TaskState,
    /// `round limit reached (<rounds>/<max_rounds>): <comment>` — written to
    /// `needs_human_reason`, carried by the `escalated` event and named in the
    /// email.
    pub reason: String,
}

impl RoundLimitRedirect {
    /// The options the redirected move is made with: `escalated`, with the
    /// reason stored on the task.
    pub fn options(&self) -> StateChangeOptions {
        StateChangeOptions {
            event: StateEventKind::Escalated {
                reason: self.reason.clone(),
            },
            needs_human_reason: Some(self.reason.clone()),
        }
    }
}

/// `needs_human_reason`, and the `escalated` event's reason, when the round
/// limit is what escalated the task.
///
/// The count and the send-back's own comment, so the card says both why a
/// person is needed and what the last reviewer or merge asked for — the
/// shape `attempt_limit_reason` gives the attempt limit.
pub fn round_limit_reason(rounds: i16, max_rounds: i16, comment: &str) -> String {
    format!("round limit reached ({rounds}/{max_rounds}): {comment}")
}

/// Whether this send-back is redirected, and where to.
///
/// `task` is the row as it stands under the mutation's lock and `requested`
/// the state the send-back asked for; `comment` is the send-back's own
/// comment, which the reason repeats. The answer is `None` — make the
/// requested move — when any of these holds:
///
/// - the mutation's actor is a user: a person's send-back is never redirected;
/// - `rounds` is below the project's `max_rounds`, read from the project row
///   locked by this mutation, so a limit changed a moment ago counts as it
///   stands now;
/// - the task is already in the human state, or the send-back asks for it:
///   there is nothing to redirect to, and leaving the human state resets the
///   count anyway.
pub async fn send_back_redirect(
    m: &mut TrackerMutation<'_>,
    task: &Task,
    requested: &TaskState,
    comment: &str,
) -> Result<Option<RoundLimitRedirect>> {
    if matches!(m.actor(), TaskActor::User { .. }) {
        return Ok(None);
    }

    let max_rounds = m.project().max_rounds;
    if task.rounds < max_rounds || requested.kind == TaskStateKind::Human {
        return Ok(None);
    }

    let human = human_state(m).await?;
    if task.state_id == human.id {
        return Ok(None);
    }

    Ok(Some(RoundLimitRedirect {
        human,
        reason: round_limit_reason(task.rounds, max_rounds, comment),
    }))
}

/// Queue the escalation email a redirected send-back owes.
///
/// Recorded on the mutation, so it is sent by
/// [`commit_and_notify`](super::commit_and_notify) after the commit and never
/// for a rollback (`tracker::escalation`).
pub fn record_round_limit_escalation(
    m: &mut TrackerMutation<'_>,
    task: &Task,
    redirect: &RoundLimitRedirect,
) {
    m.record_escalation(Escalation {
        project_id: m.project_id(),
        project_name: m.project().name.clone(),
        task_id: task.id,
        task_number: task.number,
        task_title: task.title.clone(),
        assignee_user_id: task.assignee_user_id,
        reason: redirect.reason.clone(),
    });
}

/// Send a task back to `requested`, unless the round limit redirects it.
///
/// For a send-back that is a plain move — the auto-merge job's move to a
/// conflict state — made after the caller has written its comment: the move
/// is [`change_state`]'s either way, into `requested` with `state_changed`, or
/// into the human state with `escalated`, the reason and the email
/// ([`send_back_redirect`]). `task` must be the row read under the mutation's
/// lock.
pub async fn send_back(
    m: &mut TrackerMutation<'_>,
    task: &Task,
    requested: &TaskState,
    comment: &str,
) -> Result<StateChangeResult> {
    match send_back_redirect(m, task, requested, comment).await? {
        Some(redirect) => {
            let result = change_state(m, task, &redirect.human, redirect.options()).await?;
            record_round_limit_escalation(m, task, &redirect);
            log_redirect(m, &result.task);
            Ok(result)
        }
        None => change_state(m, task, requested, StateChangeOptions::default()).await,
    }
}

/// The one log line a redirect writes. The reason is not a field of it: it
/// carries the send-back's comment, which is agent or user content.
pub(crate) fn log_redirect(m: &TrackerMutation<'_>, task: &TaskDto) {
    info!(
        project_id = %m.project_id(),
        task_id = %task.id,
        rounds = task.rounds,
        max_rounds = m.project().max_rounds,
        "send-back redirected to the human state by the round limit",
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reason_names_the_count_the_limit_and_the_comment() {
        assert_eq!(
            round_limit_reason(5, 5, "the migration still drops the index"),
            "round limit reached (5/5): the migration still drops the index",
        );
    }
}
