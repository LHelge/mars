//! Periodic jobs: one `CronService` with a named method per job and one
//! independent interval each (`ARCHITECTURE.md`, "Background jobs").
//!
//! The service holds nothing but the [`AppState`], so a job is an ordinary
//! `async fn` on it that a test can call directly with the `now` it wants.
//! [`scheduler::spawn_job`] is the only thing that knows about time: it owns
//! the tick, the panic and error isolation and the outcome logging, which is
//! why a job body never has to think about any of them.
//!
//! The bodies land in their own files as further `impl CronService` blocks,
//! one per job, each added by its own task in this epic:
//!
//! - `cron/mirror_fetch.rs` — `git fetch --prune` on every `ready` mirror;
//! - `cron/idle_reaper.rs` — delegating to `session/idle_reaper.rs`;
//! - `cron/stuck_tasks.rs` — release tasks held by ended sessions;
//! - `cron/token_cleanup.rs` — expired credentials and orphaned secret rows;
//! - `cron/secret_rotation.rs` — re-wrap rows behind the newest master key;
//! - `cron/orphan_cleanup.rs` — leftover containers, `/data/tmp` and refs.
//!
//! Until then each method below is the signature and nothing else. The `now`
//! parameter is named rather than dropped because it is part of that
//! signature: every job takes its clock from the caller so a test can place
//! one side of a timeout on either side of `now`.

use std::fmt;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::prelude::*;

pub mod scheduler;
mod stuck_tasks;

pub use scheduler::spawn_job;

/// How often the two reapers run (`ARCHITECTURE.md`, "Background jobs").
const REAPER_PERIOD: Duration = Duration::from_secs(60);

/// How often the hourly jobs run: token cleanup, secret rotation and orphan
/// cleanup (`ARCHITECTURE.md`, "Background jobs").
const HOURLY_PERIOD: Duration = Duration::from_secs(3600);

/// The periodic jobs, one variant per row of the table in `ARCHITECTURE.md`,
/// "Background jobs".
///
/// The name is what the scheduler logs as `job` and what
/// [`CronService::run_once`] dispatches on, so the `Display` spelling is a
/// contract an operator greps for and not a label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobName {
    MirrorFetch,
    IdleReaper,
    StuckTaskReaper,
    TokenCleanup,
    SecretRotation,
    OrphanCleanup,
}

impl JobName {
    /// Every job, in the table's order. [`CronService::start`] spawns one loop
    /// per entry, so a variant added here is a variant that runs.
    pub const ALL: [JobName; 6] = [
        JobName::MirrorFetch,
        JobName::IdleReaper,
        JobName::StuckTaskReaper,
        JobName::TokenCleanup,
        JobName::SecretRotation,
        JobName::OrphanCleanup,
    ];

    /// The logged name, `'static` because the scheduler holds it for the life
    /// of the loop rather than formatting it per tick.
    pub fn as_str(&self) -> &'static str {
        match self {
            JobName::MirrorFetch => "mirror_fetch",
            JobName::IdleReaper => "idle_reaper",
            JobName::StuckTaskReaper => "stuck_task_reaper",
            JobName::TokenCleanup => "token_cleanup",
            JobName::SecretRotation => "secret_rotation",
            JobName::OrphanCleanup => "orphan_cleanup",
        }
    }

    /// This job's interval (`ARCHITECTURE.md`, "Background jobs").
    ///
    /// Only the mirror fetch is configurable, because only it is about a
    /// remote an operator may want to be gentler with
    /// (`MIRROR_FETCH_INTERVAL_SECS`, `README.md`, "Configuration").
    pub fn period(&self, config: &Config) -> Duration {
        match self {
            JobName::MirrorFetch => Duration::from_secs(config.mirror_fetch_interval_secs),
            JobName::IdleReaper | JobName::StuckTaskReaper => REAPER_PERIOD,
            JobName::TokenCleanup | JobName::SecretRotation | JobName::OrphanCleanup => {
                HOURLY_PERIOD
            }
        }
    }
}

impl fmt::Display for JobName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What one run of a job did.
///
/// The same three counters for every job, because the scheduler logs them and
/// an operator reads them the same way whichever job produced them: `items` is
/// work done, `skipped` is work deliberately passed over — a mirror that is
/// not `ready`, a secret whose master key is not configured — and `failures`
/// is work that was attempted and did not succeed. A job that fails as a whole
/// returns `Err` instead; per-item failures are counted here, because one bad
/// row must not stop the sweep.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct JobReport {
    pub items: u64,
    pub skipped: u64,
    pub failures: u64,
}

/// The periodic jobs, over the state the rest of the process serves from.
///
/// Held behind an `Arc` by [`CronService::start`] and by nothing else: the
/// jobs take `&self`, so a test calls one directly on a plain value
/// (`TestApp::cron`) without a scheduler anywhere near it.
pub struct CronService {
    state: AppState,
}

impl CronService {
    pub fn new(state: AppState) -> Self {
        CronService { state }
    }

    /// Run one job now, with the clock the caller chose.
    ///
    /// The dispatch the scheduler calls, and the one an integration test calls
    /// to run a job once without waiting for a tick.
    pub async fn run_once(&self, job: JobName, now: DateTime<Utc>) -> Result<JobReport> {
        match job {
            JobName::MirrorFetch => self.mirror_fetch(now).await,
            JobName::IdleReaper => self.idle_reaper(now).await,
            JobName::StuckTaskReaper => self.stuck_task_reaper(now).await,
            JobName::TokenCleanup => self.token_cleanup(now).await,
            JobName::SecretRotation => self.secret_rotation(now).await,
            JobName::OrphanCleanup => self.orphan_cleanup(now).await,
        }
    }

    /// Start one loop per job and return their handles.
    ///
    /// Called once, from `main` after recovery and before either listener
    /// accepts a request (`ARCHITECTURE.md`, "Restart procedure", step 4).
    /// Each loop stops when `shutdown` carries `true`, and the caller awaits
    /// the handles within the drain grace.
    pub fn start(self: Arc<Self>, shutdown: watch::Receiver<bool>) -> Vec<JoinHandle<()>> {
        JobName::ALL
            .into_iter()
            .map(|job| {
                let service = Arc::clone(&self);
                spawn_job(
                    job.as_str(),
                    job.period(&self.state.config),
                    shutdown.clone(),
                    move || {
                        let service = Arc::clone(&service);
                        // `Utc::now()` per tick, not per loop: a job's `now` is
                        // the moment it runs, which is what every timeout in a
                        // job body is measured against.
                        async move { service.run_once(job, Utc::now()).await }
                    },
                )
            })
            .collect()
    }

    /// `git fetch --prune` on every `ready` mirror. Implemented by the mirror
    /// fetch task in this epic; the body lives in `cron/mirror_fetch.rs`.
    pub async fn mirror_fetch(&self, now: DateTime<Utc>) -> Result<JobReport> {
        let _ = now;
        Ok(JobReport::default())
    }

    /// Park idle conversational sessions and fail idle ephemeral ones.
    /// Implemented by the idle reaper task in this epic; the body lives in
    /// `cron/idle_reaper.rs` and delegates to `session/idle_reaper.rs`.
    pub async fn idle_reaper(&self, now: DateTime<Utc>) -> Result<JobReport> {
        let _ = now;
        Ok(JobReport::default())
    }

    /// Delete expired credentials and orphaned secret rows. Implemented by the
    /// token cleanup task in this epic; the body lives in
    /// `cron/token_cleanup.rs`.
    pub async fn token_cleanup(&self, now: DateTime<Utc>) -> Result<JobReport> {
        let _ = now;
        Ok(JobReport::default())
    }

    /// Re-wrap rows whose `key_version` is behind the newest master key.
    /// Implemented by the secret rotation task in this epic; the body lives in
    /// `cron/secret_rotation.rs`.
    pub async fn secret_rotation(&self, now: DateTime<Utc>) -> Result<JobReport> {
        let _ = now;
        Ok(JobReport::default())
    }

    /// Remove leftover containers, `/data/tmp` directories and hand-off refs.
    /// Implemented by the orphan cleanup task in this epic; the body lives in
    /// `cron/orphan_cleanup.rs`.
    pub async fn orphan_cleanup(&self, now: DateTime<Utc>) -> Result<JobReport> {
        let _ = now;
        Ok(JobReport::default())
    }
}
