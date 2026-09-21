//! The dispatcher: putting an agent on claimable work without being asked.
//!
//! `ARCHITECTURE.md`, "Dispatcher" and "Background jobs". Each run walks the
//! `ready` projects that are not paused, and within each the `auto_launch`
//! profiles in `created_at` order; for every one it launches an ephemeral
//! session on the best claimable task of the `queue` states that profile
//! serves, for as long as the capacity rules of "Unattended launches" allow.
//!
//! **It composes and decides almost nothing.** Every rule it applies already
//! exists, and this job is the order they are asked in:
//!
//! - which task — [`ready_summaries`], the very query behind the MCP `ready`
//!   tool, so the task dispatched is the first row that tool would have offered
//!   the same profile (priority, then number);
//! - may it launch — [`unattended_capacity`], the pause and the three caps
//!   (ADR 0042), re-asked before *every* launch because each launch it makes
//!   changes the counts the next one is measured against;
//! - could it authenticate — [`has_unattended_credential`], the backstop under
//!   the same check the profile was saved through, because a credential can be
//!   deleted afterwards;
//! - the launch itself — [`create_session`] with [`LaunchActor::Dispatcher`]
//!   and `require_served_state`, which is the user launch path with no user:
//!   the same claim transaction, the same generated task message, the same
//!   hand-off base and the same release when the launch fails in `creating`.
//!
//! **The claim decides, not the listing.** [`ready_summaries`] is a lock-free
//! pool read and is stale the moment it returns: an agent claiming over MCP, a
//! person pressing "run once" or the previous profile of this very run can take
//! a listed task first. That race has one answer and it is inside the launch —
//! the claim and the session insert are one project-locked transaction, so a
//! lost claim is 409 `task is not claimable` and leaves no session row behind
//! (`ARCHITECTURE.md`, "Task tracker" → "Launching a session for a task").
//! Here it is a `debug` line and the next candidate, never an error: losing a
//! race to somebody who is already doing the work is the system behaving.
//!
//! **No lock is held across a launch, and none is taken here at all.** The
//! candidate reads are pool reads, and each launch takes the project git lock
//! and then the project row lock inside [`create_session`], in that order and
//! releases both before this loop continues (ADR 0021). A sweep over twenty
//! projects therefore never holds one project's lock while another's rows are
//! written.
//!
//! **The timer is the fallback.** `DISPATCHER_INTERVAL_SECS` bounds how long a
//! missed wake-up goes unnoticed; the `task_events` wake-up that makes the job
//! prompt is the next task of the epic. Either way a tick never overlaps its
//! own previous run (`cron::scheduler`), which is what lets
//! [`unattended_capacity`] be advisory.

use chrono::{DateTime, Utc};

use super::{CronService, JobReport};
use crate::models::{AgentProfile, ProfileKind, Project, ProjectStatus, TaskRef, TaskStateKind};
use crate::prelude::*;
use crate::repositories::{ProjectRepository, TaskRepository};
use crate::secrets::has_unattended_credential;
use crate::session::{LaunchActor, LaunchRequest, create_session, unattended_capacity};
use crate::tracker::leases::{READY_DEFAULT_LIMIT, ready_summaries};

/// Why a candidate or a whole profile was passed over, as the fixed word the
/// `reason` log field carries.
///
/// Fixed words rather than sentences, for the same reason
/// [`crate::session::CapacityRefusal::bound`] is one: a skip is something an
/// operator counts in a log query. The capacity bounds contribute their own
/// words through that method, so they are not repeated here.
const SKIP_PAUSED: &str = "automation_paused";
/// The profile is `conversational`, which no unattended launch may use.
const SKIP_NOT_EPHEMERAL: &str = "not_ephemeral";
/// The profile's agent credential no longer resolves without a user.
const SKIP_NO_CREDENTIAL: &str = "no_credential";
/// The profile serves no `queue` state, so no task can ever be its.
const SKIP_NO_SERVED_STATES: &str = "no_served_states";

impl CronService {
    /// Launch an ephemeral session for the best claimable task of every
    /// `auto_launch` profile, while capacity allows.
    ///
    /// `now` is unused: nothing this job decides is measured against a clock.
    /// A task is claimable or it is not, a cap has room or it has not, and the
    /// interval between runs is the whole of its relationship with time. The
    /// parameter is part of every job's signature
    /// ([`CronService::run_once`](super::CronService::run_once)).
    ///
    /// The counters: `items` is sessions launched, `skipped` is a profile or a
    /// candidate deliberately passed over — a paused project, a cap with no
    /// room, a missing credential, a lost claim — and `failures` is a launch
    /// that errored for a reason that is none of those, logged and stepped over
    /// so that one broken project cannot stop the sweep.
    ///
    /// # Errors
    ///
    /// Only from listing the projects, at which point nothing has been
    /// attempted and there is no partial sweep to report. Everything after that
    /// is counted rather than returned.
    pub async fn dispatcher(&self, now: DateTime<Utc>) -> Result<JobReport> {
        let _ = now;

        let projects = ProjectRepository::new(&self.state.pool);
        let project_ids = projects.list_ids_by_status(ProjectStatus::Ready).await?;

        let mut report = JobReport::default();

        for project_id in project_ids {
            // Re-read the row rather than trusting the listing: the pause is a
            // toggle a person flips while a sweep is running, and the capacity
            // check is decided against this project's own columns.
            let Some(project) = projects.find(project_id).await? else {
                continue;
            };
            if project.status != ProjectStatus::Ready {
                continue;
            }
            if project.automation_paused {
                debug!(project_id = %project_id, reason = SKIP_PAUSED, "dispatch skipped");
                report.skipped += 1;
                continue;
            }

            let profiles = match projects.list_auto_launch_profiles(project_id).await {
                Ok(profiles) => profiles,
                Err(error) => {
                    error!(
                        project_id = %project_id,
                        error = %error,
                        "the dispatcher could not list a project's auto-launch profiles",
                    );
                    report.failures += 1;
                    continue;
                }
            };

            for profile in profiles {
                match self.dispatch_profile(&project, &profile, &mut report).await {
                    Ok(Halt::Continue) => {}
                    // A bound wider than this profile refused: no later profile
                    // of this project can launch either, so the scan stops here
                    // rather than re-reading the same counts once per profile.
                    Ok(Halt::Project) => break,
                    Err(error) => {
                        error!(
                            project_id = %project_id,
                            profile_id = %profile.id,
                            error = %error,
                            "the dispatcher could not read a profile's candidates",
                        );
                        report.failures += 1;
                    }
                }
            }
        }

        Ok(report)
    }

    /// Launch for one profile until a cap refuses or its candidates run out.
    ///
    /// Answers whether the rest of the project's profiles are still worth
    /// asking about: a profile cap is this profile's own, but the project and
    /// instance caps and the pause are not.
    ///
    /// # Errors
    ///
    /// Only from the two reads that stand between this profile and its
    /// candidates — the credential lookup and the listing. A launch is counted,
    /// never returned, so one profile's failure costs the next one nothing.
    async fn dispatch_profile(
        &self,
        project: &Project,
        profile: &AgentProfile,
        report: &mut JobReport,
    ) -> Result<Halt> {
        // A conversational profile cannot be launched unattended at all: it has
        // no prompt to run and no user to type one. `auto_launch` is refused on
        // one at save (`models::agent_profile`), so this is the backstop under
        // that rule rather than a second policy.
        if profile.kind != ProfileKind::Ephemeral {
            warn!(
                project_id = %project.id,
                profile_id = %profile.id,
                reason = SKIP_NOT_EPHEMERAL,
                "dispatch skipped",
            );
            report.skipped += 1;
            return Ok(Halt::Continue);
        }

        // Checked again at launch time because a credential that resolved when
        // the profile was saved can have been deleted since (`ARCHITECTURE.md`,
        // "Unattended launches" → "Eligibility"). One line per run, at `info`,
        // because it is a configuration fault a person has to fix; nothing of
        // the credential itself is logged (rule 3).
        if !has_unattended_credential(&self.state.pool, project.id, profile.backend).await? {
            info!(
                project_id = %project.id,
                profile_id = %profile.id,
                reason = SKIP_NO_CREDENTIAL,
                "dispatch skipped",
            );
            report.skipped += 1;
            return Ok(Halt::Continue);
        }

        // Only the `queue` states, as "Dispatcher" says: a profile is refused
        // any other kind at save, so this filter is what keeps a task in the
        // human state or a terminal one out of the listing even if a row said
        // otherwise.
        let served: Vec<_> = TaskRepository::new(&self.state.pool)
            .list_profile_states(profile.id)
            .await?
            .into_iter()
            .filter(|state| state.kind == TaskStateKind::Queue)
            .map(|state| state.id)
            .collect();
        if served.is_empty() {
            debug!(
                project_id = %project.id,
                profile_id = %profile.id,
                reason = SKIP_NO_SERVED_STATES,
                "dispatch skipped",
            );
            return Ok(Halt::Continue);
        }

        // One page, not one query per launch: a task this run claims cannot
        // come back — it is held — and a candidate somebody else took is
        // stepped over, so re-asking after every launch would only repeat the
        // same rows. The page is deep enough that the caps run out first.
        let candidates =
            ready_summaries(&self.state.pool, project.id, &served, READY_DEFAULT_LIMIT).await?;

        for candidate in candidates {
            // Before every launch, never once per profile: each session this
            // run started counts against all three caps (ADR 0042).
            if let Some(refusal) = unattended_capacity(&self.state, project, profile)
                .await?
                .refusal()
            {
                debug!(
                    project_id = %project.id,
                    profile_id = %profile.id,
                    bound = refusal.bound(),
                    "dispatch skipped",
                );
                report.skipped += 1;
                return Ok(Halt::for_refusal(refusal));
            }

            let mut request = LaunchRequest::new(profile.id);
            request.task = Some(TaskRef::Id(candidate.id));
            // The whole point of an unattended launch: the task must be in a
            // state this profile serves, decided under the claim's own lock.
            request.require_served_state = true;

            match create_session(&self.state, project.id, request, LaunchActor::Dispatcher).await {
                Ok(session) => {
                    report.items += 1;
                    info!(
                        project_id = %project.id,
                        profile_id = %profile.id,
                        session_id = %session.id,
                        task_id = %candidate.id,
                        "dispatched",
                    );
                }
                // Somebody claimed it first, or the queue moved under us. Not a
                // fault: the next candidate is the answer.
                // A refused launch that is not a lost claim — a base that does
                // not resolve, a profile deleted under the run — is a
                // misconfiguration somebody should see, not a no-work outcome.
                Err(Error::BadRequest(reason)) => {
                    warn!(
                        project_id = %project.id,
                        profile_id = %profile.id,
                        task_id = %candidate.id,
                        reason = %reason,
                        "dispatch refused",
                    );
                    report.skipped += 1;
                }
                Err(Error::Conflict(reason)) => {
                    debug!(
                        project_id = %project.id,
                        profile_id = %profile.id,
                        task_id = %candidate.id,
                        reason = %reason,
                        "dispatch skipped",
                    );
                    report.skipped += 1;
                }
                Err(Error::NotFound) => {
                    debug!(
                        project_id = %project.id,
                        profile_id = %profile.id,
                        task_id = %candidate.id,
                        reason = "gone",
                        "dispatch skipped",
                    );
                    report.skipped += 1;
                }
                Err(error) => {
                    error!(
                        project_id = %project.id,
                        profile_id = %profile.id,
                        task_id = %candidate.id,
                        error = %error,
                        "dispatch failed",
                    );
                    report.failures += 1;
                }
            }
        }

        Ok(Halt::Continue)
    }
}

/// How far a capacity refusal reaches.
///
/// A profile cap is one profile's business and the next profile of the same
/// project may still have room; the pause and the two wider caps are the
/// project's or the instance's, and asking the remaining profiles would read
/// the same counts to the same answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Halt {
    /// Carry on with the next profile of this project.
    Continue,
    /// Nothing else in this project can launch now.
    Project,
}

impl Halt {
    /// What a refusal of `refusal` means for the rest of the project.
    fn for_refusal(refusal: crate::session::CapacityRefusal) -> Self {
        use crate::session::CapacityRefusal::*;

        match refusal {
            ProfileCap { .. } => Self::Continue,
            AutomationPaused | ProjectCap { .. } | InstanceCap { .. } => Self::Project,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::CapacityRefusal;

    /// Only the profile's own cap leaves the rest of the project worth asking.
    #[test]
    fn a_wider_bound_stops_the_project_and_the_profile_cap_does_not() {
        assert_eq!(
            Halt::for_refusal(CapacityRefusal::ProfileCap { live: 1, cap: 1 }),
            Halt::Continue,
        );
        for wider in [
            CapacityRefusal::AutomationPaused,
            CapacityRefusal::ProjectCap { live: 2, cap: 2 },
            CapacityRefusal::InstanceCap { live: 4, cap: 4 },
        ] {
            assert_eq!(Halt::for_refusal(wider), Halt::Project, "{wider:?}");
        }
    }
}
