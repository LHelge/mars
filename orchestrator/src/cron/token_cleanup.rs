//! The hourly token cleanup (`ARCHITECTURE.md`, "Background jobs": "Delete
//! expired refresh tokens, reset tokens, unaccepted invites, and secrets whose
//! scope row no longer exists").
//!
//! Four independent `DELETE` statements, each its own autocommit statement
//! rather than one transaction, because they have nothing to do with each
//! other: a failure of one must not roll back the rows another has already
//! reclaimed, and the next tick is an hour away. That is also why a failing
//! step is counted and logged rather than returned — the sweep goes on, and
//! only a run in which all four failed is a failed run.
//!
//! No row locks (`docs/data-model.md`, "Users and authentication" puts the
//! user-row lock on *issuing and consuming* tokens, not on deleting expired
//! ones): an expired row has nothing left to race with, and a refresh that
//! loses the race against the deletion of its own expired token fails exactly
//! as it would have failed on the expiry itself.
//!
//! Nothing here logs a token, a hash, a secret name or a value (`CLAUDE.md`,
//! rule 3): the lines below carry counts and the step that produced them.

use chrono::{DateTime, Utc};

use crate::cron::{CronService, JobReport};
use crate::prelude::*;
use crate::repositories::{
    PasswordResetTokenRepository, RefreshTokenRepository, SecretRepository, UserInviteRepository,
};

impl CronService {
    /// Delete expired credentials and orphaned secret rows.
    ///
    /// `now` is the cut-off for the three expiry sweeps, taken from the caller
    /// so a test can place a row on either side of it. The orphan sweep has no
    /// cut-off: a scope row is either there or it is not.
    ///
    /// `items` is the total number of rows deleted across the four steps, and
    /// `failures` the number of steps that raised. `Err` only when every step
    /// failed, which means the database is unreachable rather than that one
    /// statement is wrong.
    pub async fn token_cleanup(&self, now: DateTime<Utc>) -> Result<JobReport> {
        let pool = &self.state.pool;
        let mut report = JobReport::default();

        let refresh_tokens = self
            .step(
                "refresh_tokens",
                &mut report,
                RefreshTokenRepository::new(pool).delete_expired(now),
            )
            .await;

        let reset_tokens = self
            .step(
                "password_reset_tokens",
                &mut report,
                PasswordResetTokenRepository::new(pool).delete_expired(now),
            )
            .await;

        let invites = self
            .step(
                "user_invites",
                &mut report,
                UserInviteRepository::new(pool).delete_expired(now),
            )
            .await;

        let orphan_secrets = self
            .step(
                "orphan_secrets",
                &mut report,
                SecretRepository::new(pool).delete_orphans(),
            )
            .await;

        if report.failures == STEPS {
            return Err(Error::Internal(
                "token cleanup: every step failed".to_string(),
            ));
        }

        if report.items > 0 {
            info!(
                refresh_tokens,
                reset_tokens, invites, orphan_secrets, "token cleanup deleted rows"
            );
        }

        Ok(report)
    }

    /// Run one step, folding its outcome into `report` and returning the rows
    /// it deleted (`0` when it failed).
    ///
    /// The error is logged here, with the step name, and then dropped: the
    /// caller has three more statements to run and a `JobReport` has no room
    /// for a message. `step` is the field an operator greps for.
    async fn step(
        &self,
        name: &'static str,
        report: &mut JobReport,
        deletion: impl Future<Output = Result<u64>>,
    ) -> u64 {
        match deletion.await {
            Ok(deleted) => {
                report.items += deleted;
                deleted
            }
            Err(e) => {
                error!(step = name, error = %e, "token cleanup step failed");
                report.failures += 1;
                0
            }
        }
    }
}

/// How many steps one run has, so that "all of them failed" is written once.
const STEPS: u64 = 4;
