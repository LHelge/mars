//! Scheduled agents: firing a profile's cron expression, once per tick.
//!
//! `ARCHITECTURE.md`, "Task tracker" → "Scheduled agents" and "Background
//! jobs"; ADR 0043. Each run walks the `ready` projects and, within each, the
//! profiles that carry a `schedule_cron`, and launches an ephemeral session of
//! every one whose expression came due since it was last accounted for — with
//! no task, `schedule_prompt` as the message and the orchestrator as the
//! actor.
//!
//! It is the sibling of `cron/dispatcher.rs` and composes as little: the
//! capacity rules of "Unattended launches" are [`unattended_capacity`], the
//! credential backstop is [`has_unattended_credential`] and the launch itself
//! is [`create_session`] with [`LaunchActor::Schedule`], which is what makes
//! `launch_source = 'schedule'`, `created_by` NULL and the tracker actor
//! `System`. What this job decides is only *when*.
//!
//! The name: the module is `cron/schedules.rs` and not `cron/scheduler.rs`,
//! because the latter is the generic job loop every job runs on
//! ([`super::scheduler`]). The job itself is [`JobName::Scheduler`].
//!
//! # Due, exactly once
//!
//! A profile's tick is **due** when its expression has an occurrence in
//! `(floor, now]`, where the floor is the later of `last_scheduled_at` and the
//! instant this process's cron service was built
//! ([`CronService::started_at`]).
//!
//! - **The floor's `last_scheduled_at` half** is why a tick fires once. The
//!   column is advanced *before* the launch, in one statement carrying the
//!   value that was read ([`ProjectRepository::claim_schedule_tick`]), so a
//!   restart inside the tick's own minute finds the tick already spent. A
//!   crash between the claim and the launch loses that one run; that is the
//!   direction this trades in, and the next occurrence is the retry.
//! - **The floor's process-start half** is why a tick that came due while the
//!   orchestrator was down is skipped rather than caught up. Nothing replays
//!   at startup, so an hourly agent that was off for a day runs once, at its
//!   next occurrence, and not twenty-four times.
//! - **Several occurrences in one window are one launch.** A run of this job
//!   that outlived its own period widens the next window, and
//!   [`CronSchedule::has_occurrence_in`](crate::models::schedule::CronSchedule::has_occurrence_in)
//!   answers whether the window holds an
//!   occurrence and not how many. Ticks are not a queue.
//!
//! # A claimed tick is spent, whatever happens next
//!
//! After the claim, every refusal — the project paused, any of the three caps,
//! a credential that no longer resolves, a profile that is not `ephemeral` —
//! is logged at `info` with the reason and counted as `skipped`. None of them
//! rolls `last_scheduled_at` back and none is queued: a schedule is "run at
//! these times", not "run this many times". A launch that fails for some other
//! reason is a `failures` count and the same: the next occurrence is the
//! retry.
//!
//! No lock is held across a launch and none is taken here. The scan is a pool
//! read, the claim is one unlocked statement, and each launch takes the
//! project git lock and then the project row lock inside [`create_session`]
//! and releases both before the sweep continues (ADR 0021).

use chrono::{DateTime, SecondsFormat, Utc};

use super::{CronService, JobReport};
use crate::models::{AgentProfile, ProfileKind, Project, ProjectStatus};
use crate::prelude::*;
use crate::repositories::ProjectRepository;
use crate::secrets::has_unattended_credential;
use crate::session::{LaunchActor, LaunchRequest, create_session, unattended_capacity};

/// Why a claimed tick was spent without launching, as the fixed word the
/// `reason` log field carries.
///
/// Fixed words rather than sentences, exactly as `cron/dispatcher.rs` has
/// them: a skip is something an operator counts in a log query. The capacity
/// bounds contribute their own words through
/// [`crate::session::CapacityRefusal::bound`], so they are not repeated here.
///
/// The profile is not `ephemeral`, which no unattended launch may be.
const SKIP_NOT_EPHEMERAL: &str = "not_ephemeral";
/// The profile's agent credential no longer resolves without a user.
const SKIP_NO_CREDENTIAL: &str = "no_credential";
/// Another writer moved `last_scheduled_at` between the scan and the claim.
const SKIP_NOT_CLAIMED: &str = "not_claimed";

impl CronService {
    /// Fire every scheduled profile whose expression came due, once each.
    ///
    /// `now` is both the end of every window this run considers and the value
    /// written to `last_scheduled_at`, so a run is decided entirely by the
    /// instant it was handed and never by a second reading of the clock: two
    /// profiles of one run share a window end, and a test drives the job with
    /// the instants it chose.
    ///
    /// **The claim comes before the launch.** `last_scheduled_at` is advanced
    /// first, so a crash between the two loses one run rather than firing it
    /// twice (`ARCHITECTURE.md`, "Task tracker" → "Scheduled agents").
    ///
    /// The counters: `items` is sessions launched, `skipped` is a claimed tick
    /// spent without one — a paused project, a cap with no room, a missing
    /// credential, a lost claim — and `failures` is a launch that errored for
    /// a reason that is none of those, logged and stepped over so one broken
    /// project cannot stop the sweep. A profile with no occurrence in its
    /// window is not counted at all: nothing was passed over, the time simply
    /// has not come.
    ///
    /// # Errors
    ///
    /// Only from listing the projects, at which point nothing has been claimed
    /// and there is no partial sweep to report. Everything after that is
    /// counted rather than returned.
    pub async fn scheduler(&self, now: DateTime<Utc>) -> Result<JobReport> {
        let projects = ProjectRepository::new(&self.state.pool);
        let project_ids = projects.list_ids_by_status(ProjectStatus::Ready).await?;

        let mut report = JobReport::default();

        for project_id in project_ids {
            // Re-read the row rather than trusting the listing, exactly as the
            // dispatcher does: the capacity check is decided against this
            // project's own columns, and the pause is a toggle a person flips
            // while a sweep is running. The pause is *not* a reason to skip the
            // scan, though — a tick of a paused project is claimed and then
            // refused, because a paused schedule is spent and not queued.
            let Some(project) = projects.find(project_id).await? else {
                continue;
            };
            if project.status != ProjectStatus::Ready {
                continue;
            }

            let profiles = match projects.list_scheduled_profiles(project_id).await {
                Ok(profiles) => profiles,
                Err(error) => {
                    error!(
                        project_id = %project_id,
                        error = %error,
                        "the scheduler could not list a project's scheduled profiles",
                    );
                    report.failures += 1;
                    continue;
                }
            };

            for profile in profiles {
                if let Err(error) = self
                    .fire_due_tick(&project, &profile, now, &mut report)
                    .await
                {
                    error!(
                        project_id = %project_id,
                        profile_id = %profile.id,
                        error = %error,
                        "the scheduler could not decide a profile's tick",
                    );
                    report.failures += 1;
                }
            }
        }

        Ok(report)
    }

    /// Decide one profile's tick and, if it is due and allowed, launch it.
    ///
    /// # Errors
    ///
    /// Only from the two reads between the claim and the launch — the
    /// credential lookup and the capacity counts — and from the claim itself.
    /// The launch is counted, never returned, so one profile's failure costs
    /// the next one nothing.
    async fn fire_due_tick(
        &self,
        project: &Project,
        profile: &AgentProfile,
        now: DateTime<Utc>,
        report: &mut JobReport,
    ) -> Result<()> {
        // `None` only for a stored expression that no longer parses, which
        // `AgentProfile::schedule` has already logged as the invariant failure
        // it is. Nothing to fire and nothing to say twice.
        let Some(schedule) = profile.schedule() else {
            return Ok(());
        };

        let floor = window_floor(profile.last_scheduled_at, self.started_at());
        if !schedule.has_occurrence_in(floor, now) {
            return Ok(());
        }

        // Before anything else, and before the launch above all: the tick is
        // spent the moment this returns `true`.
        if !ProjectRepository::new(&self.state.pool)
            .claim_schedule_tick(profile.id, profile.last_scheduled_at, now)
            .await?
        {
            debug!(
                project_id = %project.id,
                profile_id = %profile.id,
                reason = SKIP_NOT_CLAIMED,
                "scheduled run skipped",
            );
            report.skipped += 1;
            return Ok(());
        }

        // A schedule is refused on a conversational profile at save
        // (`models::agent_profile`), so this is the backstop under that rule
        // rather than a second policy.
        if profile.kind != ProfileKind::Ephemeral {
            warn!(
                project_id = %project.id,
                profile_id = %profile.id,
                reason = SKIP_NOT_EPHEMERAL,
                "scheduled run skipped",
            );
            report.skipped += 1;
            return Ok(());
        }

        // A credential that resolved when the profile was saved can have been
        // deleted since (`ARCHITECTURE.md`, "Unattended launches" →
        // "Eligibility"). Nothing of the credential itself is logged (rule 3).
        if !has_unattended_credential(&self.state.pool, project.id, profile.backend).await? {
            info!(
                project_id = %project.id,
                profile_id = %profile.id,
                reason = SKIP_NO_CREDENTIAL,
                "scheduled run skipped",
            );
            report.skipped += 1;
            return Ok(());
        }

        // The pause and the three caps, in one read (ADR 0042). A tick they
        // refuse is spent, logged and not queued.
        if let Some(refusal) = unattended_capacity(&self.state, project, profile)
            .await?
            .refusal()
        {
            info!(
                project_id = %project.id,
                profile_id = %profile.id,
                bound = refusal.bound(),
                "scheduled run skipped",
            );
            report.skipped += 1;
            return Ok(());
        }

        let mut request = LaunchRequest::new(profile.id);
        // No task, and so no served-state rule to apply: the prompt is the
        // whole of what this run was asked to do. It is required whenever a
        // schedule is set, so a profile reaching this point without one is an
        // invariant failure the launch refuses as a missing prompt rather than
        // something to paper over here.
        request.message = profile.schedule_prompt.clone();
        request.title = Some(run_title(&profile.name, now));

        match create_session(&self.state, project.id, request, LaunchActor::Schedule).await {
            Ok(session) => {
                report.items += 1;
                info!(
                    project_id = %project.id,
                    profile_id = %profile.id,
                    session_id = %session.id,
                    "scheduled run launched",
                );
            }
            // A base that does not resolve, a profile deleted under the run, a
            // prompt that is not there: a misconfiguration somebody should see.
            // The tick stays spent either way — the next occurrence is the
            // retry, which is the whole of this job's back-off.
            Err(error) => {
                error!(
                    project_id = %project.id,
                    profile_id = %profile.id,
                    error = %error,
                    "scheduled run failed to launch",
                );
                report.failures += 1;
            }
        }

        Ok(())
    }
}

/// The instant a profile's window opens: everything up to and including it has
/// already been accounted for.
///
/// The later of the last tick that fired and this process's start, which is
/// the two rules of "Scheduled agents" in one expression — fire once, and
/// never catch up.
fn window_floor(
    last_scheduled_at: Option<DateTime<Utc>>,
    started_at: DateTime<Utc>,
) -> DateTime<Utc> {
    last_scheduled_at.map_or(started_at, |last| last.max(started_at))
}

/// The title a scheduled session is launched with.
///
/// It names the profile and the instant, because that is what a reader of the
/// session list needs from a session nobody asked for: which schedule, and
/// which run of it. Seconds precision and a `Z`, so two runs of one minute —
/// which the claim makes impossible, but a clock is not an argument — still
/// read differently.
///
/// Bounded by construction: a profile name is at most 64 characters
/// (`models::agent_profile::MAX_PROFILE_NAME_CHARS`) and the rest is fixed at
/// 37, well inside the 200 a session title may have.
fn run_title(profile_name: &str, now: DateTime<Utc>) -> String {
    format!(
        "{profile_name} — scheduled run {}",
        now.to_rfc3339_opts(SecondsFormat::Secs, true)
    )
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;
    use crate::models::agent_profile::MAX_PROFILE_NAME_CHARS;
    use crate::models::session::MAX_SESSION_TITLE_CHARS;

    fn at(hour: u32, minute: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 22, hour, minute, 0).unwrap()
    }

    #[test]
    fn a_profile_that_never_fired_opens_its_window_at_the_process_start() {
        assert_eq!(window_floor(None, at(3, 0)), at(3, 0));
    }

    #[test]
    fn a_tick_older_than_the_process_start_does_not_lower_the_floor() {
        // Anything between the two is what was missed while the service was
        // down, and it is never caught up.
        assert_eq!(window_floor(Some(at(1, 0)), at(3, 0)), at(3, 0));
    }

    #[test]
    fn a_tick_of_this_process_is_the_floor() {
        assert_eq!(window_floor(Some(at(4, 0)), at(3, 0)), at(4, 0));
    }

    #[test]
    fn the_longest_run_title_fits_a_session_title() {
        let title = run_title(&"n".repeat(MAX_PROFILE_NAME_CHARS), at(3, 0));

        assert!(
            title.chars().count() <= MAX_SESSION_TITLE_CHARS,
            "{} characters: {title}",
            title.chars().count(),
        );
    }

    #[test]
    fn the_run_title_names_the_profile_and_the_instant() {
        assert_eq!(
            run_title("nightly scan", at(3, 30)),
            "nightly scan — scheduled run 2026-09-22T03:30:00Z",
        );
    }
}
