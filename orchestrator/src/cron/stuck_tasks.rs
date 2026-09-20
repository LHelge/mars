//! The stuck-task reaper: leases whose holder is gone.
//!
//! `ARCHITECTURE.md`, "Background jobs" (`stuck_task_reaper`, every minute)
//! and "Task tracker" → "Liveness comes from the session, not from tool
//! calls": a lease has no TTL and is valid exactly as long as its holder is
//! alive, so a lease held by a `done` or `failed` session is a task nobody is
//! working on. Every minute this finds those and gives them back.
//!
//! **It is the backstop, not the mechanism.** `AppState::on_session_ended`
//! already releases a session's leases the moment it dies
//! (`tracker::hooks`), and every ordinary ending goes through it. This sweep
//! is what catches the endings that did not: a process killed between the
//! session transition and the hook, a tracker failure the hook logged and
//! swallowed, a row a recovery pass moved by hand. On a healthy system it
//! reports nothing, run after run.
//!
//! **One transaction per dead session, never one for the sweep.** Each
//! release is [`release_leases_for_session`]'s own project-locked mutation,
//! so a backlog of a thousand stuck tasks spread over twenty projects never
//! holds one project's lock while another's rows are written, and a project
//! whose release fails does not stop the rest (`ARCHITECTURE.md`, "Task
//! tracker" → "One mutation at a time per project").
//!
//! **The reaper decides nothing about escalation, comments or email.** What a
//! release does to a task at `max_attempts`, what it writes in the thread and
//! who is told about it is the tracker's, so that the hook path and this path
//! cannot drift apart: this module only chooses the reason, calls the
//! primitive and counts. The emails the commit made due are sent afterwards,
//! outside the transaction, by the same `tracker::escalation::notify` every
//! other caller uses.

use chrono::{DateTime, Utc};

use crate::cron::{CronService, JobReport};
use crate::models::SessionState;
use crate::prelude::*;
use crate::repositories::TaskRepository;
use crate::repositories::tasks::DeadHolder;
use crate::tracker::escalation;
use crate::tracker::leases::{ReleaseReason, release_leases_for_session};

/// The `sessions.error` the idle reaper writes, and the one value that makes a
/// release [`ReleaseReason::Stalled`] rather than
/// [`ReleaseReason::SessionEnded`] (`SPEC.md`, "TaskEvent").
const STALLED: &str = "stalled";

impl CronService {
    /// Release every lease held by a session that is `done` or `failed`.
    ///
    /// `now` is unused: this job has no timeout to measure. A lease is stuck
    /// the instant its holder is dead, not after a grace period — the holder
    /// is not coming back, and a task left in a queue with a lease on it is
    /// invisible to `ready` for as long as the lease stands. The parameter is
    /// part of every job's signature (`cron::CronService::run_once`).
    ///
    /// The counters: `items` is the tasks the sweep set out to release,
    /// `skipped` is a holder whose row was deleted between the listing and the
    /// release — the `ON DELETE SET NULL` on the lease has then already freed
    /// its tasks — and `failures` is a release that errored, which is logged
    /// and stepped over so that one project cannot stop the sweep.
    pub async fn stuck_task_reaper(&self, now: DateTime<Utc>) -> Result<JobReport> {
        let _ = now;

        // Lock-free and outside any transaction: the locked re-read inside the
        // primitive is what decides, so a lease released, taken over or
        // escalated since this listing is simply not released twice.
        let holders = TaskRepository::new(&self.state.pool)
            .list_dead_lease_holders()
            .await?;

        let mut report = JobReport::default();

        for holder in holders {
            let session_id = holder.session_id;
            let project_id = holder.project_id;

            match release_leases_for_session(&self.state.pool, session_id, reason(&holder)).await {
                Ok(escalations) => {
                    report.items += holder.held as u64;
                    // After the commit, never inside it: the mutation recorded
                    // what it owes and this sends it (`tracker::escalation`).
                    escalation::notify(&self.state, escalations).await;
                }
                Err(Error::NotFound) => {
                    debug!(
                        session_id = %session_id,
                        project_id = %project_id,
                        "the holder was deleted before its leases were released",
                    );
                    report.skipped += 1;
                }
                Err(error) => {
                    error!(
                        session_id = %session_id,
                        project_id = %project_id,
                        error = %error,
                        "lease release failed",
                    );
                    report.failures += 1;
                }
            }
        }

        if report.items > 0 {
            info!(
                items = report.items,
                failures = report.failures,
                "stuck leases released",
            );
        }

        Ok(report)
    }
}

/// Why this holder's leases are being released.
///
/// The same reading the session hook does from the row it finds: a `failed`
/// session whose `error` is `stalled` is the idle reaper's, and everything
/// else — an ended conversation, a finished ephemeral session, a failed launch
/// — is [`ReleaseReason::SessionEnded`] (`ARCHITECTURE.md`, "Task tracker" →
/// "Liveness comes from the session, not from tool calls").
fn reason(holder: &DeadHolder) -> ReleaseReason {
    if holder.session_state == SessionState::Failed
        && holder.session_error.as_deref() == Some(STALLED)
    {
        ReleaseReason::Stalled
    } else {
        ReleaseReason::SessionEnded
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn holder(state: SessionState, error: Option<&str>) -> DeadHolder {
        DeadHolder {
            project_id: Uuid::nil(),
            session_id: Uuid::nil(),
            session_state: state,
            session_error: error.map(str::to_string),
            held: 1,
        }
    }

    #[test]
    fn only_a_failed_session_with_the_stalled_error_stalled() {
        assert_eq!(
            reason(&holder(SessionState::Failed, Some("stalled"))),
            ReleaseReason::Stalled,
        );
        assert_eq!(
            reason(&holder(SessionState::Failed, Some("image pull failed"))),
            ReleaseReason::SessionEnded,
        );
        assert_eq!(
            reason(&holder(SessionState::Failed, None)),
            ReleaseReason::SessionEnded,
        );
        assert_eq!(
            reason(&holder(SessionState::Done, None)),
            ReleaseReason::SessionEnded,
        );
    }
}
