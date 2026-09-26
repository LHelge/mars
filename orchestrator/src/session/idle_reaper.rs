//! The idle reaper's body: park the conversational sessions that have gone
//! quiet and fail the ephemeral ones (`ARCHITECTURE.md`, "Background jobs", the
//! idle reaper row; "Task tracker", "Liveness comes from the session").
//!
//! Once a tick (`REAPER_INTERVAL_SECS`, a minute by default), [`reap_idle`]
//! asks for every `running` session whose `last_activity_at` is older than its
//! *own* profile's `idle_timeout_secs` and asks that session's owner to stop.
//! Which stop it is follows from the kind and nothing else: a conversational
//! session is [`StopReason::Idle`] and ends `parked`, an ephemeral one is
//! [`StopReason::Stalled`] and ends `failed` with `sessions.error =
//! "stalled"`, because an ephemeral session is never parked, resumed or
//! retried (ADR 0003).
//!
//! **The normal path writes nothing.** The owner is the single writer for its
//! session: it sends the signals, drains the transcript to end of file and
//! performs the transition when the container exits, which is what lets an
//! ephemeral session whose `result` arrives during the stop still finish
//! `done`. The reaper only asks (`ARCHITECTURE.md`, "Session owner task").
//!
//! Two things it does write.
//!
//! A CLI that ignores both `SIGINT` and `SIGTERM` is still running a tick
//! later, and its registry entry still says a stop is under way. Once that stop
//! is older than the whole documented sequence twice over plus a minute
//! ([`sigkill_after`]), the reaper sends `SIGKILL` itself: the owner's watch
//! then sees the exit and finishes the session the ordinary way.
//!
//! And a `running` row with no registry entry has no owner to ask — a restart
//! that could not adopt it, an owner that ended without clearing the row — so
//! the reaper performs the transition itself, under the session row lock alone
//! (no project lock, no git lock), removes the container if the row still names
//! one and invokes the end-of-session hook when the session ends `failed`.
//!
//! `now` is the caller's, never `NOW()`: an integration test puts one side of a
//! timeout on either side of it without sleeping through a real one.

use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::time::Instant;
use uuid::Uuid;

use crate::cron::JobReport;
use crate::engine::{ContainerId, EngineError, Signal};
use crate::models::{Session, SessionKind, SessionState};
use crate::prelude::*;
use crate::repositories::{IdleSession, SessionRepository, Transition};
use crate::session::owner::StopReason;
use crate::session::registry::StopOutcome;

/// How long past the stop sequence a `SIGKILL` waits, on top of twice
/// `STOP_GRACE_SECS`.
///
/// The doubling covers the documented sequence itself — `SIGINT`, then
/// `SIGTERM` one grace period later — and this minute is the tick after it, so
/// a `STOP_GRACE_SECS` of `0` still leaves a whole minute before a process is
/// killed uninterruptibly (`ARCHITECTURE.md`, "Stop semantics").
pub const SIGKILL_GRACE: Duration = Duration::from_secs(60);

/// How long a pending stop may run before `SIGKILL` is due.
pub fn sigkill_after(config: &Config) -> Duration {
    Duration::from_secs(2 * config.stop_grace_secs) + SIGKILL_GRACE
}

/// What one idle session cost the report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Acted {
    /// Something was done about it: a stop sent, a `SIGKILL` delivered, a
    /// transition written.
    Yes,
    /// Deliberately passed over: a stop already under way and not yet old
    /// enough to escalate, or a row that changed state under the fallback.
    No,
}

/// Stop every `running` session that has been idle longer than its profile
/// allows.
///
/// `Err` only when the selection itself fails; one session's failure is logged
/// against its own id, counted in [`JobReport::failures`] and does not stop the
/// sweep.
pub async fn reap_idle(state: &AppState, now: DateTime<Utc>) -> Result<JobReport> {
    let idle = SessionRepository::new(&state.pool)
        .list_idle_running(now)
        .await?;
    let mut report = JobReport::default();

    for session in idle {
        let session_id = session.session.id;
        match reap_one(state, &session, now).await {
            Ok(Acted::Yes) => report.items += 1,
            Ok(Acted::No) => report.skipped += 1,
            Err(error) => {
                error!(
                    session_id = %session_id,
                    error = %error,
                    "an idle session could not be reaped",
                );
                report.failures += 1;
            }
        }
    }

    Ok(report)
}

/// One idle session: ask its owner, escalate, or do it without one.
async fn reap_one(state: &AppState, idle: &IdleSession, now: DateTime<Utc>) -> Result<Acted> {
    let session = &idle.session;
    let session_id = session.id;
    let kind = session.kind;
    // The kind decides the reason, and the owner corrects a reason that does
    // not match the session's kind anyway (`ARCHITECTURE.md`, "Stop
    // semantics").
    let reason = match kind {
        SessionKind::Ephemeral => StopReason::Stalled,
        SessionKind::Conversational => StopReason::Idle,
    };

    match state.session_registry.stop(session_id, reason) {
        StopOutcome::Sent => {
            let idle_secs = (now - session.last_activity_at).num_seconds();
            let timeout_secs = idle.idle_timeout_secs;
            let action = match kind {
                SessionKind::Ephemeral => "stalling",
                SessionKind::Conversational => "parking",
            };
            info!(
                session_id = %session_id,
                kind = ?kind,
                idle_secs,
                timeout_secs,
                "idle session: {action}",
            );
            Ok(Acted::Yes)
        }
        // Compared against `Instant::now()` and not against `now`: the
        // registry's clock is the monotonic one the owner's grace period runs
        // on, and the two are not the same scale.
        StopOutcome::AlreadyStopping { since } => {
            escalate(
                state,
                session,
                Instant::now().saturating_duration_since(since),
            )
            .await
        }
        StopOutcome::NoOwner => without_owner(state, session, reason).await,
    }
}

/// A stop that is still pending: `SIGKILL` once it has outlived the whole
/// sequence, otherwise nothing.
async fn escalate(state: &AppState, session: &Session, pending_for: Duration) -> Result<Acted> {
    if pending_for <= sigkill_after(&state.config) {
        return Ok(Acted::No);
    }

    // No container to signal is not an escalation: the owner is between
    // containers and its own exit path will finish the session.
    let Some(container_id) = session.container_id.as_deref() else {
        return Ok(Acted::No);
    };

    state
        .engine
        .kill(&ContainerId(container_id.to_string()), Signal::Sigkill)
        .await?;
    warn!(
        session_id = %session.id,
        "CLI ignored SIGINT and SIGTERM; sent SIGKILL",
    );

    Ok(Acted::Yes)
}

/// A `running` row nobody owns: write the transition the owner would have
/// written, and clean up after it.
///
/// One transaction for the state change and its `state_change` event, the
/// session row lock and nothing else (ADR 0021). A [`Error::Conflict`] means
/// the row left `running` between the selection and the lock — an owner that
/// was finishing after all — and is a skip rather than a failure.
async fn without_owner(state: &AppState, session: &Session, reason: StopReason) -> Result<Acted> {
    let session_id = session.id;
    let ephemeral = session.kind == SessionKind::Ephemeral;
    let to = if ephemeral {
        SessionState::Failed
    } else {
        SessionState::Parked
    };

    let recorded = reason.recorded();
    let mut change = Transition::new(SessionState::Running, to, recorded);
    if ephemeral {
        change = change.with_error(recorded);
    }

    let repository = SessionRepository::new(&state.pool);
    let mut tx = state.pool.begin().await?;
    match repository.transition(&mut tx, session_id, &change).await {
        Ok(_) => tx.commit().await?,
        Err(Error::Conflict(conflict)) => {
            debug!(
                session_id = %session_id,
                conflict = %conflict,
                "an idle session left running before the reaper could park it",
            );
            return Ok(Acted::No);
        }
        Err(error) => return Err(error),
    }

    info!(
        session_id = %session_id,
        kind = ?session.kind,
        state = %to,
        reason = recorded,
        "an idle session with no owner was ended by the reaper",
    );

    if let Some(container_id) = session.container_id.as_deref() {
        discard(state, session_id, &ContainerId(container_id.to_string())).await?;
        let mut tx = state.pool.begin().await?;
        repository
            .set_container_id(&mut tx, session_id, None)
            .await?;
        tx.commit().await?;
    }

    if to == SessionState::Failed {
        // A failed session holds nothing any more; releasing what it claimed is
        // this path's job, not the stuck-task reaper's backstop
        // (`ARCHITECTURE.md`, "Task tracker").
        state.session_ended(session_id).await;
    }

    Ok(Acted::Yes)
}

/// Stop and remove the container of a session the fallback has just ended.
///
/// `SIGTERM` first as a courtesy to a CLI that may still be writing, then a
/// forced removal. A container the engine no longer has is the outcome asked
/// for, and a `SIGTERM` that does not land is only a courtesy not taken — the
/// removal below is what the contract is about.
async fn discard(state: &AppState, session_id: Uuid, container_id: &ContainerId) -> Result<()> {
    match state.engine.kill(container_id, Signal::Sigterm).await {
        Ok(()) | Err(EngineError::NotFound(_)) => {}
        Err(error) => warn!(
            session_id = %session_id,
            container_id = %container_id,
            error = %error,
            "could not signal the container of a session the idle reaper ended",
        ),
    }

    match state.engine.remove(container_id, true).await {
        Ok(()) | Err(EngineError::NotFound(_)) => Ok(()),
        Err(error) => Err(error.into()),
    }
}
