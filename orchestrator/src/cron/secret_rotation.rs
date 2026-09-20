//! The hourly secret rotation job (`ARCHITECTURE.md`, "Background jobs", the
//! `secret rotation` row, and "Secrets" → Rotation).
//!
//! The sweep itself is [`rewrap_outdated`](crate::secrets::rewrap_outdated):
//! it selects the rows whose `key_version` is behind the newest configured
//! master key in batches of a hundred and re-wraps each one's data key under
//! the newest key. This job adds nothing to it. It is the hour: the same
//! routine `mars-orchestrator rotate-secrets` runs on demand runs here on a
//! tick, so an operator who adds a key and restarts gets the table moved onto
//! it without running anything at all.
//!
//! The two things this file does own are the translation of the sweep's
//! [`RotationReport`](crate::secrets::RotationReport) into the common
//! [`JobReport`] — `rewrapped` is the work done, `skipped` is the rows
//! deliberately passed over, and there is no third outcome, so `failures` is
//! zero and a sweep that fails as a whole returns `Err` instead — and one
//! `warn!` when rows are still behind the newest key after the sweep. That
//! second one is the operator's signal that the rotation cannot finish: the
//! remaining rows are under a version the keyring does not carry, so the old
//! key has to be put back before the old wrapping can be retired. The sweep has
//! already named those versions at `error!` once each, so the line here counts
//! rather than repeats, and nothing in it is key material (`CLAUDE.md`, rule
//! 3).
//!
//! Running while the API serves, and while a `rotate-secrets` invocation
//! sweeps, is safe by construction rather than by a lock: every row update is
//! guarded by `WHERE key_version = <the version the batch read>`, so a row a
//! `PUT /api/secrets/{id}` replaced in between is skipped rather than
//! overwritten.

use chrono::{DateTime, Utc};

use crate::cron::{CronService, JobReport};
use crate::prelude::*;
use crate::secrets::rewrap_outdated;

impl CronService {
    /// Re-wrap every secret whose data key is under an older master key.
    ///
    /// Returns zeros immediately when the keyring holds a single version,
    /// which is the usual case and the reason there is no pre-check here: the
    /// sweep's own first batch is empty, and the scheduler logs the empty
    /// report at `debug`.
    ///
    /// An error propagates: the scheduler logs it and the next hour tries
    /// again, over whatever the failed sweep had already committed, because
    /// the sweep commits row by row and never holds a transaction open.
    pub async fn secret_rotation(&self, now: DateTime<Utc>) -> Result<JobReport> {
        // Every job takes its clock from the caller; this one has no deadline
        // to measure against it, and deliberately does not thread it into the
        // sweep.
        let _ = now;

        let report = rewrap_outdated(&self.state.pool, &self.state.keyring).await?;

        if report.remaining > 0 {
            warn!(
                remaining = report.remaining,
                newest_version = self.state.keyring.current_version(),
                "secrets remain under a master key version the keyring cannot unwrap"
            );
        }

        Ok(JobReport {
            items: report.rewrapped,
            skipped: report.skipped,
            failures: 0,
        })
    }
}
