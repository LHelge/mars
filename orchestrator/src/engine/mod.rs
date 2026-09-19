//! The `ContainerEngine` trait, its bollard implementation and the mock
//! behind the `integration-tests` feature.
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
// Shadows the prelude's one-parameter `Result<T>` alias: every engine
// operation fails with an `EngineError`, not with the crate-wide `Error`, so
// that callers can match on the variant before deciding what it means.
use std::result::Result;

use async_trait::async_trait;

// The crate convention (`CLAUDE.md`, "Backend conventions"). The engine
// reports its own [`EngineError`] rather than the crate-wide one, so the glob
// is here for [`Config`], [`Arc`] and the `tracing` macros [`bootstrap_engine`]
// uses, and for the doc links.
use crate::prelude::*;

pub mod bollard;
pub mod error;
pub mod probe;
pub mod spec;
mod streams;
pub mod types;

#[cfg(feature = "integration-tests")]
pub mod mock;

// `self::`, because a plain `bollard::` in this one module is the submodule
// below and not the crate (see `bollard`'s own module documentation).
use self::bollard::BollardEngine;
use self::probe::{ProbeInput, run_startup_probe};

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
    run_startup_probe(
        &engine,
        ProbeInput {
            image: config.session_image_default.clone(),
            data_dir: config.data_dir.clone(),
            data_dir_host: config.data_dir_host.clone(),
            network_internal: config.session_network_internal.clone(),
            network_egress: config.session_network_egress.clone(),
            extra_hosts: config.session_extra_hosts.clone(),
        },
    )
    .await?;

    Ok(Arc::new(engine))
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
