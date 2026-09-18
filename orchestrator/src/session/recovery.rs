//! Startup recovery: adopt what is still running, park what is not and fail
//! what was half-built (`ARCHITECTURE.md`, "Restart procedure", "Durability and
//! recovery").
//!
//! [`recover`] runs once, between the engine's startup probe and the cron jobs,
//! and before either listener accepts traffic: a stale owner must not be able to
//! race a launch a request started (`ARCHITECTURE.md`, "Engine adapter", Startup
//! probe; "Restart procedure", steps 2–4).
//!
//! What it does, in the order the document gives:
//!
//! 1. lists every container carrying `mars.session_id` once, keyed by the label
//!    parsed as a [`Uuid`];
//! 2. for every `running` session: a container that is still running is
//!    *adopted* — stdin reattached, tailing resumed from `MAX(_offset)`, the
//!    process's translation state reconstructed by the owner itself — while a
//!    container that has exited gets an owner whose exit watch resolves at once,
//!    so the lifecycle rule for that exit code is applied by the same code that
//!    would have applied it had the orchestrator never stopped, and a session
//!    whose container is gone is parked;
//! 3. every `creating` session is failed with [`CREATING_REASON`], its labelled
//!    container removed and its end-of-session hook invoked, which releases a
//!    task claimed at launch;
//! 4. and the report is logged, which is where the caller's startup log says how
//!    many of each there were.
//!
//! **Adoption writes no configuration.** The process is already running, so its
//! MCP bearer token is the one in its `mcp.json` and the hash in its row is the
//! hash of exactly that token: nothing here calls
//! [`rotate_token`](crate::session::rotate_token) or
//! [`write_mcp_json`](crate::session::write_mcp_json), and a rotation at this
//! point would lock the running CLI out of the tracker (ADR 0029).
//!
//! **Sessions in `parked`, `done` or `failed` are not touched**, whatever the
//! engine lists for them. A container belonging to one of those is a stray, and
//! removing strays is the orphan-cleanup job's work, not recovery's
//! (`ARCHITECTURE.md`, "Background jobs").
//!
//! **One session's failure is not the sweep's.** An engine that cannot attach,
//! a row that changed underneath, a profile that cannot be read: each is logged
//! against its own `session_id` and the sweep continues. Only the listing itself
//! failing is fatal, because a recovery that does not know what is running would
//! adopt nothing and park sessions whose CLI is alive.

use std::collections::BTreeMap;

use uuid::Uuid;

use crate::agent::{AgentBackend, TranslateConfig, backend_for};
use crate::engine::{ContainerId, ContainerState, ContainerSummary, EngineError, LABEL_SESSION_ID};
use crate::events::AgentEventBody;
// The model enum and the agent trait share the name `AgentBackend`, and this
// module needs both; the enum is `Backend` here, as it is inside `agent/`.
use crate::models::AgentBackend as Backend;
use crate::models::{Session, SessionKind, SessionState};
use crate::prelude::*;
use crate::repositories::{ProjectRepository, SessionRepository, Transition};
use crate::session::{OwnerContext, Phase, SessionDirs, SessionOwner};

/// What a session left in `creating` by the restart is failed with
/// (`ARCHITECTURE.md`, "Restart procedure", step 3). The user retries.
pub const CREATING_REASON: &str = "orchestrator restarted during creation";

/// What a `running` session whose container the engine no longer has — or never
/// started — is parked with (`ARCHITECTURE.md`, "Restart procedure", step 2).
pub const MISSING_CONTAINER_REASON: &str = "container missing after orchestrator restart";

/// How many of each thing recovery did, for the one line the caller logs.
///
/// `adopted` counts the owners that were created, which includes the owner of a
/// container that had already exited: it drains the transcript and applies the
/// exit rule, so its session leaves `running` a moment after `recover` returns
/// rather than inside it. `parked` and `failed` count only the states recovery
/// wrote itself — and an ephemeral session that would have been parked is
/// counted, like the row itself, as `failed` (ADR 0003).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RecoveryReport {
    /// Sessions handed to a new [`SessionOwner`].
    pub adopted: usize,
    /// Sessions moved `running → parked` because their container was gone or
    /// could not be reattached.
    pub parked: usize,
    /// Sessions moved to `failed`: every `creating` one, plus an ephemeral
    /// session recovery would otherwise have parked.
    pub failed: usize,
}

/// What became of one `running` session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// An owner was spawned for it.
    Adopted,
    /// It was moved to `parked`.
    Parked,
    /// It was moved to `failed`, which is where an ephemeral session goes
    /// wherever the rules say `parked` (ADR 0003).
    Failed,
}

impl Outcome {
    /// The value of the `action` log field, which is the same word the report
    /// counts under.
    fn as_str(self) -> &'static str {
        match self {
            Outcome::Adopted => "adopted",
            Outcome::Parked => "parked",
            Outcome::Failed => "failed",
        }
    }
}

/// Adopt, park and fail everything the last process left behind.
///
/// `Err` only when the container listing fails, which is fatal at startup: see
/// the module documentation.
pub async fn recover(state: &AppState) -> Result<RecoveryReport> {
    // Once, for the whole sweep: the engine is asked for the label index a
    // single time and every decision below reads this map
    // (`ARCHITECTURE.md`, "Engine adapter", the list row).
    let mut by_session = index_by_session(state.engine.list_by_label(LABEL_SESSION_ID).await?);
    let repository = SessionRepository::new(&state.pool);
    let mut report = RecoveryReport::default();

    for session in repository.list_by_state(SessionState::Running).await? {
        let containers = by_session.remove(&session.id).unwrap_or_default();
        let outcome = match recover_running(state, &session, containers).await {
            Ok(outcome) => outcome,
            Err(err) => {
                // The row is left in `running` for the next start to try again:
                // it is the truthful state of a session whose container this
                // sweep could not decide anything about.
                error!(
                    session_id = %session.id,
                    error = %err,
                    "a running session could not be recovered; leaving it as it is",
                );
                continue;
            }
        };

        info!(
            session_id = %session.id,
            action = outcome.as_str(),
            "a running session was recovered",
        );
        match outcome {
            Outcome::Adopted => report.adopted += 1,
            Outcome::Parked => report.parked += 1,
            Outcome::Failed => report.failed += 1,
        }
    }

    // After the adoptions, as the document orders them: a session in `creating`
    // has no container coming and no owner to attach (step 3).
    for session in repository.fail_all_creating(CREATING_REASON).await? {
        let containers = by_session.remove(&session.id).unwrap_or_default();
        discard_containers(state, session.id, &containers).await;
        clear_container_id(state, &session).await;

        // The launch may have claimed a task; nothing else releases the lease
        // for a session that never ran (`ARCHITECTURE.md`, "Task tracker").
        state.session_ended(session.id).await;

        info!(
            session_id = %session.id,
            action = Outcome::Failed.as_str(),
            reason = CREATING_REASON,
            "a creating session was failed by the restart",
        );
        report.failed += 1;
    }

    info!(
        adopted = report.adopted,
        parked = report.parked,
        failed = report.failed,
        "startup recovery finished",
    );

    Ok(report)
}

/// Group the listed containers by the session their label names.
///
/// A label that is not a `Uuid` belongs to something that is not one of Mars's
/// sessions, or to a session id this version cannot read; either way there is no
/// row to reconcile it against, so it is logged and left alone rather than
/// removed.
fn index_by_session(listed: Vec<ContainerSummary>) -> BTreeMap<Uuid, Vec<ContainerSummary>> {
    let mut by_session: BTreeMap<Uuid, Vec<ContainerSummary>> = BTreeMap::new();

    for summary in listed {
        let label = summary.labels.get(LABEL_SESSION_ID).cloned();
        let Some(label) = label else {
            // The filter was the key, so this cannot happen through the
            // documented contract; an engine that answers otherwise is not
            // something to act on.
            warn!(
                container_id = %summary.id,
                "a listed container does not carry the label it was filtered by",
            );
            continue;
        };

        match Uuid::parse_str(&label) {
            Ok(session_id) => by_session.entry(session_id).or_default().push(summary),
            Err(_) => warn!(
                container_id = %summary.id,
                "a container's mars.session_id label is not a uuid; skipping it",
            ),
        }
    }

    by_session
}

/// One `running` session: adopt it, or park it and say why.
async fn recover_running(
    state: &AppState,
    session: &Session,
    containers: Vec<ContainerSummary>,
) -> Result<Outcome> {
    let (live, strays) = split_by_state(state, containers).await?;

    // Whatever is not the container this session is being adopted through is a
    // leftover of a crash between a create and a remove, and removing it is
    // recovery's own clean-up rather than the orphan job's: it carries the label
    // of a session that is not parked, done or failed, so nothing else would
    // ever collect it (`ARCHITECTURE.md`, "Background jobs").
    discard_containers(state, session.id, &strays).await;

    let Some((container_id, container_state)) = live else {
        return park(state, session, MISSING_CONTAINER_REASON).await;
    };

    // An exited container is adopted too, and the owner's exit path is what
    // reads the code: `engine.wait` answers immediately, the transcript is
    // drained to end of file and the lifecycle rule for that exit decides the
    // state (`ARCHITECTURE.md`, "Session owner task", step 3). No stdin is
    // attached, because there is no process left to write to.
    let stdin = if container_state.is_running() {
        match state.engine.attach_stdin(&container_id).await {
            Ok(stdin) => Some(stdin as Box<dyn tokio::io::AsyncWrite + Send + Unpin>),
            Err(err) => {
                warn!(
                    session_id = %session.id,
                    container_id = %container_id,
                    error = %err,
                    "could not reattach a running session's stdin",
                );
                // The container keeps running and is left to the orphan-cleanup
                // job, which collects containers whose session is parked: an
                // owner that cannot write stdin cannot stop it cleanly either.
                return park(state, session, &reattach_reason(&err)).await;
            }
        }
    } else {
        None
    };

    spawn_owner(state, session, container_id, stdin).await?;

    Ok(Outcome::Adopted)
}

/// Sort this session's containers into the one to adopt and the ones to remove.
///
/// Each candidate is inspected, because a list row says only whether the engine
/// calls it running and the three answers are different: running is adopted,
/// exited is adopted so its exit is read, and anything else — `created`,
/// `paused`, `dead`, a state this version does not know, or a container that has
/// disappeared between the list and the inspect — is treated as gone.
///
/// A `NotFound` is that last case; any other inspect failure is returned, which
/// leaves the session in `running` for the next start rather than parking a
/// session whose CLI may well be alive behind an engine that answered badly
/// (`ARCHITECTURE.md`, "Durability and recovery").
///
/// A running container wins over an exited one: two containers carrying the same
/// label is a crash between the create and the remove of a resume, and the one
/// with the live process is the session's.
#[allow(clippy::type_complexity)]
async fn split_by_state(
    state: &AppState,
    containers: Vec<ContainerSummary>,
) -> Result<(Option<(ContainerId, ContainerState)>, Vec<ContainerSummary>)> {
    let mut inspected = Vec::with_capacity(containers.len());

    for summary in containers {
        match state.engine.inspect(&summary.id).await {
            Ok(info) => inspected.push((summary, info.state)),
            Err(EngineError::NotFound(_)) => debug!(
                container_id = %summary.id,
                "a listed container was gone by the time recovery inspected it",
            ),
            Err(err) => return Err(err.into()),
        }
    }

    let exited =
        |container_state: &ContainerState| matches!(container_state, ContainerState::Exited { .. });
    let chosen = inspected
        .iter()
        .position(|(_, container_state)| container_state.is_running())
        .or_else(|| {
            inspected
                .iter()
                .position(|(_, container_state)| exited(container_state))
        });

    let Some(index) = chosen else {
        return Ok((
            None,
            inspected.into_iter().map(|(summary, _)| summary).collect(),
        ));
    };

    let (summary, container_state) = inspected.remove(index);
    let strays = inspected.into_iter().map(|(summary, _)| summary).collect();

    Ok((Some((summary.id, container_state)), strays))
}

/// Build the adopted owner's context and put it on its own task.
///
/// `adopted: true` is the whole difference from a launch: the owner reconstructs
/// the running process's open subagents, denial bookkeeping, input-echo hashes
/// and cumulative-cost baseline from retained history before it reads a byte past
/// the committed offset, publishing nothing (`ARCHITECTURE.md`, "Durability and
/// recovery"). The registry entry is [`Phase::Running`] straight away: an adopted
/// process is long past its `init` and there is nothing to wait for, and the
/// inputs that queued before the restart are gone with the old process (ADR
/// 0020).
async fn spawn_owner(
    state: &AppState,
    session: &Session,
    container_id: ContainerId,
    stdin: Option<Box<dyn tokio::io::AsyncWrite + Send + Unpin>>,
) -> Result<()> {
    let repository = SessionRepository::new(&state.pool);
    let start_offset = repository.max_offset(session.id).await?.unwrap_or(0).max(0) as u64;
    let (backend, partial_messages) = profile_settings(state, session).await;

    // The row is the orchestrator's record of which container is the session's,
    // and the container just adopted is the one that is really there.
    if session.container_id.as_deref() != Some(container_id.0.as_str()) {
        warn!(
            session_id = %session.id,
            container_id = %container_id,
            "the adopted container is not the one the session row recorded",
        );
        let mut tx = state.pool.begin().await?;
        repository
            .set_container_id(&mut tx, session.id, Some(container_id.0.as_str()))
            .await?;
        tx.commit().await?;
    }

    let commands = state
        .session_registry
        .register(session.id, session.kind, Phase::Running);

    SessionOwner::spawn(OwnerContext {
        session_id: session.id,
        kind: session.kind,
        dirs: SessionDirs::from_config(&state.config, session.id),
        backend,
        start_offset,
        translate: TranslateConfig {
            resumed: resumed_flag(&repository, session.id).await?,
            partial_messages,
            // Not recoverable: which model credential was injected is a
            // property of the launch, and no row records it. `None` costs only
            // the sentence a credential failure would otherwise have added to
            // its warning — the failure itself is still reported
            // (`ARCHITECTURE.md`, "Secrets", Resolution at launch).
            credential: None,
        },
        adopted: true,
        stdin,
        container_id: Some(container_id),
        commands,
        state: state.clone(),
    });

    Ok(())
}

/// The adapter and the `partial_messages` flag the adopted process was launched
/// with.
///
/// A profile that cannot be read is not a reason to refuse the adoption: the
/// defaults translate the same output, with `partial_messages` off, which is
/// what a profile that never asked for them would have had.
async fn profile_settings(state: &AppState, session: &Session) -> (Arc<dyn AgentBackend>, bool) {
    let profile = ProjectRepository::new(&state.pool)
        .find_profile(session.project_id, session.profile_id)
        .await;

    match profile {
        Ok(Some(profile)) => (backend_for(profile.backend), profile.partial_messages),
        Ok(None) | Err(_) => {
            warn!(
                session_id = %session.id,
                profile_id = %session.profile_id,
                "the adopted session's profile could not be read; adopting with the defaults",
            );
            (backend_for(Backend::Claude), false)
        }
    }
}

/// Whether the process being adopted was launched with `--resume`, as its own
/// committed `init` reported it.
///
/// The flag belongs to the process and only its transcript records it, so it is
/// read back from the last `init` this process committed rather than guessed
/// from the session row. `false` when the process has not announced one yet,
/// which is what a fresh launch would have said.
async fn resumed_flag(repository: &SessionRepository<'_>, session_id: Uuid) -> Result<bool> {
    let start = repository.process_start(session_id).await?;
    let resumed = repository
        .events_after(session_id, start.launch_seq)
        .await?
        .iter()
        .rev()
        .find_map(|event| match event.event.body {
            AgentEventBody::Init { resumed, .. } => Some(resumed),
            _ => None,
        })
        .unwrap_or(false);

    Ok(resumed)
}

/// Move a `running` session out of `running` with `reason`, and forget its
/// container.
///
/// `parked` for a conversational session, which is the state a session with no
/// process is in, and `failed` with the same reason for an ephemeral one: an
/// ephemeral session is never parked, resumed or retried, so a restart that
/// finds its process gone ends it (ADR 0003; `ARCHITECTURE.md`, "Restart
/// procedure", "Claude Code invocation").
///
/// One transaction, so the `state_change` the UI reads and the cleared
/// `container_id` cannot disagree.
async fn park(state: &AppState, session: &Session, reason: &str) -> Result<Outcome> {
    let to = if session.kind == SessionKind::Ephemeral {
        SessionState::Failed
    } else {
        SessionState::Parked
    };

    let mut change = Transition::new(SessionState::Running, to, reason);
    if to == SessionState::Failed {
        change = change.with_error(reason);
    }

    let repository = SessionRepository::new(&state.pool);
    let mut tx = state.pool.begin().await?;
    repository.transition(&mut tx, session.id, &change).await?;
    repository
        .set_container_id(&mut tx, session.id, None)
        .await?;
    tx.commit().await?;

    Ok(if to == SessionState::Failed {
        Outcome::Failed
    } else {
        Outcome::Parked
    })
}

/// Remove containers recovery has decided are not a session's any more.
///
/// Forced, because a duplicate left by a crashed resume may still be running,
/// and best effort: a container that cannot be removed is logged and left for
/// the orphan-cleanup job, which is the other half of this rule
/// (`ARCHITECTURE.md`, "Background jobs").
async fn discard_containers(state: &AppState, session_id: Uuid, containers: &[ContainerSummary]) {
    for summary in containers {
        match state.engine.remove(&summary.id, true).await {
            Ok(()) | Err(EngineError::NotFound(_)) => info!(
                session_id = %session_id,
                container_id = %summary.id,
                "removed a container the restart left behind",
            ),
            Err(err) => warn!(
                session_id = %session_id,
                container_id = %summary.id,
                error = %err,
                "could not remove a container the restart left behind",
            ),
        }
    }
}

/// Clear `container_id` on a session whose container recovery has removed.
///
/// Its own transaction and best effort: the state change is what matters and it
/// is already committed (`docs/data-model.md`, `sessions.container_id`).
async fn clear_container_id(state: &AppState, session: &Session) {
    if session.container_id.is_none() {
        return;
    }

    let cleared = async {
        let mut tx = state.pool.begin().await?;
        SessionRepository::new(&state.pool)
            .set_container_id(&mut tx, session.id, None)
            .await?;
        tx.commit().await?;
        Ok::<(), Error>(())
    }
    .await;

    if let Err(err) = cleared {
        error!(
            session_id = %session.id,
            error = %err,
            "could not clear a failed session's container id",
        );
    }
}

/// What a session parked because its stdin could not be reattached says.
///
/// The engine's own message is part of it: an operator reading the transcript
/// has to be able to tell a socket that went away from a container that refused
/// the attach. It carries no secret — an [`EngineError`] never does (rule 3).
fn reattach_reason(err: &EngineError) -> String {
    format!("could not reattach after restart: {err}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reasons_are_the_ones_the_restart_procedure_names() {
        assert_eq!(CREATING_REASON, "orchestrator restarted during creation");
        assert_eq!(
            MISSING_CONTAINER_REASON,
            "container missing after orchestrator restart",
        );
    }

    #[test]
    fn a_reattach_failure_names_the_engines_own_message() {
        assert_eq!(
            reattach_reason(&EngineError::NotFound("mock-1".to_string())),
            "could not reattach after restart: the container engine has no such object: mock-1",
        );
    }

    #[test]
    fn every_outcome_logs_the_word_the_report_counts_under() {
        assert_eq!(Outcome::Adopted.as_str(), "adopted");
        assert_eq!(Outcome::Parked.as_str(), "parked");
        assert_eq!(Outcome::Failed.as_str(), "failed");
    }
}
