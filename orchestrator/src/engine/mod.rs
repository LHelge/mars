//! The `ContainerEngine` trait, its bollard implementation, the mock behind
//! the `integration-tests` feature, and the startup sequence
//! [`bootstrap_engine`] runs over an engine — the session networks and the
//! startup probe.
//!
//! The trait is the boundary: everything a session needs from a container
//! engine is expressed here in the plain types of [`types`], so `HostConfig`,
//! `bollard` and the difference between Podman and Docker stay inside this
//! module (ADR 0004; `ARCHITECTURE.md`, "Engine adapter"). The operations are
//! exactly the compatible subset that table verifies on both engines; adding
//! one means verifying it on both and recording it there first.
//!
//! **Async style.** The three collaborator traits (`ContainerEngine`,
//! [`EmailClient`](crate::email::EmailClient) and
//! [`GitCredentialProvider`](crate::git::GitCredentialProvider)) are held as
//! `Arc<dyn Trait>` in [`AppState`], so they must be dyn compatible. They use
//! `#[async_trait::async_trait]` rather than a native `async fn` in a trait;
//! every epic that extends them keeps that choice so the trait objects stay
//! usable.
//!
//! **Secrets.** A session's resolved secrets reach the engine in
//! [`ContainerSpec::secret_env`] and nowhere else, still in the `Zeroizing`
//! buffers the resolver decrypted them into;
//! [`spec::to_bollard`] is the one place they are copied into plain bytes and
//! is where that guarantee ends. A spec is never `Debug`-printed into an error
//! or logged at `info` or above; its own [`Debug`] prints env keys only, so
//! even a careless `{:?}` cannot leak one (CLAUDE.md rule 3).

use std::any::Any;
use std::fs::{self, DirBuilder, OpenOptions};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
// Shadows the prelude's one-parameter `Result<T>` alias: every engine
// operation fails with an `EngineError`, not with the crate-wide `Error`, so
// that callers can match on the variant before deciding what it means.
use std::result::Result;
use std::time::{Duration, Instant};

use async_trait::async_trait;

// The crate convention (`CLAUDE.md`, "Backend conventions"). The engine
// reports its own [`EngineError`] rather than the crate-wide one, so the glob
// is here for [`Config`], [`Arc`] and the `tracing` macros the bootstrap and
// the startup probe use, and for the doc links.
use crate::prelude::*;

pub mod bollard;
pub mod error;
pub mod spec;
mod streams;
pub mod types;

#[cfg(feature = "integration-tests")]
pub mod mock;

// `self::`, because a plain `bollard::` in this one module is the submodule
// below and not the crate (see `bollard`'s own module documentation).
use self::bollard::BollardEngine;
use self::spec::{ProbeSpecInput, build_probe_spec};

pub use error::EngineError;
pub use types::{
    Bind, ContainerId, ContainerInfo, ContainerSpec, ContainerState, ContainerSummary, EngineKind,
    ExecSession, ExitStatus, LABEL_PROFILE_ID, LABEL_PROJECT_ID, LABEL_SESSION_ID, STDIN_FIFO,
    Signal, StdinWriter, session_container_name,
};

/// The container engine, behind a trait so the API can be tested without one.
///
/// `ARCHITECTURE.md`, "Orchestrator internals": [`AppState`] holds this as
/// `Arc<dyn ContainerEngine>`, and the `integration-tests` feature supplies
/// `mock::MockEngine`.
///
/// Every method answers with an [`EngineError`] rather than the crate-wide
/// [`Error`], because callers branch on the variant — most of all on
/// [`EngineError::NotFound`], which is how recovery tells "the container is
/// gone" from "the engine is unhappy".
///
/// **The normalised semantics are part of the interface.** Docker and Podman
/// number the same refusal differently, and a retry, a recovery or a relaunch
/// asks for the same operation twice, so what an operation answers for a
/// container that is missing, one that has exited and one that is already
/// running is decided here rather than at each call site. Each method's
/// documentation states its rule in the words of `ARCHITECTURE.md`, "Engine
/// adapter", Normalised semantics, and
/// `orchestrator/tests/common/engine_contract.rs` is the executable definition:
/// `assert_engine_contract` runs one scenario per rule against any
/// `Arc<dyn ContainerEngine>`, `tests/engine.rs` runs it against
/// [`BollardEngine`] under `DOCKER_HOST` and `tests/engine_mock.rs` against
/// `mock::MockEngine` on every run of the test suite. An implementation that
/// does not pass the suite is not an implementation of this trait.
#[async_trait]
pub trait ContainerEngine: Send + Sync {
    /// Which engine is behind the socket, detected once when the adapter
    /// connected. It decides whether `UsernsMode: keep-id` is set (ADR 0004).
    fn kind(&self) -> EngineKind;

    /// Whether the engine answers at all. `GET /api/health` reports the result
    /// as `engine` (`SPEC.md`, "Health").
    ///
    /// Reachability and nothing else; every failure is
    /// [`EngineError::Connection`], whatever the engine said.
    async fn ping(&self) -> Result<(), EngineError>;

    /// Create the named network if it does not exist, and succeed if it does.
    ///
    /// `internal` is the sessions network, which has no route off the host;
    /// the egress network is created without it (`ARCHITECTURE.md`,
    /// "Networking").
    ///
    /// A network that already exists is `Ok` and is left exactly as it is,
    /// including when its own flags disagree with what was asked for (a warning,
    /// never a failure) and when a concurrent creation is what made it exist.
    async fn ensure_network(&self, name: &str, internal: bool) -> Result<(), EngineError>;

    /// Whether the image is present locally, so the launcher can skip a pull.
    ///
    /// An absent image is `Ok(false)`, never an error; every other failure
    /// propagates instead of being reported as absence.
    async fn image_exists(&self, image: &str) -> Result<bool, EngineError>;

    /// Pull the image, waiting for the pull to finish.
    ///
    /// Every failure is [`EngineError::ImagePull`], including one the engine
    /// reports inside an otherwise successful response stream; the launcher
    /// puts that message into `sessions.error` and fails the launch.
    async fn pull_image(&self, image: &str) -> Result<(), EngineError>;

    /// Create a container from the spec, without starting it.
    ///
    /// The container is created on [`ContainerSpec::network`]; the egress
    /// network is a second [`connect_network`](Self::connect_network) before
    /// [`start`](Self::start), so the container never runs with the wrong set.
    ///
    /// A name already in use is [`EngineError::Conflict`]; an image the engine
    /// does not have is [`EngineError::NotFound`] naming the image. At most one
    /// create per engine host is in flight (`ARCHITECTURE.md`, "Engine
    /// adapter", the `UsernsMode` row).
    async fn create(&self, spec: &ContainerSpec) -> Result<ContainerId, EngineError>;

    /// Attach a created container to a second network.
    ///
    /// A container already on the network is `Ok`, so connecting is idempotent;
    /// a missing container or network is [`EngineError::NotFound`].
    async fn connect_network(&self, id: &ContainerId, network: &str) -> Result<(), EngineError>;

    /// Start a created container.
    ///
    /// A container that is already running is `Ok`: both engines answer 304 and
    /// the adapter reads it as success, so a start that races another start
    /// cannot fail on it. A missing container is [`EngineError::NotFound`].
    async fn start(&self, id: &ContainerId) -> Result<(), EngineError>;

    /// Stop a container, letting it have `grace_secs` before the engine's own
    /// hard kill. Mars's own stop sequence is [`kill`](Self::kill) with
    /// [`Signal::Sigint`] then [`Signal::Sigterm`] (`ARCHITECTURE.md`, "Stop
    /// semantics"); this is the plain stop used when ending a session.
    ///
    /// A container that is not running — one that has already exited, or one
    /// that was never started — is `Ok`, because being stopped is what the
    /// caller asked for and it already is: both engines answer 304 and the
    /// adapter reads it as success. A missing container is
    /// [`EngineError::NotFound`].
    async fn stop(&self, id: &ContainerId, grace_secs: u32) -> Result<(), EngineError>;

    /// Send a named signal to the container's main process.
    ///
    /// A container that has exited is [`EngineError::Conflict`], which the
    /// session owner reads as "it is already gone" rather than as a failure; a
    /// missing container is [`EngineError::NotFound`]. The signal reaches the
    /// container's main process without `Init: true` (`ARCHITECTURE.md`,
    /// "Session image").
    async fn kill(&self, id: &ContainerId, signal: Signal) -> Result<(), EngineError>;

    /// Remove a container. `force` removes it even while running.
    ///
    /// A missing container is `Ok`, because a container that is not there is
    /// already removed; a running container is [`EngineError::Conflict`]
    /// without `force` and `Ok` with it, although Docker refuses the unforced
    /// removal with 409 and Podman with 500.
    async fn remove(&self, id: &ContainerId, force: bool) -> Result<(), EngineError>;

    /// Everything the engine knows about one container.
    ///
    /// A missing container is [`EngineError::NotFound`], which is the answer
    /// recovery reads as "the container is gone" and parks the session on.
    async fn inspect(&self, id: &ContainerId) -> Result<ContainerInfo, EngineError>;

    /// Wait for the container to exit and report how it ended.
    ///
    /// A container that has already exited answers immediately with its exit
    /// code, which is what makes this safe to call on a container a restart
    /// readopted; a non-zero exit code is not a failure (130 and 143 are
    /// ordinary stops); a missing container is [`EngineError::NotFound`].
    async fn wait(&self, id: &ContainerId) -> Result<ExitStatus, EngineError>;

    /// Every container carrying the label, running or not. Recovery lists
    /// [`LABEL_SESSION_ID`] to find the containers it owns.
    ///
    /// A key nothing carries is an empty list, not an error.
    async fn list_by_label(&self, label_key: &str) -> Result<Vec<ContainerSummary>, EngineError>;

    /// A writer to the stdin of the container's main process.
    ///
    /// Not an attach to the container's own stdin: the adapter starts an exec
    /// that relays into [`STDIN_FIFO`], which the image's entrypoint made and
    /// the CLI holds open read-write, so dropping the writer — or losing the
    /// orchestrator — ends the relay and is never an EOF for the CLI, and a
    /// later call reaches the same process (ADR 0034; `ARCHITECTURE.md`,
    /// "Restart procedure"). Only stdin: the CLI's output is read from the
    /// transcript file, not the socket (`ARCHITECTURE.md`, "Agent process
    /// model"). The session owner is the only holder of the returned writer.
    ///
    /// A missing container is [`EngineError::NotFound`] and one with no FIFO to
    /// relay into is [`EngineError::Unsupported`]; a write after the
    /// container exited returns an error, never a silent success, because a
    /// rootless Podman accepts and discards such writes (`ARCHITECTURE.md`,
    /// "Engine adapter", the stdin row).
    async fn attach_stdin(&self, id: &ContainerId) -> Result<Box<dyn StdinWriter>, EngineError>;

    /// Start an exec with a PTY: the terminal view.
    ///
    /// `SPEC.md`, "WebSocket: session stream": `/bin/bash -l` as `agent`, at
    /// the client's initial `cols` and `rows`.
    ///
    /// A container that is not running is [`EngineError::Conflict`] on both
    /// engines, although Docker answers 409 and Podman 500; a missing container
    /// is [`EngineError::NotFound`].
    async fn exec_pty(
        &self,
        id: &ContainerId,
        cmd: &[String],
        user: &str,
        cols: u16,
        rows: u16,
    ) -> Result<Box<dyn ExecSession>, EngineError>;

    /// Downcast hook, so a test that injected a concrete engine can read back
    /// what the handler did with it.
    fn as_any(&self) -> &dyn Any;
}

/// Connect to the engine, create the session networks and run the startup
/// probe: everything the orchestrator does between the migrations and binding
/// its listeners.
///
/// The order is the documented one (`ARCHITECTURE.md`, "Orchestrator
/// internals", "Networks" and "Restart procedure"): the networks exist before
/// any container is created on them, and the probe runs before any session is
/// adopted. A failure here is fatal at startup — the caller logs it once and
/// exits without binding a port — because an unreachable socket or a uid
/// mapping that does not hold would otherwise surface as every session failing
/// later, in less obvious ways.
///
/// It is one function rather than three calls in `main.rs` so that the engine
/// test suite drives exactly the sequence the binary does, on both engines.
///
/// The probe is a startup gate and nothing else: no health check, no reconnect
/// and no later launch runs it again.
///
/// # Errors
///
/// [`EngineError::Connection`] when the socket `DOCKER_HOST` names cannot be
/// reached, [`EngineError::Probe`] when the probe container ran but did not
/// prove what it has to prove, and whatever the engine answered when a network
/// could not be created. Nothing here logs a failure; the caller does, once.
pub async fn bootstrap_engine(config: &Config) -> Result<Arc<dyn ContainerEngine>, EngineError> {
    // `connect` writes the one startup line that carries the engine kind, its
    // version and the socket path (rule 3: the path appears there and nowhere
    // else), so nothing is logged again here.
    let engine = BollardEngine::connect(&config.docker_host).await?;

    bootstrap_connected(&engine, config).await?;

    Ok(Arc::new(engine))
}

/// Everything [`bootstrap_engine`] does once it has an engine: the networks and
/// then the probe, in that order.
///
/// It is separate only so that the steps past the connection can be driven over
/// a `MockEngine` — the tests at the bottom of this module are the probe's own
/// coverage, and the live version of the same sequence is
/// `tests/engine.rs::bootstrap_engine_end_to_end`.
async fn bootstrap_connected(
    engine: &dyn ContainerEngine,
    config: &Config,
) -> Result<(), EngineError> {
    // Created if missing and left alone if they exist. The sessions network is
    // internal — no route off the host — and the egress one is how a session
    // reaches the world (`ARCHITECTURE.md`, "Networks"). An existing network
    // whose `internal` flag disagrees is a warning from the adapter, not a
    // startup failure: the operator is told, and the orchestrator carries on.
    engine
        .ensure_network(&config.session_network_internal, true)
        .await?;
    engine
        .ensure_network(&config.session_network_egress, false)
        .await?;
    info!(
        internal = %config.session_network_internal,
        egress = %config.session_network_egress,
        "session networks ready"
    );

    // The same `HostConfig` a session gets, over the same data directory: what
    // passes here is what a launch can rely on.
    run_startup_probe(engine, config).await
}

// ---- the startup probe ----------------------------------------------------
//
// A short-lived container run with the same `HostConfig` a session would get,
// proving that a file it writes under `DATA_DIR` comes back owned by the
// orchestrator's own uid.
//
// It is what verifies that Podman honours `keep-id` and that the bind mounts
// and uid layout are sane on either engine (`ARCHITECTURE.md`, "Engine
// adapter", Startup probe; ADR 0004). A failure is [`EngineError::Probe`] and
// is fatal at startup, because every session would otherwise fail later in
// less obvious ways.
//
// **Why the file is enough.** The probe container runs as uid 1000 inside its
// user namespace and touches one file on a bind mount the orchestrator can
// see. Three things can be wrong with the result and each has its own message:
// the file is missing, which on macOS means `DATA_DIR_HOST` is not shared with
// the engine's VM; it is there but owned by somebody else, which under
// rootless Podman means `keep-id:uid=1000,gid=1000` was not honoured and the
// owner is a sub-uid; or it is owned correctly and still not writable. Only
// the owning uid is compared — the group may legitimately differ under
// `keep-id`.
//
// **Who owns "the orchestrator's uid".** The probe directory the orchestrator
// created itself a moment earlier is the reference, so the answer needs no
// `libc` call: whoever owns that directory is who the orchestrator runs as.
//
// **Secrets.** The probe spec carries no environment at all (see
// `build_probe_spec`), so nothing here can log one (CLAUDE.md rule 3).

/// How long the probe container is given to touch one file and exit.
///
/// Generous on purpose: the container is `touch` and nothing else, so the
/// whole budget is the engine's own create-and-start latency on a loaded
/// machine. Reaching it means the engine is wedged, not that the probe is
/// slow.
const PROBE_TIMEOUT: Duration = Duration::from_secs(60);

/// The file the probe container touches, relative to its `work` directory.
///
/// The container side of this name is the `cmd` in
/// [`build_probe_spec`](spec::build_probe_spec) — `touch
/// /session/work/probe-ok` — and the two have to agree: that bind's host
/// source is the directory read here.
const PROBE_FILE: &str = "probe-ok";

/// The subdirectories of a probe directory, which are the session directories
/// the spec binds into the container.
///
/// `home` and `log` are mounted although the probe writes only under `work`,
/// because the image's entrypoint redirects its output into `/session/log` and
/// takes `/session/home` as `HOME` before it runs anything: a probe that
/// mounted only `work` would not be running what a session runs
/// (`ARCHITECTURE.md`, "Session container specification").
const PROBE_SUBDIRS: [&str; 3] = ["work", "home", "log"];

/// The mode the probe directory itself is created with, matching the session
/// directories the launcher creates.
///
/// It stays the ordinary mode because nothing writes into it: the check reads
/// the orchestrator's own uid off this directory and the container writes one
/// level down, in [`PROBE_SUBDIR_MODE`].
const PROBE_DIR_MODE: u32 = 0o755;

/// The mode the three session subdirectories are given after creation.
///
/// World-writable on purpose, so that the check measures ownership and not
/// permission (`ARCHITECTURE.md`, "Engine adapter", Startup probe). Under
/// Docker the container's uid 1000 is uid 1000 on the host, so against a
/// 0o755 directory owned by an orchestrator running as some other uid the
/// container cannot write its file at all and the probe reports an exit code
/// instead of the uid mismatch that actually caused it. These directories are
/// throwaway — created under `DATA_DIR/tmp`, removed after the probe, swept by
/// orphan cleanup if the process dies — so the mode weakens nothing.
const PROBE_SUBDIR_MODE: u32 = 0o777;

/// Run the startup probe.
///
/// Creates `DATA_DIR/tmp/probe-<random>/{work,home,log}`, runs one container
/// from `config.session_image_default` over it with the session `HostConfig`,
/// and checks the file that container wrote. The container is removed on every
/// path, the directory is removed afterwards, and neither removal failing turns
/// a pass into a failure — `/data/tmp` and stray `mars.probe` containers are
/// what orphan cleanup is for (`ARCHITECTURE.md`, "Background jobs").
///
/// Both data paths of `Config` are used and they are not the same directory
/// seen twice: `data_dir` is where the orchestrator itself looks at the file
/// afterwards, and `data_dir_host` is what the engine resolves the bind-mount
/// source against (`ARCHITECTURE.md`, "Storage"). They are equal only when the
/// orchestrator runs on the host.
///
/// A pass logs one `info!` line. A failure is returned, not logged: the caller
/// is startup, which logs it once at `error!` and stops.
///
/// # Errors
///
/// [`EngineError::Probe`] when the container ran but did not prove what it has
/// to prove — a non-zero exit, a timeout, a missing file, a file owned by
/// another uid, a file that cannot be appended to, or a pull that failed. Any
/// other variant is the engine itself failing and is returned unchanged, so
/// the operator sees "the engine is unreachable" as that rather than as a
/// probe verdict.
async fn run_startup_probe(
    engine: &dyn ContainerEngine,
    config: &Config,
) -> Result<(), EngineError> {
    let started = Instant::now();
    // 64 bits of randomness names both the directory and the container, so two
    // orchestrators sharing a data directory and an engine cannot collide on
    // either.
    let suffix = format!("{:016x}", rand::random::<u64>());
    let probe_dir = probe_dir_in(&config.data_dir, &suffix);

    create_probe_dirs(&probe_dir)?;
    let outcome = run_probe_container(engine, config, &suffix, &probe_dir).await;

    // Unconditional: a failed probe stops startup, and leaving its directory
    // behind would only grow `/data/tmp` across restart attempts. A directory
    // whose contents the orchestrator may not own — precisely the uid-mismatch
    // failure — cannot be removed, which is a `warn!` and not the verdict.
    if let Err(error) = fs::remove_dir_all(&probe_dir) {
        warn!(
            path = %probe_dir.display(),
            %error,
            "could not remove the startup probe directory"
        );
    }

    // On success the orchestrator's uid and the file's are equal by definition:
    // the check is what returned the value.
    let uid = outcome?;

    info!(
        engine_kind = %engine.kind(),
        image = %config.session_image_default,
        uid,
        duration_ms = started.elapsed().as_millis() as u64,
        "startup probe passed"
    );
    Ok(())
}

/// Everything between the directory being there and the verdict, so that
/// [`run_startup_probe`] can clean the directory up on every path with one
/// `if let`.
///
/// Returns the uid that owns both the probe directory and the file the
/// container wrote.
async fn run_probe_container(
    engine: &dyn ContainerEngine,
    config: &Config,
    suffix: &str,
    probe_dir: &Path,
) -> Result<u32, EngineError> {
    ensure_probe_image(engine, &config.session_image_default).await?;

    let spec = build_probe_spec(&ProbeSpecInput {
        suffix: suffix.to_string(),
        image: config.session_image_default.clone(),
        probe_dir_host: probe_dir_in(&config.data_dir_host, suffix),
        network_internal: config.session_network_internal.clone(),
        extra_hosts: config.session_extra_hosts.clone(),
    });

    let id = engine.create(&spec).await?;
    let outcome = run_probe_to_exit(engine, &id, &config.session_network_egress).await;

    // Before the verdict and on every path, including the ones that never
    // started it: a probe container that survives its probe is an orphan.
    match engine.remove(&id, true).await {
        // Already gone is already removed.
        Ok(()) | Err(EngineError::NotFound(_)) => {}
        Err(error) => warn!(
            container_id = %id,
            %error,
            "could not remove the startup probe container; orphan cleanup will take it"
        ),
    }

    outcome?;
    check_probe_file(probe_dir, &probe_dir.join("work").join(PROBE_FILE))
}

/// Connect the egress network, start the container and wait for it, with the
/// same network order a session launch uses: the container never runs with the
/// wrong set of networks.
async fn run_probe_to_exit(
    engine: &dyn ContainerEngine,
    id: &ContainerId,
    network_egress: &str,
) -> Result<(), EngineError> {
    engine.connect_network(id, network_egress).await?;
    engine.start(id).await?;

    match tokio::time::timeout(PROBE_TIMEOUT, engine.wait(id)).await {
        Ok(Ok(status)) if status.code == 0 => Ok(()),
        Ok(Ok(status)) => Err(EngineError::Probe(format!(
            "probe container exited with code {}",
            status.code
        ))),
        Ok(Err(error)) => Err(error),
        Err(_elapsed) => {
            // The removal that follows is forced and would kill it anyway;
            // signalling first means the engine is asked to end a process it
            // still believes in, which is the friendlier order and the one
            // whose failure is worth a line.
            if let Err(error) = engine.kill(id, Signal::Sigkill).await {
                warn!(
                    container_id = %id,
                    %error,
                    "could not kill the timed-out startup probe container"
                );
            }
            Err(EngineError::Probe(format!(
                "probe container did not exit within {}s",
                PROBE_TIMEOUT.as_secs()
            )))
        }
    }
}

/// Pull the image if the engine does not have it, which is the ordinary state
/// of a first start.
///
/// A pull failure becomes a probe failure carrying the registry's own words,
/// because at startup there is no session row to put them in and the operator
/// needs to read "manifest unknown" rather than "the probe failed".
async fn ensure_probe_image(engine: &dyn ContainerEngine, image: &str) -> Result<(), EngineError> {
    if engine.image_exists(image).await? {
        return Ok(());
    }

    info!(%image, "pulling default session image");
    match engine.pull_image(image).await {
        Ok(()) => Ok(()),
        Err(EngineError::ImagePull { message, .. }) => Err(EngineError::Probe(format!(
            "probe image pull failed: {message}"
        ))),
        Err(error) => Err(error),
    }
}

/// `<data>/tmp/probe-<suffix>`, in whichever of the two views of the data
/// directory is passed.
fn probe_dir_in(data: &Path, suffix: &str) -> PathBuf {
    data.join("tmp").join(format!("probe-{suffix}"))
}

/// Create the probe directory and its three session subdirectories, and
/// `DATA_DIR/tmp` along the way.
fn create_probe_dirs(probe_dir: &Path) -> Result<(), EngineError> {
    let mut builder = DirBuilder::new();
    builder.recursive(true).mode(PROBE_DIR_MODE);

    for subdir in PROBE_SUBDIRS {
        let path = probe_dir.join(subdir);
        builder.create(&path)?;
        // Separately from the builder's mode, which the process umask masks:
        // the container has to be able to write here whatever umask the
        // service was started with.
        fs::set_permissions(&path, fs::Permissions::from_mode(PROBE_SUBDIR_MODE))?;
    }
    Ok(())
}

/// The whole verdict: the file is there, the orchestrator owns it, and the
/// orchestrator can write to it.
///
/// `probe_dir` is the reference for "the orchestrator's uid" — it created that
/// directory itself moments ago — and `file` is what the container wrote.
/// Returns the uid both are owned by.
fn check_probe_file(probe_dir: &Path, file: &Path) -> Result<u32, EngineError> {
    let own_uid = fs::metadata(probe_dir)?.uid();

    // Any failure to stat the file is "it is not there": the directory was
    // stat-ed a line earlier, so a permission error on a child of it is not a
    // distinction the operator can act on differently. The common cause is a
    // `DATA_DIR_HOST` the engine's VM does not share, where the container
    // wrote its file into a directory nobody else can see.
    let Ok(metadata) = fs::metadata(file) else {
        return Err(EngineError::Probe(format!(
            "probe file was not written: {}",
            file.display()
        )));
    };

    let file_uid = metadata.uid();
    if file_uid != own_uid {
        // Both numbers, because which one is surprising depends on the engine:
        // a sub-uid like 100999 is Podman ignoring `keep-id`, and a file owned
        // by 1000 under an orchestrator that is not 1000 is the documented
        // Docker contract not being met.
        return Err(EngineError::Probe(format!(
            "probe file is owned by uid {file_uid}, orchestrator runs as uid {own_uid}: \
             on Podman check that keep-id:uid=1000,gid=1000 is supported, \
             on Docker run the orchestrator as uid 1000 with DATA_DIR_HOST owned by uid 1000"
        )));
    }

    // Ownership is not permission: a file the container wrote 0o444 would pass
    // the uid check and still stop every session that tried to edit it.
    // Appending nothing changes nothing.
    OpenOptions::new()
        .append(true)
        .open(file)
        .map_err(|error| {
            EngineError::Probe(format!(
                "probe file is not writable by the orchestrator: {error}"
            ))
        })?;

    Ok(own_uid)
}

/// The engine fixture for the tests that must compile *without* the
/// `integration-tests` feature, and so cannot reach `mock::MockEngine`.
///
/// The binary builds a [`BollardEngine`] through [`bootstrap_engine`], and a
/// test that wants engine behaviour uses the mock. This exists for the callers
/// that have neither available:
///
/// - `routes::tests::the_test_routes_exist_only_behind_the_integration_tests_feature`,
///   which asserts both sides of the feature gate and therefore has to build
///   an [`AppState`] in either configuration. This is the one caller that
///   cannot be converted, and so the one that keeps this type alive.
/// - `prelude::state`'s unit tests, `routes::health`'s unreachable-database
///   test and `tests/shutdown.rs`, which only need *an* engine and stay on the
///   placeholder so they keep running in a plain `cargo test`.
///
/// It answers `ping` with `Ok(())`, so `GET /api/health` reports
/// `engine: true`, and every real operation with
/// [`EngineError::Unsupported`], so a caller that reaches for one fails loudly
/// instead of appearing to work. A test that wants engine behaviour uses the
/// mock instead.
///
/// It is deliberately not a conforming implementation of the normalised
/// semantics, and the conformance suite is never run against it: an engine that
/// refuses every operation has no container to be missing, exited or already
/// running.
#[derive(Debug, Clone, Copy, Default)]
pub struct PlaceholderEngine;

impl PlaceholderEngine {
    /// The error every real operation answers with.
    fn unsupported<T>(operation: &str) -> Result<T, EngineError> {
        Err(EngineError::Unsupported(format!(
            "the placeholder engine cannot {operation}"
        )))
    }
}

#[async_trait]
impl ContainerEngine for PlaceholderEngine {
    /// Docker, arbitrarily: it is the kind whose only behavioural consequence
    /// — no `keep-id` — is the one that cannot be wrong on an engine that
    /// never creates a container.
    fn kind(&self) -> EngineKind {
        EngineKind::Docker
    }

    async fn ping(&self) -> Result<(), EngineError> {
        Ok(())
    }

    async fn ensure_network(&self, _name: &str, _internal: bool) -> Result<(), EngineError> {
        Self::unsupported("create a network")
    }

    async fn image_exists(&self, _image: &str) -> Result<bool, EngineError> {
        Self::unsupported("look up an image")
    }

    async fn pull_image(&self, _image: &str) -> Result<(), EngineError> {
        Self::unsupported("pull an image")
    }

    async fn create(&self, _spec: &ContainerSpec) -> Result<ContainerId, EngineError> {
        Self::unsupported("create a container")
    }

    async fn connect_network(&self, _id: &ContainerId, _network: &str) -> Result<(), EngineError> {
        Self::unsupported("connect a network")
    }

    async fn start(&self, _id: &ContainerId) -> Result<(), EngineError> {
        Self::unsupported("start a container")
    }

    async fn stop(&self, _id: &ContainerId, _grace_secs: u32) -> Result<(), EngineError> {
        Self::unsupported("stop a container")
    }

    async fn kill(&self, _id: &ContainerId, _signal: Signal) -> Result<(), EngineError> {
        Self::unsupported("signal a container")
    }

    async fn remove(&self, _id: &ContainerId, _force: bool) -> Result<(), EngineError> {
        Self::unsupported("remove a container")
    }

    async fn inspect(&self, _id: &ContainerId) -> Result<ContainerInfo, EngineError> {
        Self::unsupported("inspect a container")
    }

    async fn wait(&self, _id: &ContainerId) -> Result<ExitStatus, EngineError> {
        Self::unsupported("wait for a container")
    }

    async fn list_by_label(&self, _label_key: &str) -> Result<Vec<ContainerSummary>, EngineError> {
        Self::unsupported("list containers")
    }

    async fn attach_stdin(&self, _id: &ContainerId) -> Result<Box<dyn StdinWriter>, EngineError> {
        Self::unsupported("attach to stdin")
    }

    async fn exec_pty(
        &self,
        _id: &ContainerId,
        _cmd: &[String],
        _user: &str,
        _cols: u16,
        _rows: u16,
    ) -> Result<Box<dyn ExecSession>, EngineError> {
        Self::unsupported("start an exec")
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The trait has to stay dyn compatible: [`AppState`] holds it as
    /// `Arc<dyn ContainerEngine>`, so a method that breaks object safety
    /// breaks the whole crate. This fails to compile before it fails a test.
    fn _assert_object_safe(_engine: Arc<dyn ContainerEngine>) {}

    #[tokio::test]
    async fn the_placeholder_answers_the_ping_and_refuses_every_real_operation() {
        let engine: Arc<dyn ContainerEngine> = Arc::new(PlaceholderEngine);

        assert_eq!(engine.kind(), EngineKind::Docker);
        engine.ping().await.expect("the placeholder is reachable");

        let id = ContainerId("nothing".to_string());
        let error = engine
            .start(&id)
            .await
            .expect_err("the placeholder starts nothing");
        assert!(
            matches!(error, EngineError::Unsupported(_)),
            "unexpected: {error:?}"
        );

        assert!(engine.list_by_label(LABEL_SESSION_ID).await.is_err());
        assert!(engine.inspect(&id).await.is_err());
    }

    /// The bootstrap stops at its first step: a `DOCKER_HOST` no engine can be
    /// reached at is an [`EngineError::Connection`], which is the variant
    /// startup names the socket for, and no network is created and no probe
    /// container is run on the way there. Everything past the connection needs
    /// a real engine and belongs to the engine test suite.
    #[tokio::test]
    async fn the_bootstrap_refuses_an_unusable_docker_host() {
        let vars: std::collections::HashMap<String, String> = [
            ("PUBLIC_URL", "https://mars.example.invalid"),
            ("JWT_SECRET", "not-a-real-signing-secret"),
            ("DATABASE_URL", "postgres://mars:fake@localhost:5432/mars"),
            ("DOCKER_HOST", "ssh://nothing.example.invalid"),
            ("DATA_DIR_HOST", "/srv/mars/data"),
            ("SECRETS_MASTER_KEYS", "1=not-a-real-key"),
            ("GIT_BOT_NAME", "Mars Bot"),
            ("GIT_BOT_EMAIL", "mars-bot@example.invalid"),
            ("SESSION_IMAGE_DEFAULT", "mars-session-claude:dev"),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();
        let config = Config::from_vars(|name| vars.get(name).cloned()).expect("the fixture loads");

        // Matched rather than `expect_err`, because the success type is an
        // `Arc<dyn ContainerEngine>` and the trait is not `Debug`.
        let Err(error) = bootstrap_engine(&config).await else {
            panic!("no engine answers that address");
        };
        assert!(
            matches!(error, EngineError::Connection(_)),
            "unexpected: {error:?}"
        );
        // The scheme is named, the address itself never is (rule 3).
        assert!(
            !error.to_string().contains("nothing.example.invalid"),
            "the address leaked into the error: {error}"
        );
    }

    #[test]
    fn as_any_downcasts_a_placeholder_back() {
        let engine: Arc<dyn ContainerEngine> = Arc::new(PlaceholderEngine);
        assert!(
            engine
                .as_any()
                .downcast_ref::<PlaceholderEngine>()
                .is_some()
        );
    }
}

/// The bootstrap's networks and the startup probe, driven over `MockEngine`.
///
/// The mock has no filesystem, so a test that wants the probe to pass writes
/// `work/probe-ok` itself between the container starting and exiting — the
/// simplest of the two options, and the one that also checks that the bind the
/// container would have written through is the directory the orchestrator then
/// reads. Everything else is read back through the mock's own recorders
/// (`specs`/`spec_of`, `connections`, `networks`, `state_of`), so there is no
/// test double here beyond the mock every other suite uses.
///
/// What the mock cannot show is the uid mismatch — chowning a file to another
/// uid is exactly the privilege a test does not have — so that message is
/// asserted live, on both engines, by `tests/engine.rs`.
#[cfg(all(test, feature = "integration-tests"))]
mod probe_tests {
    use std::collections::HashMap;

    use super::*;
    use crate::engine::mock::MockEngine;

    /// The mock mints ids in creation order and the probe creates exactly one
    /// container.
    const PROBE_CONTAINER: &str = "mock-0";

    /// The image the probe runs, obviously fake (rule 3) and never pulled: the
    /// mock answers `image_exists` with `true` unless a test says otherwise.
    const PROBE_IMAGE: &str = "mars-session-claude:test";

    const INTERNAL_NETWORK: &str = "mars-sessions-test";
    const EGRESS_NETWORK: &str = "mars-egress-test";

    /// The configuration the bootstrap is driven with: the required variables
    /// of `README.md`, "Configuration", with obviously fake values and this
    /// test's own data directory, which is both views of the volume because
    /// the test process is the host.
    fn probe_config(data: &Path) -> Config {
        let data = data.display().to_string();
        let vars: HashMap<String, String> = [
            ("PUBLIC_URL", "https://mars.example.invalid"),
            ("JWT_SECRET", "not-a-real-signing-secret"),
            ("DATABASE_URL", "postgres://mars:fake@localhost:5432/mars"),
            ("DOCKER_HOST", "unix:///nothing.example.invalid"),
            ("DATA_DIR", data.as_str()),
            ("DATA_DIR_HOST", data.as_str()),
            ("SECRETS_MASTER_KEYS", "1=not-a-real-key"),
            ("GIT_BOT_NAME", "Mars Bot"),
            ("GIT_BOT_EMAIL", "mars-bot@example.invalid"),
            ("SESSION_IMAGE_DEFAULT", PROBE_IMAGE),
            ("SESSION_NETWORK_INTERNAL", INTERNAL_NETWORK),
            ("SESSION_NETWORK_EGRESS", EGRESS_NETWORK),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();

        Config::from_vars(|name| vars.get(name).cloned()).expect("the fixture configuration loads")
    }

    fn probe_id() -> ContainerId {
        ContainerId(PROBE_CONTAINER.to_string())
    }

    /// Park until the probe has started its container, which is when the mock
    /// will accept an `exit` for it.
    async fn started(engine: &MockEngine) -> ContainerId {
        let id = probe_id();
        loop {
            if engine.state_of(&id) == Some(ContainerState::Running) {
                return id;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }

    /// The host directory behind the container's `/session/work`, which is
    /// where the real container's `touch` would land.
    fn work_dir(spec: &ContainerSpec) -> PathBuf {
        spec.binds
            .iter()
            .find(|bind| bind.container_target == "/session/work")
            .expect("the probe mounts a work directory")
            .host_source
            .clone()
    }

    fn mode_of(path: &Path) -> u32 {
        fs::metadata(path).expect("the path is there").mode() & 0o777
    }

    fn reason(error: &EngineError) -> String {
        match error {
            EngineError::Probe(reason) => reason.clone(),
            other => panic!("expected a probe failure, got: {other:?}"),
        }
    }

    /// Whether this process is root, read the way the probe reads a uid: off a
    /// directory it created itself.
    fn running_as_root() -> bool {
        let dir = tempfile::tempdir().expect("a temporary directory");
        mode_owner(dir.path()) == 0
    }

    fn mode_owner(path: &Path) -> u32 {
        fs::metadata(path).expect("the path is there").uid()
    }

    /// The whole passing path: both networks first, then pull nothing, create
    /// the probe container on the internal network, connect the egress network
    /// before starting it, and find the file where the bind said it would be.
    ///
    /// The directory modes are asserted from inside the container's lifetime,
    /// because the probe removes the directory before it returns.
    #[tokio::test]
    async fn the_bootstrap_creates_both_networks_and_passes_a_probe_whose_file_comes_back() {
        let data = tempfile::tempdir().expect("a temporary directory");
        let engine = Arc::new(MockEngine::default());
        let config = probe_config(data.path());

        let mock = Arc::clone(&engine);
        let container = tokio::spawn(async move {
            let id = started(&mock).await;
            let spec = mock.spec_of(&id).expect("the probe container exists");
            let work = work_dir(&spec);
            let probe_dir = work
                .parent()
                .expect("the work directory has a parent")
                .to_path_buf();

            // World-writable subdirectories, an ordinary probe directory: the
            // check measures ownership, not permission.
            let modes: Vec<u32> = PROBE_SUBDIRS
                .iter()
                .map(|subdir| mode_of(&probe_dir.join(subdir)))
                .collect();
            let dir_mode = mode_of(&probe_dir);

            fs::write(work.join(PROBE_FILE), b"").expect("the probe file is written");
            // Read before the exit, because the mock forgets a container's
            // connections when it is removed. Their presence here is the
            // ordering a launch shares: egress connected before start.
            let connections = mock.connections(&id);
            assert!(mock.exit(&id, 0), "the probe container is still there");

            (spec, connections, modes, dir_mode)
        });

        bootstrap_connected(engine.as_ref(), &config)
            .await
            .expect("the bootstrap passes");
        let (spec, connections, modes, dir_mode) =
            container.await.expect("the container task finished");

        assert_eq!(
            engine.networks(),
            vec![
                (INTERNAL_NETWORK.to_string(), true),
                (EGRESS_NETWORK.to_string(), false),
            ],
            "the networks are created before any container is"
        );
        assert_eq!(connections, vec![EGRESS_NETWORK.to_string()]);

        assert!(
            spec.name.starts_with("mars-probe-"),
            "unexpected probe container name: {}",
            spec.name
        );
        assert_eq!(
            spec.labels.get(spec::LABEL_PROBE).map(String::as_str),
            Some("true")
        );
        assert_eq!(spec.image, PROBE_IMAGE);
        assert_eq!(spec.network, INTERNAL_NETWORK);
        assert!(
            work_dir(&spec).starts_with(data.path().join("tmp")),
            "the probe writes under DATA_DIR/tmp: {:?}",
            work_dir(&spec)
        );

        assert_eq!(modes, vec![PROBE_SUBDIR_MODE; PROBE_SUBDIRS.len()]);
        assert_eq!(
            dir_mode & 0o002,
            0,
            "the probe directory itself should not be world-writable: {dir_mode:o}"
        );

        assert_eq!(
            engine.state_of(&probe_id()),
            None,
            "the probe container is removed"
        );
        assert!(
            fs::read_dir(data.path().join("tmp"))
                .expect("DATA_DIR/tmp was created")
                .next()
                .is_none(),
            "the probe directory is removed"
        );
    }

    /// The first start of a fresh host: the image is not there yet.
    #[tokio::test]
    async fn a_missing_image_is_pulled_before_the_probe_container_is_created() {
        let data = tempfile::tempdir().expect("a temporary directory");
        let engine = Arc::new(MockEngine::default());
        engine.set_missing_images([PROBE_IMAGE]);
        let config = probe_config(data.path());

        let mock = Arc::clone(&engine);
        let container = tokio::spawn(async move {
            let id = started(&mock).await;
            let work = work_dir(&mock.spec_of(&id).expect("the probe container exists"));
            fs::write(work.join(PROBE_FILE), b"").expect("the probe file is written");
            mock.exit(&id, 0);
        });

        bootstrap_connected(engine.as_ref(), &config)
            .await
            .expect("the bootstrap passes");
        container.await.expect("the container task finished");

        assert_eq!(engine.pulled_images(), vec![PROBE_IMAGE.to_string()]);
        assert_eq!(
            engine.specs().len(),
            1,
            "one probe container, created after the pull"
        );
    }

    /// A pull failure carries the registry's own words and never reaches the
    /// container steps.
    #[tokio::test]
    async fn a_failed_pull_is_a_probe_failure_carrying_the_engine_message() {
        let data = tempfile::tempdir().expect("a temporary directory");
        let engine = Arc::new(MockEngine::default());
        engine.set_missing_images([PROBE_IMAGE]);
        engine.fail_next_pull("manifest unknown");
        let config = probe_config(data.path());

        let error = bootstrap_connected(engine.as_ref(), &config)
            .await
            .expect_err("the bootstrap fails");

        assert_eq!(reason(&error), "probe image pull failed: manifest unknown");
        assert!(
            engine.specs().is_empty(),
            "no container is created after a failed pull"
        );
    }

    /// A container that ran and failed is reported by its code, and is still
    /// removed.
    #[tokio::test]
    async fn a_non_zero_exit_fails_the_probe_and_the_container_is_still_removed() {
        let data = tempfile::tempdir().expect("a temporary directory");
        let engine = Arc::new(MockEngine::default());
        let config = probe_config(data.path());

        let mock = Arc::clone(&engine);
        let container = tokio::spawn(async move {
            let id = started(&mock).await;
            mock.exit(&id, 127);
        });

        let error = bootstrap_connected(engine.as_ref(), &config)
            .await
            .expect_err("the bootstrap fails");
        container.await.expect("the container task finished");

        assert_eq!(reason(&error), "probe container exited with code 127");
        assert_eq!(engine.state_of(&probe_id()), None);
        assert!(
            fs::read_dir(data.path().join("tmp"))
                .expect("DATA_DIR/tmp was created")
                .next()
                .is_none(),
            "a failed probe removes its directory too"
        );
    }

    /// A container that exits cleanly without writing anything is the unshared
    /// `DATA_DIR_HOST`: the check runs and names the path it looked at.
    #[tokio::test]
    async fn a_clean_exit_that_wrote_nothing_is_the_missing_file_failure() {
        let data = tempfile::tempdir().expect("a temporary directory");
        let engine = Arc::new(MockEngine::default());
        let config = probe_config(data.path());

        let mock = Arc::clone(&engine);
        let container = tokio::spawn(async move {
            let id = started(&mock).await;
            mock.exit(&id, 0);
        });

        let error = bootstrap_connected(engine.as_ref(), &config)
            .await
            .expect_err("the bootstrap fails");
        container.await.expect("the container task finished");

        let message = reason(&error);
        assert!(
            message.starts_with("probe file was not written: "),
            "unexpected: {message}"
        );
        assert!(
            message.ends_with("/work/probe-ok"),
            "the path names the file the container should have touched: {message}"
        );
        assert_eq!(engine.state_of(&probe_id()), None);
    }

    /// Owned correctly and still unusable: a file the container left read-only
    /// would stop every session that tried to edit it.
    #[tokio::test]
    async fn a_file_the_orchestrator_cannot_append_to_fails_the_probe() {
        // Root ignores the mode bits, so there is nothing to assert as root.
        if running_as_root() {
            return;
        }

        let data = tempfile::tempdir().expect("a temporary directory");
        let engine = Arc::new(MockEngine::default());
        let config = probe_config(data.path());

        let mock = Arc::clone(&engine);
        let container = tokio::spawn(async move {
            let id = started(&mock).await;
            let file =
                work_dir(&mock.spec_of(&id).expect("the probe container exists")).join(PROBE_FILE);
            fs::write(&file, b"").expect("the probe file is written");
            fs::set_permissions(&file, fs::Permissions::from_mode(0o444))
                .expect("the file is made read-only");
            mock.exit(&id, 0);
        });

        let error = bootstrap_connected(engine.as_ref(), &config)
            .await
            .expect_err("the bootstrap fails");
        container.await.expect("the container task finished");

        let message = reason(&error);
        assert!(
            message.starts_with("probe file is not writable by the orchestrator: "),
            "unexpected: {message}"
        );
    }

    /// A container that never exits is killed and removed, and the message
    /// names the budget it blew.
    ///
    /// Time is paused, so the runtime advances its own clock to the deadline
    /// the moment every task is parked; the test costs no wall-clock seconds
    /// and still exercises the real [`PROBE_TIMEOUT`]. The mock forgets a
    /// removed container's signals, so that the `SIGKILL` precedes the removal
    /// is what the live suite covers; what is asserted here is the verdict and
    /// that nothing survives it.
    #[tokio::test(start_paused = true)]
    async fn a_container_that_never_exits_is_killed_and_removed() {
        let data = tempfile::tempdir().expect("a temporary directory");
        let engine = Arc::new(MockEngine::default());
        let config = probe_config(data.path());

        let error = bootstrap_connected(engine.as_ref(), &config)
            .await
            .expect_err("the bootstrap fails");

        assert_eq!(reason(&error), "probe container did not exit within 60s");
        assert_eq!(engine.state_of(&probe_id()), None);
        assert!(
            fs::read_dir(data.path().join("tmp"))
                .expect("DATA_DIR/tmp was created")
                .next()
                .is_none(),
            "a timed-out probe removes its directory too"
        );
    }

    /// An engine failure is not a probe verdict: the variant is passed
    /// through, and the container is removed all the same.
    #[tokio::test]
    async fn a_container_that_disappears_propagates_not_found() {
        let data = tempfile::tempdir().expect("a temporary directory");
        let engine = Arc::new(MockEngine::default());
        let config = probe_config(data.path());

        let mock = Arc::clone(&engine);
        let container = tokio::spawn(async move {
            let id = started(&mock).await;
            assert!(mock.vanish(&id), "the probe container is still there");
        });

        let error = bootstrap_connected(engine.as_ref(), &config)
            .await
            .expect_err("the bootstrap fails");
        container.await.expect("the container task finished");

        assert!(
            matches!(error, EngineError::NotFound(_)),
            "unexpected: {error:?}"
        );
        assert_eq!(engine.state_of(&probe_id()), None);
    }
}
