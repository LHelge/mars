//! The mirror fetch job: `git fetch --prune` on every `ready` mirror
//! (`ARCHITECTURE.md`, "Background jobs"; "Git model", Project clone).
//!
//! The whole body is a loop around [`git::fetch_project`], which is the one
//! routine the cron job, `POST /projects/{id}/fetch` and a fresh session
//! launch all go through: it takes the project git lock, asks the credential
//! provider for the upstream credential, runs the fetch and records
//! `last_fetched_at` in a short transaction *after* the lock is released
//! (ADR 0021). Doing it that way is what gives this job its two documented
//! properties for free — no database transaction is ever open while git runs,
//! and nothing under `refs/heads/*`, `refs/sessions/*` or `refs/handoffs/*` is
//! ever written (ADR 0017).
//!
//! Sequential, not concurrent. Each fetch takes that project's git lock and
//! costs one credential lookup, so fetching many projects at once would only
//! hammer the upstream hosts and the secrets service; the interval is ten
//! minutes and the work is not urgent.

use std::time::Instant;

use chrono::{DateTime, Utc};

use super::{CronService, JobReport};
use crate::git::{self, GitActor};
use crate::models::ProjectStatus;
use crate::prelude::*;
use crate::repositories::ProjectRepository;

impl CronService {
    /// Fetch every `ready` project's upstream, one after another.
    ///
    /// `now` is taken for signature uniformity with the other jobs and used as
    /// nothing but the logged timestamp: freshness is not this job's decision.
    /// It passes `max_age = None`, which means the cron job always fetches —
    /// the interval *is* the freshness policy (`ARCHITECTURE.md`, "Git model",
    /// Project clone).
    ///
    /// One unreachable upstream must not cost the others their fetch, so every
    /// per-project outcome is counted rather than returned:
    ///
    /// - a fetch that ran counts as `items`;
    /// - [`Error::Conflict`] (a retry-clone moved the project out of `ready`
    ///   between the listing and the fetch) and [`Error::NotFound`] (it was
    ///   deleted in that window) count as `skipped`, because neither is a
    ///   fault — the next tick simply will not list the project;
    /// - anything else counts as `failures` and is logged at `warn` with the
    ///   project id, never with any part of the credential (rule 3).
    ///
    /// # Errors
    ///
    /// Only [`Error::Database`], from listing the projects: at that point
    /// nothing has been attempted, so there is no partial sweep to report.
    pub async fn mirror_fetch(&self, now: DateTime<Utc>) -> Result<JobReport> {
        let project_ids = ProjectRepository::new(&self.state.pool)
            .list_ids_by_status(ProjectStatus::Ready)
            .await?;

        debug!(
            now = %now,
            projects = project_ids.len(),
            "fetching every ready mirror"
        );

        let mut report = JobReport::default();

        for project_id in project_ids {
            let started = Instant::now();

            match git::fetch_project(&self.state, project_id, &GitActor::System, None).await {
                Ok(_) => {
                    report.items += 1;
                    debug!(
                        project_id = %project_id,
                        elapsed_ms = started.elapsed().as_millis(),
                        "mirror fetched"
                    );
                }
                Err(Error::Conflict(_)) | Err(Error::NotFound) => {
                    report.skipped += 1;
                    debug!(
                        project_id = %project_id,
                        "mirror fetch skipped: the project is no longer ready"
                    );
                }
                Err(e) => {
                    report.failures += 1;
                    warn!(project_id = %project_id, error = %e, "mirror fetch failed");
                }
            }
        }

        Ok(report)
    }
}
