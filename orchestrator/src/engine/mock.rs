//! The engine mock, compiled only with the `integration-tests` feature.
//!
//! It records what was asked of it so a test can assert the orchestrator's
//! side of an interaction without an engine socket.
//!
//! Only `ping` is recorded and answered so far. Every other operation refuses
//! with [`EngineError::Unsupported`], the same way [`PlaceholderEngine`] does,
//! so a test that reaches one before the mock task of the container engine
//! epic has taught it to answer fails loudly rather than silently passing.

use std::any::Any;
// Shadows the prelude's one-parameter `Result<T>` alias; see
// [`super`] for why the engine trait returns `Result<T, EngineError>`.
use std::result::Result;
use std::sync::{Mutex, MutexGuard};

use async_trait::async_trait;

use super::{
    ContainerEngine, ContainerId, ContainerInfo, ContainerSpec, ContainerSummary, EngineError,
    EngineKind, ExecSession, ExitStatus, Signal, StdinWriter,
};
// The crate convention (`CLAUDE.md`, "Backend conventions"); the mock reports
// the engine's own [`EngineError`], so outside the tests below the glob is
// here for the doc links and for what the mock grows into.
#[allow(unused_imports)]
use crate::prelude::*;

/// One recorded call. The mock task of the container engine epic adds a
/// variant per operation it teaches the mock to answer, carrying the arguments
/// a test wants to assert on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineCall {
    /// [`ContainerEngine::ping`].
    Ping,
}

/// An engine that answers every call it supports successfully and remembers
/// it.
#[derive(Debug, Default)]
pub struct MockContainerEngine {
    calls: Mutex<Vec<EngineCall>>,
}

impl MockContainerEngine {
    /// A fresh mock with no recorded calls.
    pub fn new() -> Self {
        Self::default()
    }

    /// The calls made so far, in order, cloned out of the mutex so a caller
    /// never holds the lock across an await.
    pub fn calls(&self) -> Vec<EngineCall> {
        self.lock().clone()
    }

    fn lock(&self) -> MutexGuard<'_, Vec<EngineCall>> {
        // Test-only code: a poisoned lock means another test thread already
        // panicked, which is a failure in its own right.
        self.calls.lock().expect("the mock engine lock is healthy")
    }

    /// The error every operation the mock does not answer yet refuses with.
    fn unsupported<T>(operation: &str) -> Result<T, EngineError> {
        Err(EngineError::Unsupported(format!(
            "the mock engine cannot {operation} yet"
        )))
    }
}

#[async_trait]
impl ContainerEngine for MockContainerEngine {
    /// Podman: the target engine, so a code path that branches on the kind
    /// takes the branch production takes (ADR 0004).
    fn kind(&self) -> EngineKind {
        EngineKind::Podman
    }

    async fn ping(&self) -> Result<(), EngineError> {
        self.lock().push(EngineCall::Ping);
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

    #[tokio::test]
    async fn ping_succeeds_and_is_recorded_in_order() {
        let engine = MockContainerEngine::new();
        assert!(engine.calls().is_empty());

        engine.ping().await.expect("the mock always answers");
        engine.ping().await.expect("the mock always answers");

        assert_eq!(engine.calls(), vec![EngineCall::Ping, EngineCall::Ping]);
    }

    #[tokio::test]
    async fn as_any_downcasts_a_trait_object_back_to_the_mock() {
        let engine: Arc<dyn ContainerEngine> = Arc::new(MockContainerEngine::new());
        engine.ping().await.expect("the mock always answers");

        let mock = engine
            .as_any()
            .downcast_ref::<MockContainerEngine>()
            .expect("the trait object is the mock");
        assert_eq!(mock.calls(), vec![EngineCall::Ping]);
    }

    #[tokio::test]
    async fn an_operation_the_mock_cannot_answer_yet_refuses_and_records_nothing() {
        let engine = MockContainerEngine::new();

        let error = engine
            .start(&ContainerId("nothing".to_string()))
            .await
            .expect_err("the mock starts nothing");

        assert!(
            matches!(error, EngineError::Unsupported(_)),
            "unexpected: {error:?}"
        );
        assert!(engine.calls().is_empty());
    }

    #[test]
    fn the_mock_reports_the_target_engine() {
        assert_eq!(MockContainerEngine::new().kind(), EngineKind::Podman);
    }
}
