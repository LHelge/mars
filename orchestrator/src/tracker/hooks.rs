//! What the tracker does when a session dies.
//!
//! `ARCHITECTURE.md`, "Task tracker" → "Liveness comes from the session, not
//! from tool calls": a task's lease lives as long as the session holding it,
//! and dead means the session reached `done` or `failed`. Ending a session
//! from the UI releases its leases immediately; an ephemeral session that
//! finishes, a launch that fails in `creating` and recovery that gives up on a
//! session all do the same, because every one of those paths ends in
//! [`AppState::session_ended`]. The stuck-task reaper is only the backstop and
//! calls [`release_leases_for_session`] itself.
//!
//! This module is the join: the session lifecycle knows nothing about tasks
//! and the tracker knows nothing about containers, so the one function the
//! lifecycle runs when a session dies is installed here, at startup, as
//! [`AppState::on_session_ended`] ([`session_ended_hook`]).
//!
//! **After the commit, never inside it.** The hook is called by the ending
//! paths once their session-state transaction has committed, and it opens a
//! tracker mutation of its own. That is not an optimisation: the documented
//! lock order is git lock → project row → session rows → task rows (ADR 0021;
//! `ARCHITECTURE.md`, "Task tracker"), so a project lock taken while a session
//! row is locked would invert it. [`release_leases_for_session`] never touches
//! the session row for the same reason.
//!
//! **Nothing here can fail a session transition.** The session is already
//! `done` or `failed`; a tracker failure is logged with the session id and
//! swallowed, and the reaper picks the leases up later.
//!
//! **A deleted session is a no-op, and delete depends on that.** A session row
//! that is gone answers [`Error::NotFound`], logged at `debug` and returned
//! from. The `task_sessions` and lease foreign keys have already nulled
//! `lease_holder_session_id`, but `tasks` carries
//! `CHECK ((lease_holder_session_id IS NULL) = (lease_since IS NULL))`
//! (`docs/data-model.md`, `tasks`), so that `SET NULL` cannot succeed on a
//! task whose lease was still held: deleting a session only works once its
//! leases are released, which is why `DELETE /sessions/{id}` is refused for
//! anything but a `done` or `failed` session — one this hook has already run
//! for.
//!
//! **Idempotent.** Called twice for one session, the second call locks the
//! project, re-reads the tasks and finds none it still holds, so it writes no
//! comment and emits no event.

use uuid::Uuid;

use crate::models::SessionState;
use crate::prelude::*;
use crate::repositories::SessionRepository;
use crate::tracker::escalation;
use crate::tracker::leases::{ReleaseReason, release_leases_for_session};

/// The `sessions.error` the idle reaper writes, and the one value that makes a
/// release [`ReleaseReason::Stalled`] rather than
/// [`ReleaseReason::SessionEnded`] (`SPEC.md`, "TaskEvent";
/// `ARCHITECTURE.md`, "Session lifecycle").
const STALLED: &str = "stalled";

/// Release everything a dead session held, and send what that made due.
///
/// The whole of the tracker's side of a session ending: the leases go back to
/// their queues — or to a person, when `attempts` has reached the project's
/// `max_attempts` — and the escalation emails the mutation recorded are sent
/// after it committed, which is the pairing every tracker caller uses
/// (`tracker::commit_and_notify`).
///
/// Returns `()`: see the module documentation for why a failure here is logged
/// and swallowed rather than answered to the caller.
pub async fn on_session_dead(state: &AppState, session_id: Uuid, reason: ReleaseReason) {
    let escalations = match release_leases_for_session(&state.pool, session_id, reason).await {
        Ok(escalations) => escalations,
        Err(Error::NotFound) => {
            debug!(
                session_id = %session_id,
                "the session was deleted before its leases were released",
            );
            return;
        }
        Err(error) => {
            error!(
                session_id = %session_id,
                error = %error,
                "releasing the leases of an ended session failed",
            );
            return;
        }
    };

    escalation::notify(state, escalations).await;
}

/// The hook `main` and `TestApp` install on the state
/// ([`AppState::with_session_ended_hook`]).
///
/// [`SessionEndedHook`] is handed only the session id, so the reason is read
/// back off the row: a `failed` session whose `error` is `stalled` is the idle
/// reaper's, and everything else — an ended conversation, a finished ephemeral
/// session, a failed launch — is [`ReleaseReason::SessionEnded`].
///
/// The captured state is this state with the hook field cleared, which is what
/// keeps it from being an `Arc` cycle: the closure is stored *on* the state it
/// needs in order to reach the email client, so it holds a copy that points at
/// no hook. Every other field is the same allocation, so the hook sends
/// through the very client the rest of the process sends through — which is
/// what lets a test assert on the mock.
pub fn session_ended_hook(state: &AppState) -> SessionEndedHook {
    let mut state = state.clone();
    state.on_session_ended = None;

    Arc::new(move |session_id| {
        let state = state.clone();

        Box::pin(async move {
            let reason = release_reason(&state, session_id).await;
            on_session_dead(&state, session_id, reason).await;
        })
    })
}

/// Why this session's leases are being released, read from its row.
///
/// A row that is gone, or a read that fails, is [`ReleaseReason::SessionEnded`]
/// — the release itself answers for the missing row, and the reason only
/// decides what the event and the comment say.
async fn release_reason(state: &AppState, session_id: Uuid) -> ReleaseReason {
    match SessionRepository::new(&state.pool).find(session_id).await {
        Ok(Some(session))
            if session.state == SessionState::Failed
                && session.error.as_deref() == Some(STALLED) =>
        {
            ReleaseReason::Stalled
        }
        Ok(_) => ReleaseReason::SessionEnded,
        Err(error) => {
            error!(
                session_id = %session_id,
                error = %error,
                "the ended session could not be read; releasing as session_ended",
            );
            ReleaseReason::SessionEnded
        }
    }
}
