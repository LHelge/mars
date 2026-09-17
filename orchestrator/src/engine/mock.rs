//! The engine mock, compiled only with the `integration-tests` feature.
//!
//! It records what was asked of it so a test can assert the orchestrator's
//! side of an interaction without an engine socket.

use std::any::Any;
use std::sync::{Mutex, MutexGuard};

use async_trait::async_trait;

use super::ContainerEngine;
use crate::prelude::*;

/// One recorded call. The container engine epic adds a variant per method it
/// adds to the trait, carrying the arguments a test wants to assert on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineCall {
    /// [`ContainerEngine::ping`].
    Ping,
}

/// An engine that answers every call successfully and remembers it.
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
}

#[async_trait]
impl ContainerEngine for MockContainerEngine {
    fn as_any(&self) -> &dyn Any {
        self
    }

    async fn ping(&self) -> Result<()> {
        self.lock().push(EngineCall::Ping);
        Ok(())
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
}
