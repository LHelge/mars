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
//! [`ContainerSpec::env`] and nowhere else. A spec is never `Debug`-printed
//! into an error or logged at `info` or above; its own [`Debug`] prints env
//! keys only, so even a careless `{:?}` cannot leak one (CLAUDE.md rule 3).

use std::any::Any;
// Shadows the prelude's one-parameter `Result<T>` alias: every engine
// operation fails with an `EngineError`, not with the crate-wide `Error`, so
// that callers can match on the variant before deciding what it means.
use std::result::Result;

use async_trait::async_trait;

// The crate convention (`CLAUDE.md`, "Backend conventions"). The engine
// reports its own [`EngineError`] rather than the crate-wide one, so the glob
// is here for the doc links and for what this module grows into.
#[allow(unused_imports)]
use crate::prelude::*;

pub mod bollard;
pub mod error;
pub mod probe;
pub mod spec;
mod streams;
pub mod types;

#[cfg(feature = "integration-tests")]
pub mod mock;

pub use error::EngineError;
pub use types::{
    Bind, ContainerId, ContainerInfo, ContainerSpec, ContainerState, ContainerSummary, EngineKind,
    ExecSession, ExitStatus, LABEL_PROFILE_ID, LABEL_PROJECT_ID, LABEL_SESSION_ID, Signal,
    StdinWriter, session_container_name,
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
#[async_trait]
pub trait ContainerEngine: Send + Sync {
    /// Which engine is behind the socket, detected once when the adapter
    /// connected. It decides whether `UsernsMode: keep-id` is set (ADR 0004).
    fn kind(&self) -> EngineKind;

    /// Whether the engine answers at all. `GET /api/health` reports the result
    /// as `engine` (`SPEC.md`, "Health").
    async fn ping(&self) -> Result<(), EngineError>;

    /// Create the named network if it does not exist, and succeed if it does.
    ///
    /// `internal` is the sessions network, which has no route off the host;
    /// the egress network is created without it (`ARCHITECTURE.md`,
    /// "Networking").
    async fn ensure_network(&self, name: &str, internal: bool) -> Result<(), EngineError>;

    /// Whether the image is present locally, so the launcher can skip a pull.
    async fn image_exists(&self, image: &str) -> Result<bool, EngineError>;

    /// Pull the image, waiting for the pull to finish.
    ///
    /// Fails with [`EngineError::ImagePull`]; the launcher puts that message
    /// into `sessions.error` and fails the launch.
    async fn pull_image(&self, image: &str) -> Result<(), EngineError>;

    /// Create a container from the spec, without starting it.
    ///
    /// The container is created on [`ContainerSpec::network`]; the egress
    /// network is a second [`connect_network`](Self::connect_network) before
    /// [`start`](Self::start), so the container never runs with the wrong set.
    async fn create(&self, spec: &ContainerSpec) -> Result<ContainerId, EngineError>;

    /// Attach a created container to a second network.
    async fn connect_network(&self, id: &ContainerId, network: &str) -> Result<(), EngineError>;

    /// Start a created container.
    async fn start(&self, id: &ContainerId) -> Result<(), EngineError>;

    /// Stop a container, letting it have `grace_secs` before the engine's own
    /// hard kill. Mars's own stop sequence is [`kill`](Self::kill) with
    /// [`Signal::Sigint`] then [`Signal::Sigterm`] (`ARCHITECTURE.md`, "Stop
    /// semantics"); this is the plain stop used when ending a session.
    async fn stop(&self, id: &ContainerId, grace_secs: u32) -> Result<(), EngineError>;

    /// Send a named signal to the container's main process.
    async fn kill(&self, id: &ContainerId, signal: Signal) -> Result<(), EngineError>;

    /// Remove a container. `force` removes it even while running.
    ///
    /// A container that is already gone is [`EngineError::NotFound`], which
    /// callers treat as success.
    async fn remove(&self, id: &ContainerId, force: bool) -> Result<(), EngineError>;

    /// Everything the engine knows about one container.
    async fn inspect(&self, id: &ContainerId) -> Result<ContainerInfo, EngineError>;

    /// Wait for the container to exit and report how it ended. Returns
    /// immediately for a container that has already exited.
    async fn wait(&self, id: &ContainerId) -> Result<ExitStatus, EngineError>;

    /// Every container carrying the label, running or not. Recovery lists
    /// [`LABEL_SESSION_ID`] to find the containers it owns.
    async fn list_by_label(&self, label_key: &str) -> Result<Vec<ContainerSummary>, EngineError>;

    /// Attach to the container's stdin, with the TTY off.
    ///
    /// Only stdin: stdout and stderr are not attached, because the CLI's
    /// output is read from the transcript file, not the socket
    /// (`ARCHITECTURE.md`, "Agent process model"). The session owner is the
    /// only holder of the returned writer.
    async fn attach_stdin(&self, id: &ContainerId) -> Result<Box<dyn StdinWriter>, EngineError>;

    /// Start an exec with a PTY: the terminal view.
    ///
    /// `SPEC.md`, "WebSocket: session stream": `/bin/bash -l` as `agent`, at
    /// the client's initial `cols` and `rows`.
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

/// The engine used until the wiring task of the container engine epic replaces
/// it with the bollard adapter.
///
/// It answers `ping` with `Ok(())`, which keeps `GET /api/health` reporting
/// `engine: true` exactly as the scaffold's `engine_ready()` placeholder did,
/// and every real operation with [`EngineError::Unsupported`], so a caller
/// that reaches for one before the adapter exists fails loudly instead of
/// appearing to work.
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
