//! The launcher: what turns a `creating` or `parked` session row into a
//! running container with a [`SessionOwner`] attached to it.
//!
//! [`Launcher::launch`] is the whole of `ARCHITECTURE.md`, "Launch sequence" as
//! code. It spawns a task and returns, because a launch clones a repository and
//! pulls an image and no request waits for that: the row is already `creating`
//! (or `parked`) and every step's outcome reaches the client as an event on the
//! session stream.
//!
//! **The order is the contract**, and it is the document's:
//!
//! 1. the session, its project, its profile, the project's shared directories
//!    and the launching user are read;
//! 2. the session directories exist ([`SessionDirs::ensure`]) and `mcp.json` is
//!    written — from the token the route generated on a fresh launch, from
//!    [`rotate_token`] on a resume, which commits the replacement hash before
//!    the file and before any container (ADR 0029);
//! 3. on a fresh launch the mirror is fetched, skipped when it was fetched less
//!    than [`FRESH_FETCH_MAX_AGE`] ago and *warned* about rather than failed
//!    when the upstream is unreachable; then, under the project git lock, the
//!    base ref is resolved and the work tree is cloned on `session/<sid>`;
//! 4. the project's shared directories and its CLI state directory are created
//!    if missing (ADR 0015);
//! 5. the profile's secrets and the backend's agent credential are resolved,
//!    each missing profile name becoming a `launch_warning` and a backend with
//!    no credential row at any scope becoming one more (ADR 0036);
//! 6. the container specification is built by
//!    [`build_session_spec`](crate::engine::spec::build_session_spec) — the one
//!    place the specification table lives — the image is pulled if absent, the
//!    container is created, the egress network is connected, the container is
//!    started and stdin is attached;
//! 7. the session becomes `running` **at the stdin attach** (ADR 0032), the
//!    owner is spawned, and the inputs that queued during the launch are handed
//!    to it in order.
//!
//! **Why `running` is committed before the owner exists.** The owner of an
//! adopted process reconstructs it from the last `state_change` into `running`
//! (`ARCHITECTURE.md`, "Durability and recovery"), so that transition has to be
//! in the transcript before the new process can commit any output at all.
//! Hence: transition, then [`SessionOwner::spawn`], then
//! [`SessionRegistry::mark_running`](crate::session::SessionRegistry::mark_running)
//! and the queued inputs.
//!
//! **A resume differs in three things only**: it rotates the token first, it
//! passes `--resume <cli_session_id>` and it does no git work, because the
//! checkout is already there. A parked session that never spoke has no
//! `cli_session_id` and is relaunched fresh in that same checkout, which is not
//! an error — there is no conversation to resume (ADR 0032). A resume whose work
//! directory has gone is a retry of a session that failed during creation, and
//! runs the git sequence.
//!
//! **A launch can be cancelled.** A user who ends a session that is still
//! `creating` cancels its launch through the registry
//! ([`SessionRegistry::cancel_launch`](crate::session::SessionRegistry::cancel_launch)),
//! and the sequence reads that flag at three checkpoints: before the git work,
//! before any container is created, and after the stdin attach, immediately
//! before the transition into `running`. At any of them it stops where it is,
//! [`cancelled`] removes the container if one was created and clears
//! `container_id`, and nothing else is touched: the state change, the registry
//! entry and the end-of-session hook are the waiting `end`'s, which is what
//! keeps one session from being closed twice (`ARCHITECTURE.md`, "Session
//! lifecycle", "A session ended while it is creating").
//!
//! **Every failure lands in one place.** [`fail`] removes the container if one
//! was created, clears `container_id`, transitions the session to `failed` with
//! the message in `sessions.error`, forgets the registry entry (dropping the
//! queued inputs, ADR 0020) and runs the end-of-session hook, which releases a
//! task the launch claimed. Engine and git messages go into `sessions.error`
//! verbatim; a secret *value* never appears in one, and neither does a path.

use std::time::Duration;

use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::agent::LaunchMode as AgentLaunchMode;
use crate::agent::{
    AgentBackend, DEFAULT_MCP_CONFIG_PATH, InjectedCredential, LaunchContext, TranslateConfig,
    backend_for, credential_secret_names,
};
use crate::engine::spec::{SessionSpecInput, SharedDirMount, build_session_spec};
use crate::engine::{ContainerId, EngineError, LABEL_SESSION_ID};
use crate::events::{AgentEvent, AgentEventBody};
use crate::git::{
    CommitIdentity, DataPaths, GitActor, GitCommand, GitService, create_work_clone, resolve_base,
    session_branch,
};
// The model enum and the agent module's trait share the name `AgentBackend`;
// the enum is `Backend` here, as it is inside `agent/`.
use crate::models::AgentBackend as Backend;
use crate::models::{AgentProfile, Project, Session, SessionKind, SessionState, SharedDir};
use crate::prelude::*;
use crate::repositories::{ProjectRepository, SessionRepository, Transition, UserRepository};
use crate::secrets::{LaunchScope, ResolvedSecrets, resolve_for_launch};
use crate::session::{
    LaunchGuard, McpToken, OwnerContext, OwnerRx, Phase, SessionDirs, SessionOwner, SubmitResult,
    rotate_token, session_system_prompt, write_mcp_json,
};

/// How recent a mirror fetch has to be for a fresh launch to skip its own
/// (`ARCHITECTURE.md`, "Launch sequence": "skipped if fetched < 30 s ago").
///
/// Read from `projects.last_fetched_at` by the git layer's own fetch routine,
/// which is also what records it: the rule is one `max_age` argument here and
/// no second implementation of the fetch.
pub const FRESH_FETCH_MAX_AGE: Duration = Duration::from_secs(30);

/// The `state_change` reason for the transition into `running`.
///
/// It names the moment the state is decided by, because that is the part a
/// reader of the transcript is likely to be surprised by: the session is
/// running once stdin is attached, not once the CLI has said anything (ADR
/// 0032).
pub const LAUNCHED_REASON: &str = "container started and stdin attached";

/// The `state_change` reason every failed launch carries; the message that says
/// *what* failed is `sessions.error`.
pub const LAUNCH_FAILED_REASON: &str = "launch failed";

/// The `launch_warning` a session whose backend found no credential at any
/// scope records (`ARCHITECTURE.md`, "Secrets", Agent credentials; ADR 0036).
///
/// A warning and not a refusal: the stub image and an image that carries its
/// own authentication need none. It names the backend and nothing else — no
/// secret name the caller did not ask for, and no value (rule 3).
pub fn no_agent_credential_warning(backend: Backend) -> String {
    format!("no agent credential for backend {backend}; add one on the Secrets page")
}

/// Which launch this is.
///
/// `Fresh` carries the token the creating route generated before it inserted
/// the row, which is the only copy of it: the row holds the hash and the
/// launcher writes the value into `mcp.json` (ADR 0029). It also carries the
/// prompt an ephemeral session runs — the generated task message followed by
/// the user's message — which is not a column, because it is an argument of
/// this one process launch and not a property of the session.
///
/// Deliberately not `Debug`: `Fresh` holds a credential.
pub enum LaunchMode {
    /// A first start: write the given token's `mcp.json`, clone the work tree.
    Fresh {
        /// The token whose hash is already in `sessions.mcp_token_hash`.
        token: McpToken,
        /// An ephemeral session's whole prompt; `None` for a conversational
        /// one, which reads its messages from stdin.
        prompt: Option<String>,
    },
    /// A relaunch of a session that has a checkout: rotate the token, skip git,
    /// resume the CLI conversation if there is one.
    Resume,
}

impl LaunchMode {
    /// A fresh launch of a conversational session, which carries no prompt.
    pub fn fresh(token: McpToken) -> Self {
        Self::Fresh {
            token,
            prompt: None,
        }
    }

    /// Whether this is a resume, which is what the owner's `start_offset` and
    /// the `--resume` flag follow from.
    fn is_resume(&self) -> bool {
        matches!(self, Self::Resume)
    }
}

/// The asynchronous half of the session lifecycle: launches and resumes.
///
/// Holds the [`AppState`] rather than living in it, for the same reason
/// [`GitService`] does: every collaborator a launch needs — the pool, the
/// engine, the git locks, the registry, the keyring, the end-of-session hook —
/// is already there, and a field in the state pointing back at a type holding
/// the state would be a cycle. [`AppState::launcher`] is the one-call way to
/// get one, so the routes, the session service and recovery all launch through
/// the same implementation.
#[derive(Clone)]
pub struct Launcher {
    state: AppState,
}

impl std::fmt::Debug for Launcher {
    /// The state holds the keyring and the pool, so nothing about it is
    /// rendered.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Launcher").finish_non_exhaustive()
    }
}

impl Launcher {
    /// A launcher over `state`.
    pub fn new(state: AppState) -> Self {
        Self { state }
    }

    /// The launcher the handlers use, over the state they already hold.
    pub fn from_state(state: &AppState) -> Self {
        Self::new(state.clone())
    }

    /// Launch or resume `session_id`, on a task of its own.
    ///
    /// Returns as soon as the task is spawned, and `None` without spawning
    /// anything when another launch of the same session already holds the
    /// registry's [`LaunchGuard`] — which is what keeps a message to a parked
    /// session and an explicit resume from starting two containers.
    ///
    /// The returned handle is the launch task, which ends once the owner has
    /// been spawned or the session has been failed; a test awaits it, and
    /// production code drops it.
    pub fn launch(&self, session_id: Uuid, mode: LaunchMode) -> Option<JoinHandle<()>> {
        let Some(guard) = self.state.session_registry.try_begin_launch(session_id) else {
            info!(
                session_id = %session_id,
                "a launch of this session is already under way; not starting a second one",
            );
            return None;
        };

        let state = self.state.clone();
        Some(tokio::spawn(async move { run(state, guard, mode).await }))
    }
}

/// A launch failure, carrying exactly the text `sessions.error` will hold.
///
/// A newtype rather than the crate-wide [`Error`], because what a failed launch
/// owes the user is one sentence they can act on — the engine's own words for
/// an engine failure, a named ref for a base that does not resolve — and not
/// whatever a database driver would have said (`docs/data-model.md`,
/// `sessions.error`).
#[derive(Debug)]
struct Failure(String);

impl Failure {
    /// A failure whose message is `message`.
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

/// An internal fault during a launch: the detail goes to the log, a generic
/// sentence goes into `sessions.error` (`CLAUDE.md`, "Backend conventions").
fn internal(session_id: Uuid, what: &'static str, err: &dyn std::fmt::Display) -> Failure {
    error!(session_id = %session_id, error = %err, "{what}");
    Failure::new(what)
}

/// The launch task: read the row, register the session, run the sequence, and
/// route whatever it returns.
async fn run(state: AppState, guard: LaunchGuard, mode: LaunchMode) {
    let session_id = guard.session_id();

    let session = match SessionRepository::new(&state.pool).get(session_id).await {
        Ok(session) => session,
        Err(err) => {
            error!(
                session_id = %session_id,
                error = %err,
                "the session being launched could not be read; not launching it",
            );
            return;
        }
    };

    let from = session.state;
    if !matches!(from, SessionState::Creating | SessionState::Parked) {
        // Not a failure of this session: something else already moved it, and
        // the caller's read was stale.
        warn!(
            session_id = %session_id,
            state = %from,
            "not launching a session that is neither creating nor parked",
        );
        return;
    }

    // Registered before the first `await` that can take time, so an input
    // arriving during the launch queues instead of being refused. `register`
    // keeps an existing queue, which is how a message to a parked session is
    // still delivered by the resume it triggered.
    let commands = state
        .session_registry
        .register(session_id, session.kind, Phase::Creating);

    let mut created: Option<ContainerId> = None;
    match launch(&state, &session, from, mode, commands, &mut created).await {
        Ok(Launched::Running) => {}
        Ok(Launched::Cancelled) => cancelled(&state, session_id, created.as_ref()).await,
        Err(failure) => fail(&state, &session, from, &failure, created.as_ref()).await,
    }

    // Explicit, so the reason the guard lives this long is on the page: it is
    // released only once the owner is attached or the session has failed.
    drop(guard);
}

/// How a launch sequence that did not fail ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Launched {
    /// The container is up, the owner is attached and the session is
    /// `running`.
    Running,
    /// An `end` cancelled the launch at one of its checkpoints; the session is
    /// still `creating` and the `end` closes it.
    Cancelled,
}

/// The documented sequence. Every `Err` is a launch failure with its message.
async fn launch(
    state: &AppState,
    session: &Session,
    from: SessionState,
    mode: LaunchMode,
    commands: OwnerRx,
    created: &mut Option<ContainerId>,
) -> std::result::Result<Launched, Failure> {
    let session_id = session.id;
    let project_id = session.project_id;
    let sessions = SessionRepository::new(&state.pool);
    let projects = ProjectRepository::new(&state.pool);

    let project = projects
        .find(project_id)
        .await
        .map_err(|err| internal(session_id, "the project could not be read", &err))?
        .ok_or_else(|| Failure::new("the session's project no longer exists"))?;
    // `ON DELETE RESTRICT` keeps a profile alive while a session references it,
    // so this is unreachable through the API; a launch is still refused rather
    // than guessing a profile.
    let profile = projects
        .find_profile(project_id, session.profile_id)
        .await
        .map_err(|err| internal(session_id, "the agent profile could not be read", &err))?
        .ok_or_else(|| Failure::new("the session's agent profile no longer exists"))?;
    let mut shared_dirs = projects
        .list_shared_dirs(project_id)
        .await
        .map_err(|err| internal(session_id, "the shared directories could not be read", &err))?;
    SharedDir::sort_for_mount(&mut shared_dirs);
    let identity = launching_identity(state, session).await?;

    let dirs = SessionDirs::from_config(&state.config, session_id);
    dirs.ensure()
        .await
        .map_err(|err| internal(session_id, "the session directories are not usable", &err))?;

    let resuming = mode.is_resume();
    let prompt = match mode {
        // The hash is already the row's; only the file is missing.
        LaunchMode::Fresh { token, prompt } => {
            write_mcp_json(&dirs, &state.config.mcp_url, &token)
                .await
                .map_err(|err| {
                    internal(session_id, "the session MCP config was not written", &err)
                })?;
            prompt
        }
        // Before anything else a relaunch does: the replacement hash commits,
        // then the file is rewritten, and only then may a process start
        // (ADR 0029).
        LaunchMode::Resume => {
            rotate_token(&state.pool, &dirs, &state.config.mcp_url, session_id)
                .await
                .map_err(|err| {
                    internal(session_id, "the session MCP token was not rotated", &err)
                })?;
            None
        }
    };

    // Before the git work, which is the long half of a fresh launch: a session
    // ended this early never clones anything.
    if cancelled_at(state, session_id, "before the session clone") {
        return Ok(Launched::Cancelled);
    }

    // A resume keeps its checkout; one that has none is the retry of a session
    // that failed during creation and gets the fresh sequence.
    if !resuming || !dirs.work().join(".git").is_dir() {
        clone_work_tree(state, session, &project, &dirs, &identity).await?;
    }

    // Lazily, at every launch: a shared directory's row may be older than its
    // directory, and the CLI state directory is shared by the project's
    // sessions (ADR 0015; `ARCHITECTURE.md`, "Storage").
    let layout = state.config.project_layout(project_id);
    layout
        .ensure_created()
        .await
        .map_err(|err| internal(session_id, "the project directories are not usable", &err))?;
    for shared in &shared_dirs {
        layout
            .ensure_shared_dir(&shared.name)
            .await
            .map_err(|err| internal(session_id, "a shared directory is not usable", &err))?;
    }

    // The backend's own names, resolved for every session of it whether the
    // profile lists them or not (ADR 0036).
    let backend = backend_for(profile.backend);
    let credential_names = credential_secret_names(backend.as_ref());

    let resolved = resolve_for_launch(
        &state.pool,
        &state.keyring,
        LaunchScope {
            session_id,
            project_id,
            created_by: session.created_by,
        },
        // The profile yields validated names; the resolver checks none of its
        // own (`ARCHITECTURE.md`, "Secrets", Resolution at launch).
        &profile.secret_names(),
        &credential_names,
    )
    .await
    .map_err(|err| {
        internal(
            session_id,
            "the profile's secrets could not be resolved",
            &err,
        )
    })?;
    for message in &resolved.warnings {
        warn_on_session(state, session_id, message).await;
    }

    let credential = injected_credential(backend.as_ref(), &resolved);
    if credential.is_none() && !credential_names.is_empty() {
        // Not a refusal: a session can run without one, and the user is told
        // where to add it ("Secrets", Agent credentials).
        warn_on_session(
            state,
            session_id,
            &no_agent_credential_warning(profile.backend),
        )
        .await;
    }

    let spec = {
        let cmd = launch_command(session, &profile, prompt, resume_id(session, resuming))?;
        build_session_spec(&SessionSpecInput {
            session_id,
            project_id,
            profile_id: profile.id,
            task_id: session.task_id,
            image: profile.image.clone(),
            runtime: profile.runtime.clone(),
            cmd,
            // Moved, not copied: the values stay in the buffers the resolver
            // decrypted them into (`ARCHITECTURE.md`, "Secrets").
            secrets: resolved.env,
            shared_dirs: shared_dirs
                .iter()
                .map(|shared| SharedDirMount {
                    name: shared.name.clone(),
                    container_path: shared.container_path.clone(),
                })
                .collect(),
            data_dir: state.config.data_dir.clone(),
            data_dir_host: state.config.data_dir_host.clone(),
            network_internal: state.config.session_network_internal.clone(),
            extra_hosts: state.config.session_extra_hosts.clone(),
        })
        // A reserved-name collision: a configuration error, and its message
        // names the variable and never a value.
        .map_err(|err| Failure::new(err.to_string()))?
    };

    // The last moment at which no container of this launch exists: past it a
    // cancellation costs a create and a remove rather than nothing.
    if cancelled_at(state, session_id, "before the container was created") {
        return Ok(Launched::Cancelled);
    }

    // A crash between a create and a remove leaves a container holding this
    // session's name, which the next create would collide with.
    discard_stale_containers(state, session_id).await;

    if !state
        .engine
        .image_exists(&spec.image)
        .await
        .map_err(|err| Failure::new(format!("image pull failed: {}", engine_message(&err))))?
    {
        state
            .engine
            .pull_image(&spec.image)
            .await
            .map_err(|err| Failure::new(format!("image pull failed: {}", engine_message(&err))))?;
    }

    let container_id = state
        .engine
        .create(&spec)
        .await
        .map_err(|err| Failure::new(format!("container create failed: {err}")))?;
    // Recorded here, so a failure of any later step — or a restart — finds the
    // container through the row (`docs/data-model.md`, `sessions.container_id`).
    *created = Some(container_id.clone());
    set_container_id(state, session_id, Some(&container_id))
        .await
        .map_err(|err| internal(session_id, "the container id could not be stored", &err))?;

    state
        .engine
        .connect_network(&container_id, &state.config.session_network_egress)
        .await
        .map_err(|err| Failure::new(format!("connecting the egress network failed: {err}")))?;
    state
        .engine
        .start(&container_id)
        .await
        .map_err(|err| Failure::new(format!("container start failed: {err}")))?;
    // An ephemeral session's whole input is its argv, and its owner drops
    // anything sent to it, so it gets no stdin writer: the attach is an exec
    // (ADR 0034), and an exec into a run short enough to have ended already
    // would be refused and fail a launch that in fact succeeded.
    let stdin = match session.kind {
        SessionKind::Ephemeral => None,
        SessionKind::Conversational => Some(
            state
                .engine
                .attach_stdin(&container_id)
                .await
                .map_err(|err| Failure::new(format!("attaching stdin failed: {err}")))?
                as Box<dyn tokio::io::AsyncWrite + Send + Unpin>,
        ),
    };

    let start_offset = if resuming {
        sessions
            .max_offset(session_id)
            .await
            .map_err(|err| internal(session_id, "the transcript offset could not be read", &err))?
            .unwrap_or(0)
            .max(0) as u64
    } else {
        0
    };

    // The last checkpoint, and the one this whole mechanism is for: past the
    // transition the session is `running` with an owner attached, and the end
    // that is waiting stops it the ordinary way instead.
    if cancelled_at(state, session_id, "with its container created") {
        return Ok(Launched::Cancelled);
    }

    // Committed before the owner exists, because the owner of an adopted
    // process reads this very event to reconstruct what it took over
    // (`ARCHITECTURE.md`, "Durability and recovery").
    {
        let mut tx = state.pool.begin().await.map_err(|err| {
            internal(
                session_id,
                "the session could not be moved to running",
                &err,
            )
        })?;
        sessions
            .transition(
                &mut tx,
                session_id,
                &Transition::new(from, SessionState::Running, LAUNCHED_REASON),
            )
            .await
            .map_err(|err| {
                internal(
                    session_id,
                    "the session could not be moved to running",
                    &err,
                )
            })?;
        tx.commit().await.map_err(|err| {
            internal(
                session_id,
                "the session could not be moved to running",
                &err,
            )
        })?;
    }

    SessionOwner::spawn(OwnerContext {
        session_id,
        kind: session.kind,
        dirs,
        // The same adapter the credential names came from, shared rather than
        // built a second time.
        backend: Arc::clone(&backend),
        start_offset,
        translate: TranslateConfig {
            resumed: resume_id(session, resuming).is_some(),
            partial_messages: profile.partial_messages,
            credential,
        },
        adopted: false,
        stdin,
        container_id: Some(container_id.clone()),
        commands,
        state: state.clone(),
    });

    // The pinned CLI writes nothing until it has read a line, so the queue is
    // flushed now rather than on `init` (ADR 0032). In order, through the
    // registry, which is the only writer of the owner's channel.
    for input in state.session_registry.mark_running(session_id) {
        match state.session_registry.submit(session_id, input) {
            SubmitResult::Forwarded => {}
            outcome => warn!(
                session_id = %session_id,
                ?outcome,
                "an input that queued during the launch could not be delivered",
            ),
        }
    }

    info!(
        session_id = %session_id,
        project_id = %project_id,
        container_id = %container_id,
        "session launched",
    );

    Ok(Launched::Running)
}

/// Whether the `end` of a session that is still `creating` has cancelled this
/// launch, saying where the launch was when it noticed.
///
/// `where_it_stopped` is a fixed phrase per checkpoint, logged as a field so
/// the record says how far the sequence got.
fn cancelled_at(state: &AppState, session_id: Uuid, where_it_stopped: &'static str) -> bool {
    if !state.session_registry.launch_cancelled(session_id) {
        return false;
    }

    info!(
        session_id = %session_id,
        checkpoint = where_it_stopped,
        "the launch was cancelled by an end of the session",
    );
    true
}

/// The mirror fetch, the base resolution and the work clone: everything a fresh
/// launch does to git (`ARCHITECTURE.md`, "Git model", Session clone and
/// Serialization).
///
/// The fetch takes and releases the project git lock itself, inside the one
/// routine that also records `last_fetched_at` and applies the 30-second rule;
/// the lock this function takes covers the base resolution and the clone, which
/// is the acquisition the document calls for. No engine call happens under it.
async fn clone_work_tree(
    state: &AppState,
    session: &Session,
    project: &Project,
    dirs: &SessionDirs,
    identity: &CommitIdentity,
) -> std::result::Result<(), Failure> {
    let session_id = session.id;
    let paths = DataPaths::from_config(&state.config);
    let actor = match session.created_by {
        Some(user_id) => GitActor::User(user_id),
        None => GitActor::System,
    };

    match GitService::from_state(state)
        .fetch_project(project.id, &actor, Some(FRESH_FETCH_MAX_AGE))
        .await
    {
        Ok(outcome) => debug!(
            session_id = %session_id,
            project_id = %project.id,
            fetched = outcome.fetched,
            "the mirror is current for this launch",
        ),
        // An unreachable upstream is not a reason to refuse a session: the
        // mirror is whatever it already was and the launch says so
        // (`ARCHITECTURE.md`, "Launch sequence").
        Err(err) => {
            warn!(
                session_id = %session_id,
                project_id = %project.id,
                error = %err,
                "the mirror fetch before a launch failed; launching from the mirror as it is",
            );
            warn_on_session(state, session_id, &format!("mirror fetch failed: {err}")).await;
        }
    }

    // The retry of a launch that failed after the clone: the checkout is
    // already the one this session wants, so it is kept with whatever the agent
    // had written into it.
    if reusable_checkout(dirs, session_id).await {
        info!(
            session_id = %session_id,
            "reusing the work tree a previous launch attempt left on this session's branch",
        );
        return Ok(());
    }

    let guard = state.git_locks.lock(project.id).await;

    let base = resolve_base(
        &guard,
        &paths,
        Some(&session.base_ref),
        project.default_branch.as_deref().unwrap_or_default(),
    )
    .await
    .map_err(|err| {
        warn!(
            session_id = %session_id,
            git.base = %session.base_ref,
            error = %err,
            "a session's base ref does not resolve",
        );
        Failure::new(format!(
            "base ref '{}' does not resolve",
            session.base_ref.as_str()
        ))
    })?;

    // Removes whatever an interrupted attempt left in `work/` and clones again,
    // with the launching user's identity configured for the agent's commits.
    create_work_clone(&guard, &paths, session_id, &base, identity)
        .await
        .map_err(|err| Failure::new(format!("the session clone failed: {err}")))?;

    // Still under the git lock, which comes before the row's (ADR 0021), so an
    // end racing this launch reads the commit the clone was made at together
    // with the clone. A failed write is not a failed launch: without the
    // commit the end-of-session rule cannot tell a session with no commits of
    // its own, and keeps its ref unless an integration head or a hand-off
    // holds its tip, as for a session launched before the column (ADR 0050).
    if let Err(err) = record_base_commit(state, session_id, &base.commit).await {
        warn!(
            session_id = %session_id,
            error = %err,
            "the session's base commit could not be recorded",
        );
    }

    Ok(())
}

/// Write `sessions.base_commit`, in a transaction of its own.
async fn record_base_commit(state: &AppState, session_id: Uuid, commit: &str) -> Result<()> {
    let mut tx = state.pool.begin().await?;
    SessionRepository::new(&state.pool)
        .set_base_commit(&mut tx, session_id, commit)
        .await?;
    tx.commit().await?;

    Ok(())
}

/// Whether `work/` already holds this session's clone, checked out on its own
/// branch.
///
/// Both halves matter: a `.git` directory alone could be an interrupted clone,
/// and a checkout on another branch is not this session's work tree. Anything
/// else is thrown away and cloned again.
async fn reusable_checkout(dirs: &SessionDirs, session_id: Uuid) -> bool {
    let work = dirs.work();
    if !work.join(".git").is_dir() {
        return false;
    }

    let head = GitCommand::new()
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .cwd(&work)
        .run()
        .await;

    match head {
        Ok(output) if output.status == 0 => output.stdout.trim() == session_branch(session_id),
        // A repository git will not answer for is not one to launch into.
        _ => false,
    }
}

/// The identity the agent's commits inside the container carry: the launching
/// user's, or the bot's when the session has no user left
/// (`ARCHITECTURE.md`, "Git model", Commit identity).
async fn launching_identity(
    state: &AppState,
    session: &Session,
) -> std::result::Result<CommitIdentity, Failure> {
    if let Some(user_id) = session.created_by {
        let user = UserRepository::new(&state.pool)
            .find(user_id)
            .await
            .map_err(|err| internal(session.id, "the launching user could not be read", &err))?;

        if let Some(user) = user {
            return Ok(CommitIdentity {
                name: user.username,
                email: user.email,
            });
        }
    }

    state
        .git_credentials
        .commit_identity(session.project_id)
        .await
        .map_err(|err| internal(session.id, "the bot commit identity is unavailable", &err))
}

/// The container command for this launch, from the backend the profile names.
///
/// The launcher only assembles the inputs; which flags they become is the
/// backend's (`ARCHITECTURE.md`, "Claude Code invocation"). An ephemeral
/// session's prompt is an argument of the command, which is why a launch
/// without one is refused here rather than producing a process with nothing to
/// do (ADR 0003).
fn launch_command(
    session: &Session,
    profile: &AgentProfile,
    prompt: Option<String>,
    resume: Option<String>,
) -> std::result::Result<Vec<String>, Failure> {
    let mode = match session.kind {
        SessionKind::Ephemeral => AgentLaunchMode::Ephemeral {
            prompt: prompt.ok_or_else(|| Failure::new("an ephemeral session has no prompt"))?,
        },
        SessionKind::Conversational => AgentLaunchMode::Conversational { resume },
    };

    let ctx = LaunchContext {
        mode,
        model: profile.model.clone(),
        // The one place a launch's system prompt is composed, for every backend
        // and every launch path: Mars's git preamble, then the profile's own
        // prompt (`SPEC.md`, "Session preamble"; ADR 0055).
        system_prompt: Some(session_system_prompt(
            session.id,
            profile.system_prompt.as_deref(),
        )),
        partial_messages: profile.partial_messages,
        mcp_config_path: DEFAULT_MCP_CONFIG_PATH.to_string(),
    };

    Ok(backend_for(profile.backend).launch_command(&ctx).argv)
}

/// The `cli_session_id` this launch resumes, if any.
///
/// A resume of a session that never spoke has none, and relaunches fresh in the
/// existing checkout rather than failing: there is no conversation to continue
/// (ADR 0032; `ARCHITECTURE.md`, "Launch sequence").
fn resume_id(session: &Session, resuming: bool) -> Option<String> {
    if !resuming || session.kind == SessionKind::Ephemeral {
        return None;
    }

    session.cli_session_id.clone()
}

/// The agent credential this launch injected, as the translator describes it.
///
/// The resolver already decided which row wins the credential slot and says so
/// in [`ResolvedSecrets::credential`]; all that is left here is to give the
/// name back its backend's [`CredentialName`](crate::agent::CredentialName),
/// which is what the
/// authentication-failure event carries (`ARCHITECTURE.md`, "Claude Code
/// invocation", Credentials). The names are the backend's own and are never
/// enumerated here, and the value never reaches this function at all.
fn injected_credential(
    backend: &dyn AgentBackend,
    resolved: &ResolvedSecrets,
) -> Option<InjectedCredential> {
    let winner = resolved.credential.as_ref()?;

    backend
        .credential_names()
        .iter()
        .find(|name| name.as_str() == winner.name.as_str())
        .map(|name| InjectedCredential {
            name: *name,
            scope: winner.scope,
        })
}

/// Append one `launch_warning` to the session's transcript
/// (`SPEC.md`, "AgentEvent").
///
/// Its own transaction, and best effort: a warning that cannot be stored must
/// not turn a launch that is otherwise fine into a failure.
async fn warn_on_session(state: &AppState, session_id: Uuid, message: &str) {
    let appended = async {
        let mut tx = state.pool.begin().await?;
        SessionRepository::new(&state.pool)
            .append_event(
                &mut tx,
                session_id,
                &AgentEvent::new(AgentEventBody::LaunchWarning {
                    message: message.to_string(),
                }),
            )
            .await?;
        tx.commit().await?;
        Ok::<(), Error>(())
    }
    .await;

    if let Err(err) = appended {
        error!(
            session_id = %session_id,
            error = %err,
            "a launch warning could not be recorded",
        );
    }
}

/// Write `sessions.container_id`, in a transaction of its own.
async fn set_container_id(
    state: &AppState,
    session_id: Uuid,
    container_id: Option<&ContainerId>,
) -> Result<()> {
    let mut tx = state.pool.begin().await?;
    SessionRepository::new(&state.pool)
        .set_container_id(&mut tx, session_id, container_id.map(|id| id.0.as_str()))
        .await?;
    tx.commit().await?;

    Ok(())
}

/// Remove any container still carrying this session's label before a create.
///
/// The stale container of a crash between a create and a remove: it holds the
/// name `mars-session-<sid>`, so a create would be refused for a conflict that
/// has nothing to do with this launch. Forced, because it may still be running,
/// and best effort — a create that then fails reports the conflict itself.
async fn discard_stale_containers(state: &AppState, session_id: Uuid) {
    let listed = match state.engine.list_by_label(LABEL_SESSION_ID).await {
        Ok(listed) => listed,
        Err(err) => {
            warn!(
                session_id = %session_id,
                error = %err,
                "the engine could not be asked for this session's containers before a launch",
            );
            return;
        }
    };

    let label = session_id.to_string();
    for summary in listed
        .iter()
        .filter(|summary| summary.labels.get(LABEL_SESSION_ID) == Some(&label))
    {
        match state.engine.remove(&summary.id, true).await {
            Ok(()) | Err(EngineError::NotFound(_)) => warn!(
                session_id = %session_id,
                container_id = %summary.id,
                "removed a container a previous launch of this session left behind",
            ),
            Err(err) => warn!(
                session_id = %session_id,
                container_id = %summary.id,
                error = %err,
                "a container left behind by a previous launch could not be removed",
            ),
        }
    }
}

/// Everything a cancelled launch owes the session: the container, and nothing
/// else.
///
/// The `end` that cancelled it is waiting for this task to release the launch
/// claim and then writes the `creating → done` transition, forgets the registry
/// entry and runs the end-of-session hook. Doing any of that here as well would
/// close the session twice and release its tasks twice, so all this does is
/// leave no container behind and clear the `container_id` the create recorded
/// (`ARCHITECTURE.md`, "Session lifecycle", "A session ended while it is
/// creating").
async fn cancelled(state: &AppState, session_id: Uuid, created: Option<&ContainerId>) {
    let Some(container_id) = created else {
        info!(session_id = %session_id, "the cancelled launch had created no container");
        return;
    };

    match state.engine.remove(container_id, true).await {
        Ok(()) | Err(EngineError::NotFound(_)) => info!(
            session_id = %session_id,
            container_id = %container_id,
            "removed the container of a cancelled launch",
        ),
        // The row keeps pointing at it, so the `end` that is waiting removes it
        // once this task lets go of the session: a failure here is not the last
        // word.
        Err(err) => {
            warn!(
                session_id = %session_id,
                container_id = %container_id,
                error = %err,
                "the container of a cancelled launch could not be removed; \
                 leaving it to the end that cancelled it",
            );
            return;
        }
    }

    if let Err(err) = set_container_id(state, session_id, None).await {
        error!(
            session_id = %session_id,
            error = %err,
            "the container id of a cancelled launch could not be cleared",
        );
    }
}

/// Everything a failed launch owes the session.
///
/// The container first, so nothing is left running for a session the row says
/// is failed; then the state change, which is what the UI reads; then the
/// registry entry, whose queued inputs go with it (ADR 0020); then the
/// end-of-session hook, which releases a task the launch claimed.
async fn fail(
    state: &AppState,
    session: &Session,
    from: SessionState,
    failure: &Failure,
    created: Option<&ContainerId>,
) {
    let session_id = session.id;

    if let Some(container_id) = created {
        match state.engine.remove(container_id, true).await {
            Ok(()) | Err(EngineError::NotFound(_)) => {}
            Err(err) => warn!(
                session_id = %session_id,
                container_id = %container_id,
                error = %err,
                "the container of a failed launch could not be removed",
            ),
        }

        if let Err(err) = set_container_id(state, session_id, None).await {
            error!(
                session_id = %session_id,
                error = %err,
                "the container id of a failed launch could not be cleared",
            );
        }
    }

    let change = Transition::new(from, SessionState::Failed, LAUNCH_FAILED_REASON)
        .with_error(failure.0.as_str());
    let failed = async {
        let mut tx = state.pool.begin().await?;
        SessionRepository::new(&state.pool)
            .transition(&mut tx, session_id, &change)
            .await?;
        tx.commit().await?;
        Ok::<(), Error>(())
    }
    .await;

    if let Err(err) = failed {
        // The row is left as it was; the next start's recovery fails a session
        // stuck in `creating` (`ARCHITECTURE.md`, "Restart procedure").
        error!(
            session_id = %session_id,
            error = %err,
            "a failed launch could not be recorded on the session",
        );
    }

    state.session_registry.remove(session_id);
    state.session_ended(session_id).await;

    error!(
        session_id = %session_id,
        project_id = %session.project_id,
        reason = %failure.0,
        "session launch failed",
    );
}

/// The engine's own words for a failure, without the wrapper this crate adds.
///
/// `sessions.error` says `image pull failed: <what the registry said>`, so a
/// pull failure contributes the registry's message and not a second copy of the
/// sentence around it. An [`EngineError`] never carries a secret (rule 3).
fn engine_message(err: &EngineError) -> String {
    match err {
        EngineError::ImagePull { message, .. } => message.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use zeroize::Zeroizing;

    use super::*;
    use crate::agent::CredentialName;
    use crate::models::{SecretName, SecretScope};
    use crate::secrets::ResolvedCredential;

    /// An obviously fake value; nothing here is a credential (rule 3).
    fn resolution(
        names: &[(&str, SecretScope)],
        credential: Option<(&str, SecretScope)>,
    ) -> ResolvedSecrets {
        ResolvedSecrets {
            env: names
                .iter()
                .map(|(name, _)| {
                    (
                        (*name).to_string(),
                        Zeroizing::new("not-a-real-value".to_string()),
                    )
                })
                .collect(),
            scopes: names
                .iter()
                .map(|(name, scope)| ((*name).to_string(), *scope))
                .collect(),
            warnings: Vec::new(),
            skipped: Vec::new(),
            credential: credential.map(|(name, scope)| ResolvedCredential {
                name: SecretName::parse(name).expect("the test credential name is valid"),
                scope,
            }),
        }
    }

    #[test]
    fn the_credential_is_reported_with_the_scope_it_resolved_at() {
        let resolved = resolution(
            &[("DEPLOY_TOKEN", SecretScope::Project)],
            Some(("CLAUDE_CODE_OAUTH_TOKEN", SecretScope::User)),
        );

        let credential =
            injected_credential(backend_for(Backend::Claude).as_ref(), &resolved).expect("found");

        assert_eq!(credential.name, CredentialName::ClaudeCodeOauthToken);
        assert_eq!(credential.scope, SecretScope::User);
    }

    #[test]
    fn a_resolution_with_no_credential_reports_none() {
        let resolved = resolution(&[("DEPLOY_TOKEN", SecretScope::Global)], None);

        assert!(injected_credential(backend_for(Backend::Claude).as_ref(), &resolved).is_none());
    }

    #[test]
    fn a_credential_of_another_backend_is_not_this_ones() {
        // The resolver is handed the launching backend's names, so this cannot
        // arise from a launch; the mapping back to a `CredentialName` is still
        // the backend's own list and nothing else.
        let resolved = resolution(&[], Some(("DEPLOY_TOKEN", SecretScope::Global)));

        assert!(injected_credential(backend_for(Backend::Claude).as_ref(), &resolved).is_none());
    }

    #[test]
    fn the_missing_credential_warning_names_the_backend_and_nothing_else() {
        let message = no_agent_credential_warning(Backend::Claude);

        assert_eq!(
            message,
            "no agent credential for backend claude; add one on the Secrets page"
        );
        for name in crate::agent::all_credential_names() {
            assert!(!message.contains(name.as_str()), "{message}");
        }
    }

    #[test]
    fn the_engine_message_of_a_pull_failure_is_the_registrys_own() {
        assert_eq!(
            engine_message(&EngineError::ImagePull {
                image: "mars-session-claude:dev".to_string(),
                message: "manifest unknown".to_string(),
            }),
            "manifest unknown",
        );
        assert_eq!(
            engine_message(&EngineError::NotFound("mock-1".to_string())),
            "the container engine has no such object: mock-1",
        );
    }

    #[test]
    fn only_a_conversational_resume_with_a_recorded_id_passes_resume() {
        let mut session = crate::models::Session {
            id: Uuid::new_v4(),
            project_id: Uuid::new_v4(),
            profile_id: Uuid::new_v4(),
            kind: SessionKind::Conversational,
            created_by: None,
            launch_source: crate::models::SessionLaunchSource::User,
            title: None,
            task_id: None,
            handoff_id: None,
            state: SessionState::Parked,
            base_ref: "refs/heads/main".to_string(),
            branch: "session/x".to_string(),
            container_id: None,
            cli_session_id: Some("fake-cli-session".to_string()),
            mcp_token_hash: String::new(),
            last_seq: 0,
            last_activity_at: chrono::Utc::now(),
            cost_usd: 0.0,
            input_tokens: 0,
            output_tokens: 0,
            error: None,
            created_at: chrono::Utc::now(),
            parked_at: None,
            ended_at: None,
        };

        assert_eq!(
            resume_id(&session, true).as_deref(),
            Some("fake-cli-session"),
        );
        assert_eq!(
            resume_id(&session, false),
            None,
            "a fresh launch never resumes"
        );

        session.cli_session_id = None;
        assert_eq!(
            resume_id(&session, true),
            None,
            "a session that never spoke has nothing to resume",
        );
    }
}
