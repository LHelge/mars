//! One answer to "may this profile launch now?".
//!
//! [`unattended_capacity`] is the whole of `ARCHITECTURE.md`, "Task tracker" →
//! "Unattended launches" → "Capacity" and "Pause" (ADR 0042), in one function
//! that both jobs that launch without a user — the dispatcher and, later, the
//! scheduled agents — ask before they claim anything. It is here, beside
//! [`crate::session::create`], because it is the question that comes
//! immediately before that path and shares its subject: a project, a profile
//! and the sessions already running.
//!
//! **A user launch never asks it.** The three caps and the pause hold
//! automation back and nothing else; a person pressing the button is never
//! refused by them, which is why [`crate::session::create_session`] does not
//! call this and the caller that has no user does.

use crate::models::{AgentProfile, Project};
use crate::prelude::*;
use crate::repositories::{LiveSessionCounts, SessionRepository};

/// Whether an unattended launch of a profile may happen now, and if not, which
/// bound said no.
///
/// An enum rather than a `bool` and a message, so the caller logs the reason
/// with structured fields — `bound = "project_cap"`, `live = 3`, `cap = 3` —
/// and nothing string-formatted (`CLAUDE.md`, "Backend conventions").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchCapacity {
    /// Every bound has room: the launch may go ahead.
    Available,
    /// One bound refused, and this is which.
    Refused(CapacityRefusal),
}

impl LaunchCapacity {
    /// Whether the launch may go ahead.
    pub fn is_available(self) -> bool {
        matches!(self, Self::Available)
    }

    /// The refusal, or `None` when there was none.
    pub fn refusal(self) -> Option<CapacityRefusal> {
        match self {
            Self::Available => None,
            Self::Refused(refusal) => Some(refusal),
        }
    }
}

/// Which of the four bounds refused an unattended launch.
///
/// In the order they are checked: the pause first, because it is a fact about
/// the project and costs no count, then the three caps from the narrowest
/// scope outwards, so the reason logged is the tightest one that applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapacityRefusal {
    /// `projects.automation_paused` is set. No count was consulted: the pause
    /// refuses whatever the counts are.
    AutomationPaused,
    /// The profile's own `max_concurrent` is reached.
    ProfileCap {
        /// Live sessions of this profile.
        live: i64,
        /// `agent_profiles.max_concurrent`.
        cap: i64,
    },
    /// The project's `max_concurrent_sessions` is reached. A project whose
    /// column is NULL has no project cap and can never refuse here.
    ProjectCap {
        /// Live sessions of this project.
        live: i64,
        /// `projects.max_concurrent_sessions`.
        cap: i64,
    },
    /// The instance's `AUTOMATION_MAX_SESSIONS` is reached.
    InstanceCap {
        /// Live sessions on the instance.
        live: i64,
        /// `AUTOMATION_MAX_SESSIONS`.
        cap: i64,
    },
}

impl CapacityRefusal {
    /// A fixed word naming the bound, for the `bound = ...` field of the
    /// caller's log line.
    ///
    /// A constant per variant and never a formatted sentence, so the refusals
    /// are countable in a log query.
    pub fn bound(self) -> &'static str {
        match self {
            Self::AutomationPaused => "automation_paused",
            Self::ProfileCap { .. } => "profile_cap",
            Self::ProjectCap { .. } => "project_cap",
            Self::InstanceCap { .. } => "instance_cap",
        }
    }
}

/// May `profile` launch an unattended session in `project` right now?
///
/// The four bounds of `ARCHITECTURE.md`, "Task tracker" → "Unattended
/// launches", in one read: the project must not be `automation_paused`, and the
/// live sessions — `creating` or `running` — must be *below*
/// `agent_profiles.max_concurrent`, below `projects.max_concurrent_sessions`
/// when that column is set, and below `AUTOMATION_MAX_SESSIONS`. Each count
/// includes every live session in its scope, whoever launched it, so a session
/// a person started holds automation back exactly as a dispatched one does.
///
/// # The answer is advisory, by design
///
/// The counts are read on the pool, outside the transaction the launch then
/// opens, and no row is locked. Nothing here promises that the number is still
/// true when the session row is inserted, and that is deliberate:
///
/// - **The jobs are single-flight.** A dispatcher or scheduler tick never
///   overlaps its own previous run, so the only launches that can race this
///   answer are a person's — and a person's launch is not something these caps
///   may refuse or delay in the first place.
/// - **Holding the count would cost the wrong lock.** Making it exact means
///   locking every project on the instance, or serialising the instance's
///   launches behind one row, for a ceiling whose purpose is to keep a host
///   from being swamped rather than to be exact at the boundary.
/// - **The overshoot is bounded and self-correcting.** A user launch committed
///   between this read and the insert can put the instance one over a cap;
///   the next tick sees the higher count and launches nothing until a session
///   ends. A cap is a ceiling on what automation adds, not an invariant of the
///   `sessions` table, and `ARCHITECTURE.md` says as much: a launch a cap would
///   exceed is skipped and logged, never queued.
///
/// Nothing is logged here. The caller knows which profile and which task it was
/// about to launch, and logs the refusal with those fields beside
/// [`CapacityRefusal::bound`].
pub async fn unattended_capacity(
    state: &AppState,
    project: &Project,
    profile: &AgentProfile,
) -> Result<LaunchCapacity> {
    // First, and without counting anything: the pause refuses whatever the
    // counts are, and a paused project is the common case for a job ticking
    // over a project somebody switched off.
    if project.automation_paused {
        return Ok(LaunchCapacity::Refused(CapacityRefusal::AutomationPaused));
    }

    let counts = SessionRepository::new(&state.pool)
        .count_live(project.id, profile.id)
        .await?;

    Ok(decide(
        counts,
        i64::from(profile.max_concurrent),
        project.max_concurrent_sessions.map(i64::from),
        state.config.automation_max_sessions,
    ))
}

/// The comparison itself, over numbers alone.
///
/// Split out so the ordering of the three caps — narrowest scope first, so the
/// reason reported is the tightest bound that applies — is testable without a
/// database.
fn decide(
    counts: LiveSessionCounts,
    profile_cap: i64,
    project_cap: Option<i64>,
    instance_cap: i64,
) -> LaunchCapacity {
    if counts.profile >= profile_cap {
        return LaunchCapacity::Refused(CapacityRefusal::ProfileCap {
            live: counts.profile,
            cap: profile_cap,
        });
    }
    // A NULL column is no project cap at all, not a cap of zero.
    if let Some(cap) = project_cap
        && counts.project >= cap
    {
        return LaunchCapacity::Refused(CapacityRefusal::ProjectCap {
            live: counts.project,
            cap,
        });
    }
    if counts.instance >= instance_cap {
        return LaunchCapacity::Refused(CapacityRefusal::InstanceCap {
            live: counts.instance,
            cap: instance_cap,
        });
    }

    LaunchCapacity::Available
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counts(profile: i64, project: i64, instance: i64) -> LiveSessionCounts {
        LiveSessionCounts {
            profile,
            project,
            instance,
        }
    }

    /// Below every bound is available; the counts need not be zero.
    #[test]
    fn room_under_every_cap_is_available() {
        assert_eq!(
            decide(counts(1, 2, 3), 2, Some(3), 4),
            LaunchCapacity::Available
        );
    }

    /// Each cap refuses at its own limit, and names itself.
    #[test]
    fn each_cap_refuses_at_its_limit() {
        assert_eq!(
            decide(counts(2, 2, 2), 2, Some(9), 9)
                .refusal()
                .map(|r| r.bound()),
            Some("profile_cap"),
        );
        assert_eq!(
            decide(counts(0, 3, 3), 9, Some(3), 9)
                .refusal()
                .map(|r| r.bound()),
            Some("project_cap"),
        );
        assert_eq!(
            decide(counts(0, 0, 4), 9, Some(9), 4)
                .refusal()
                .map(|r| r.bound()),
            Some("instance_cap"),
        );
    }

    /// The tightest bound is the one reported when several are reached.
    #[test]
    fn the_narrowest_bound_is_the_reason() {
        assert_eq!(
            decide(counts(2, 2, 2), 2, Some(2), 2),
            LaunchCapacity::Refused(CapacityRefusal::ProfileCap { live: 2, cap: 2 }),
        );
    }

    /// A NULL project cap is no cap, not a cap of zero.
    #[test]
    fn a_null_project_cap_never_refuses() {
        assert_eq!(
            decide(counts(0, 99, 0), 9, None, 9),
            LaunchCapacity::Available
        );
    }
}
