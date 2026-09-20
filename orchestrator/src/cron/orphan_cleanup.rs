//! The orphan cleanup job: leftover containers, `/data/tmp` entries and
//! hand-off refs (`ARCHITECTURE.md`, "Background jobs").
//!
//! Three independent sweeps over three different things, which is why they are
//! three functions and not one loop. Nothing here is a consistency mechanism:
//! every leftover this job collects is something another path was supposed to
//! remove and did not, because the process died between a create and a remove,
//! or a merge's temporary clone outlived the merge. That is what decides the
//! guards. A sweep would rather leave a leftover for the next hour than remove
//! something a live path still holds, so a container is only touched when the
//! session row says there is nothing running, *and* this process has no entry
//! for the session, *and* the container is old enough that no launch can still
//! be on its way to `running`.
//!
//! **Isolation.** The engine, the database and the filesystem all fail per
//! item, and one failure must not cost the rest of the sweep: an item that
//! fails is logged and counted in [`JobReport::failures`], and only the
//! listing call at the top of a sweep — the engine being unreachable, the
//! `/data/tmp` directory being unreadable — is reported as the sweep's own
//! `Err`. A sweep that returns `Err` does not stop the others either: the job
//! logs it, counts one failure and runs the next sweep.

use std::fs;
use std::io;
use std::path::Path;
use std::time::SystemTime;

use chrono::{DateTime, TimeDelta, Utc};
use uuid::Uuid;

use super::{CronService, JobReport};
use crate::engine::{EngineError, LABEL_SESSION_ID};
use crate::git::DataPaths;
use crate::models::SessionState;
use crate::prelude::*;
use crate::repositories::SessionRepository;

/// How young a container is protected whatever its session row says.
///
/// The launch window: the launcher registers the owner, creates the container
/// and only then reaches `running`, and a resume of a parked session does the
/// same while the row still says `parked`. The registry entry covers that
/// window inside this process; this covers the rest — a container created by
/// an orchestrator that has since restarted, and any ordering the registry
/// does not see (`ARCHITECTURE.md`, "Background jobs").
const CONTAINER_GRACE: TimeDelta = TimeDelta::minutes(5);

/// How long an entry under `DATA_DIR/tmp` is left alone.
///
/// A temporary clone is used under its project's git lock, which this job does
/// not take, so age is the only safe evidence that nothing is using it. An
/// hour is far longer than any merge, rebase or startup probe
/// (`ARCHITECTURE.md`, "Storage", "Git model").
const TMP_MAX_AGE: TimeDelta = TimeDelta::hours(1);

impl CronService {
    /// Remove what other paths left behind: containers, `/data/tmp` entries
    /// and hand-off refs (`ARCHITECTURE.md`, "Background jobs").
    ///
    /// The three sweeps are independent and all three run: a sweep that fails
    /// as a whole is logged with its name and costs one failure, and the job
    /// carries on. The reports are summed, so `items` is everything removed
    /// this run whichever sweep removed it.
    pub async fn orphan_cleanup(&self, now: DateTime<Utc>) -> Result<JobReport> {
        let mut report = JobReport::default();

        for (sweep, outcome) in [
            ("containers", self.cleanup_containers(now).await),
            ("tmp", self.cleanup_tmp(now).await),
            ("handoff_refs", self.cleanup_handoff_refs(now).await),
        ] {
            match outcome {
                Ok(swept) => {
                    report.items += swept.items;
                    report.skipped += swept.skipped;
                    report.failures += swept.failures;
                }
                Err(e) => {
                    error!(sweep = sweep, error = %e, "an orphan cleanup sweep failed");
                    report.failures += 1;
                }
            }
        }

        Ok(report)
    }

    /// Remove containers labelled `mars.session_id` whose session has nothing
    /// running.
    ///
    /// One listing, then a decision per container. A container is removed when
    /// its session row is missing or says `parked`, `done` or `failed` — and
    /// none of the guards holds. `creating` and `running` sessions have a
    /// container by definition and are never touched; a container a running
    /// session no longer owns is recovery's, not this job's
    /// (`ARCHITECTURE.md`, "Restart procedure").
    ///
    /// # Errors
    ///
    /// The listing failing, which is the engine being unreachable. Everything
    /// after it is per container.
    async fn cleanup_containers(&self, now: DateTime<Utc>) -> Result<JobReport> {
        let containers = self.state.engine.list_by_label(LABEL_SESSION_ID).await?;
        let sessions = SessionRepository::new(&self.state.pool);
        let mut report = JobReport::default();

        for summary in containers {
            let label = summary
                .labels
                .get(LABEL_SESSION_ID)
                .map(String::as_str)
                .unwrap_or_default();

            let Ok(session_id) = Uuid::parse_str(label) else {
                // Not ours to reason about: something else is using the key.
                warn!(
                    container_id = %summary.id,
                    "a container carries a mars.session_id that is not a session id",
                );
                report.skipped += 1;
                continue;
            };

            let session = match sessions.find(session_id).await {
                Ok(session) => session,
                Err(e) => {
                    error!(session_id = %session_id, error = %e, "could not read the session of a labelled container");
                    report.failures += 1;
                    continue;
                }
            };

            // A session that is `creating` or `running` owns its containers.
            if session
                .as_ref()
                .is_some_and(|session| !is_ended(session.state))
            {
                report.skipped += 1;
                continue;
            }

            // A relaunch between the create and `running`: the row still says
            // `parked` and the owner is already registered.
            if self.state.session_registry.has(session_id) {
                report.skipped += 1;
                continue;
            }

            if now.signed_duration_since(summary.created) < CONTAINER_GRACE {
                report.skipped += 1;
                continue;
            }

            match self.state.engine.remove(&summary.id, true).await {
                // A container that is already gone is already removed
                // (`ARCHITECTURE.md`, "Engine adapter"); the explicit arm is
                // here because a race with a stop is not a failure.
                Ok(()) | Err(EngineError::NotFound(_)) => {
                    info!(
                        session_id = %session_id,
                        container_id = %summary.id,
                        "removed orphan container",
                    );
                    report.items += 1;

                    let is_current = session
                        .as_ref()
                        .and_then(|session| session.container_id.as_deref())
                        == Some(summary.id.0.as_str());

                    if is_current && !self.clear_container_id(session_id).await {
                        report.failures += 1;
                    }
                }
                Err(e) => {
                    warn!(
                        session_id = %session_id,
                        container_id = %summary.id,
                        error = %e,
                        "could not remove an orphan container",
                    );
                    report.failures += 1;
                }
            }
        }

        Ok(report)
    }

    /// Clear `sessions.container_id` after removing the container it names
    /// (`docs/data-model.md`, `sessions`), reporting whether it worked.
    ///
    /// Its own transaction: the container is already gone, so a failure here
    /// leaves a row pointing at nothing, which the next launch overwrites.
    async fn clear_container_id(&self, session_id: Uuid) -> bool {
        let cleared = async {
            let mut tx = self.state.pool.begin().await?;
            SessionRepository::new(&self.state.pool)
                .set_container_id(&mut tx, session_id, None)
                .await?;
            tx.commit().await?;
            Ok::<(), Error>(())
        }
        .await;

        match cleared {
            Ok(()) => true,
            Err(e) => {
                error!(session_id = %session_id, error = %e, "could not clear the container id of an orphan container");
                false
            }
        }
    }

    /// Delete everything under `DATA_DIR/tmp` older than an hour: the
    /// temporary clones merge and rebase make, the temporary credential
    /// configs beside them, and the startup probe's `probe-<random>`
    /// directories (`ARCHITECTURE.md`, "Storage").
    ///
    /// Age is the only guard, because a live temporary clone is protected by
    /// its project's git lock and this sweep takes no lock: anything younger
    /// than an hour may be in use right now and is left alone.
    ///
    /// # Errors
    ///
    /// Reading the directory failing for a reason other than its not being
    /// there. A `tmp` that does not exist is nothing to do: no merge, rebase
    /// or probe has run yet.
    async fn cleanup_tmp(&self, now: DateTime<Utc>) -> Result<JobReport> {
        let tmp = DataPaths::from_config(&self.state.config).tmp();
        let cutoff = SystemTime::from(now - TMP_MAX_AGE);
        let mut report = JobReport::default();

        // Blocking filesystem calls on the runtime: an hourly sweep of one
        // directory that holds a handful of entries at most, and the removals
        // are the same `std::fs` calls the startup probe and the temporary
        // clones make.
        let entries = match fs::read_dir(&tmp) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(report),
            Err(e) => {
                return Err(Error::Internal(format!(
                    "could not read {}: {e}",
                    tmp.display()
                )));
            }
        };

        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(e) => {
                    warn!(path = %tmp.display(), error = %e, "could not read an entry of the temporary directory");
                    report.failures += 1;
                    continue;
                }
            };
            let path = entry.path();

            // `DirEntry::metadata` does not follow a symlink, so a link into
            // the data directory is judged and removed as the link it is and
            // never as what it points at.
            let stat = entry
                .metadata()
                .and_then(|metadata| Ok((metadata.is_dir(), metadata.modified()?)));

            let (is_dir, modified) = match stat {
                Ok(stat) => stat,
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "could not stat a temporary directory entry");
                    report.failures += 1;
                    continue;
                }
            };

            if modified >= cutoff {
                report.skipped += 1;
                continue;
            }

            match remove(&path, is_dir) {
                Ok(()) => {
                    info!(path = %path.display(), "removed a stale temporary directory entry");
                    report.items += 1;
                }
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "could not remove a stale temporary directory entry");
                    report.failures += 1;
                }
            }
        }

        Ok(report)
    }

    /// Remove `refs/handoffs/*` with no matching hand-off row, under each
    /// project's git lock (`ARCHITECTURE.md`, "Git model", "Merge, rebase,
    /// push"). Implemented by the hand-off ref task in this epic.
    async fn cleanup_handoff_refs(&self, now: DateTime<Utc>) -> Result<JobReport> {
        let _ = now;
        Ok(JobReport::default())
    }
}

/// Whether a session in this state has no container of its own
/// (`ARCHITECTURE.md`, "Session lifecycle").
fn is_ended(state: SessionState) -> bool {
    match state {
        SessionState::Parked | SessionState::Done | SessionState::Failed => true,
        SessionState::Creating | SessionState::Running => false,
    }
}

/// Remove one `DATA_DIR/tmp` entry: the whole tree for a directory, the entry
/// itself for a file or a symlink, which is never followed.
fn remove(path: &Path, is_dir: bool) -> io::Result<()> {
    if is_dir {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}
