//! The orphan cleanup job: leftover containers, `/data/tmp` entries, and
//! hand-off and session refs (`ARCHITECTURE.md`, "Background jobs").
//!
//! Three independent sweeps over three different things, which is why they are
//! three functions and not one loop. Nothing here is a consistency mechanism:
//! every leftover this job collects is something another path was supposed to
//! remove and did not, because the process died between a create and a remove,
//! or a merge's temporary clone outlived the merge. That is what decides the
//! guards. A sweep would rather leave a leftover for the next hour than remove
//! something a live path still holds, so a container is only touched when the
//! session row says there is nothing running, *and* this process is neither
//! launching nor running the session, *and* the container is old enough that
//! no launch can still be on its way to `running`.
//!
//! **Isolation.** The engine, the database and the filesystem all fail per
//! item, and one failure must not cost the rest of the sweep: an item that
//! fails is logged and counted in [`JobReport::failures`], and only the
//! listing call at the top of a sweep — the engine being unreachable, the
//! `/data/tmp` directory being unreadable — is reported as the sweep's own
//! `Err`. A sweep that returns `Err` does not stop the others either: the job
//! logs it, counts one failure and runs the next sweep.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::Path;
use std::time::SystemTime;

use chrono::{DateTime, TimeDelta, Utc};
use uuid::Uuid;

use super::{CronService, JobReport};
use crate::engine::{EngineError, LABEL_SESSION_ID};
use crate::git::{DataPaths, refs};
use crate::models::SessionState;
use crate::prelude::*;
use crate::repositories::{ProjectRepository, SessionRepository, TaskRepository};

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

/// How long a hand-off ref must have been orphaned before it is removed.
///
/// The second half of the two-sighting rule: an orphan first seen less than
/// this ago is only remembered. The sweep runs hourly, so in practice a
/// leftover ref lives for one extra tick — long enough that a publication
/// whose tracker transaction commits after the git lock was released has
/// finished many times over, and short enough that nothing accumulates
/// (`ARCHITECTURE.md`, "Task tracker" → "Code hand-offs").
const ORPHAN_REF_GRACE: TimeDelta = TimeDelta::hours(1);

impl CronService {
    /// Remove what other paths left behind: containers, `/data/tmp` entries,
    /// and hand-off and session refs (`ARCHITECTURE.md`, "Background jobs").
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
            ("refs", self.cleanup_refs(now).await),
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
            if self.state.session_registry.holds_container(session_id) {
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

    /// Remove `refs/handoffs/*` with no matching hand-off row and
    /// `refs/sessions/*` with no matching session row, under each project's
    /// git lock (`ARCHITECTURE.md`, "Background jobs"; "Task tracker" → "Code
    /// hand-offs"; ADR 0049).
    ///
    /// Both namespaces of a project are swept under one acquisition of its
    /// lock, so a project whose repository cannot be read costs one failure,
    /// not one per namespace. The session half is
    /// [`Self::sweep_project_sessions`]; what follows is the hand-off half.
    ///
    /// Two guards, and it takes both to be safe. The first is the project git
    /// lock: publication pins its ref and commits its tracker transaction
    /// under that lock, so a listing taken while holding it can never catch a
    /// publication between the two. The second is the sighting rule — a ref is
    /// removed only when it was found orphaned on an earlier run too, at least
    /// [`ORPHAN_REF_GRACE`] ago — which covers the case the lock does not: a
    /// publication whose database half commits after the lock was released,
    /// and a task deletion interrupted between its rows and its refs, whose
    /// retry this sweep then is.
    ///
    /// The sightings are rebuilt from scratch each run rather than edited:
    /// whatever is not sighted now is forgotten, which drops the refs that
    /// gained a row, the ones just removed, and the entries of projects that
    /// no longer exist, in one assignment. A project whose sweep failed
    /// forgets its sightings with them and simply starts counting again.
    ///
    /// # Errors
    ///
    /// Only the project listing. Everything after it is per project: a
    /// repository that vanished under the sweep, or a git command that failed,
    /// costs one `failures` and the loop continues.
    async fn cleanup_refs(&self, now: DateTime<Utc>) -> Result<JobReport> {
        let project_ids = ProjectRepository::new(&self.state.pool).list_ids().await?;
        let paths = DataPaths::from_config(&self.state.config);

        // A snapshot rather than the live map: the lock must not be held
        // across the git lock, and the new map is written once at the end.
        let previous = self.handoff_sightings.lock().await.clone();
        let mut sighted = HashMap::new();
        let mut report = JobReport::default();

        for project_id in project_ids {
            let mirror = paths.project_repo(project_id);

            // A project that has no repository yet — still cloning, or one
            // whose clone failed — has no refs to sweep and is not a failure.
            if !mirror.exists() {
                continue;
            }

            match self
                .sweep_project_refs(project_id, &mirror, now, &previous, &mut sighted)
                .await
            {
                Ok(swept) => {
                    report.items += swept.items;
                    report.skipped += swept.skipped;
                    report.failures += swept.failures;
                }
                Err(e) => {
                    warn!(project_id = %project_id, error = %e, "could not sweep a project's refs");
                    report.failures += 1;
                }
            }
        }

        *self.handoff_sightings.lock().await = sighted;

        Ok(report)
    }

    /// One project's hand-off and session refs, under its git lock.
    ///
    /// The lock is taken here and released when this returns, so it covers
    /// both namespaces' listings, row reads and removals together and is never
    /// held across another project. The hand-off half runs first; either
    /// half's listing or read failing is the project's one failure.
    async fn sweep_project_refs(
        &self,
        project_id: Uuid,
        mirror: &Path,
        now: DateTime<Utc>,
        previous: &HashMap<(Uuid, Uuid), DateTime<Utc>>,
        sighted: &mut HashMap<(Uuid, Uuid), DateTime<Utc>>,
    ) -> Result<JobReport> {
        let _guard = self.state.git_locks.lock(project_id).await;

        let mut report = self
            .sweep_project_handoffs(project_id, mirror, now, previous, sighted)
            .await?;
        let sessions = self.sweep_project_sessions(project_id, mirror).await?;
        report.items += sessions.items;
        report.failures += sessions.failures;

        Ok(report)
    }

    /// One project's hand-off refs; the caller holds its git lock.
    ///
    /// The lock covers the listing, the `existing_handoff_ids` read and the
    /// removals together. The read is an autocommit query
    /// on the pool: a transaction opened here would be a database lock taken
    /// under the git lock, which is the allowed order but buys nothing
    /// (`ARCHITECTURE.md`, "Git model" → Serialization).
    ///
    /// Removal is decided against the ids this run's query returned and never
    /// against the sighting map alone, so a ref whose row appeared between two
    /// runs is safe even though it is still remembered.
    ///
    /// # Errors
    ///
    /// Listing the refs, which is the repository being gone or unreadable, and
    /// the database read. A removal that fails is counted in
    /// [`JobReport::failures`] and the remaining refs are still considered.
    async fn sweep_project_handoffs(
        &self,
        project_id: Uuid,
        mirror: &Path,
        now: DateTime<Utc>,
        previous: &HashMap<(Uuid, Uuid), DateTime<Utc>>,
        sighted: &mut HashMap<(Uuid, Uuid), DateTime<Utc>>,
    ) -> Result<JobReport> {
        let mut report = JobReport::default();

        let listed = refs::list_handoffs(mirror).await?;
        if listed.is_empty() {
            return Ok(report);
        }

        let ids: Vec<Uuid> = listed.iter().map(|(id, _)| *id).collect();
        let existing: HashSet<Uuid> = TaskRepository::new(&self.state.pool)
            .existing_handoff_ids(project_id, &ids)
            .await?
            .into_iter()
            .collect();

        for id in ids {
            if existing.contains(&id) {
                continue;
            }

            let key = (project_id, id);
            let first_seen = previous.get(&key).copied();

            match first_seen {
                Some(seen) if now - seen >= ORPHAN_REF_GRACE => {
                    match refs::remove_handoff(mirror, id).await {
                        Ok(()) => {
                            info!(project_id = %project_id, handoff_id = %id, "removed orphan hand-off ref");
                            report.items += 1;
                        }
                        Err(e) => {
                            warn!(project_id = %project_id, handoff_id = %id, error = %e, "could not remove an orphan hand-off ref");
                            report.failures += 1;
                            // Keep the sighting: the next run retries without
                            // waiting out the grace again.
                            sighted.insert(key, seen);
                        }
                    }
                }
                Some(seen) => {
                    // Seen before, but not long enough ago.
                    sighted.insert(key, seen);
                    report.skipped += 1;
                }
                None => {
                    sighted.insert(key, now);
                    report.skipped += 1;
                }
            }
        }

        Ok(report)
    }
}

impl CronService {
    /// One project's `refs/sessions/*` with no session row of the project;
    /// the caller holds its git lock.
    ///
    /// No grace and no sightings, unlike the hand-off half, because the order
    /// of writes is the other way round: a session's row is committed before
    /// its launch starts and a ref is only ever written by a sync of a session
    /// that exists, so a ref with no row is always one whose session was
    /// deleted — before session deletion removed the ref (ADR 0049), after a
    /// ref removal whose row delete then failed, or by a sync that read the
    /// row before a deletion and wrote after it. The read is the project's
    /// session ids on a pool connection, not a transaction, for the reason
    /// the hand-off half gives.
    ///
    /// # Errors
    ///
    /// Listing the refs and the database read. A removal that fails is
    /// counted in [`JobReport::failures`] and the rest are still removed.
    async fn sweep_project_sessions(&self, project_id: Uuid, mirror: &Path) -> Result<JobReport> {
        let mut report = JobReport::default();

        let listed = refs::list_sessions(mirror).await?;
        if listed.is_empty() {
            return Ok(report);
        }

        let mut conn = self.state.pool.acquire().await?;
        let existing: HashSet<Uuid> = SessionRepository::new(&self.state.pool)
            .ids_for_project(&mut conn, project_id)
            .await?
            .into_iter()
            .collect();
        drop(conn);

        for id in listed {
            if existing.contains(&id) {
                continue;
            }

            match refs::delete(mirror, &refs::session_ref(id)).await {
                Ok(()) => {
                    info!(project_id = %project_id, session_id = %id, "removed orphan session ref");
                    report.items += 1;
                }
                Err(e) => {
                    warn!(project_id = %project_id, session_id = %id, error = %e, "could not remove an orphan session ref");
                    report.failures += 1;
                }
            }
        }

        Ok(report)
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
