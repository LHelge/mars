//! The `ContainerEngine` trait, its bollard implementation and the mock
//! behind the `integration-tests` feature.
//!
//! Only the shape the test harness depends on exists yet: the trait, an
//! `EngineError` and a startup placeholder. The container engine epic adds the
//! real methods (create, start, stop, exec, inspect, attach) and the bollard
//! implementation behind this same trait.
//!
//! **Async style.** The three collaborator traits (`ContainerEngine`,
//! [`EmailClient`](crate::email::EmailClient) and
//! [`GitCredentialProvider`](crate::git::GitCredentialProvider)) are held as
//! `Arc<dyn Trait>` in [`AppState`], so they must be dyn compatible. They use
//! `#[async_trait::async_trait]` rather than a native `async fn` in a trait;
//! every epic that extends them keeps that choice so the trait objects stay
//! usable.

use std::any::Any;

use async_trait::async_trait;
use axum::http::StatusCode;

use crate::prelude::*;

#[cfg(feature = "integration-tests")]
pub mod mock;

/// Anything the container engine refuses or cannot answer.
///
/// One variant for now; the container engine epic adds the ones it needs and
/// refines [`EngineError::status`] where a failure is really a state conflict
/// rather than an internal fault.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    /// The engine socket is unreachable, or answered an error.
    #[error("the container engine is unavailable: {0}")]
    Unavailable(String),
}

impl EngineError {
    /// The HTTP status this failure maps to. Everything is an internal fault
    /// until the container engine epic distinguishes state conflicts.
    pub fn status(&self) -> StatusCode {
        match self {
            EngineError::Unavailable(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

/// The container engine, behind a trait so the API can be tested without one.
///
/// `ARCHITECTURE.md`, "Orchestrator internals": [`AppState`] holds this as
/// `Arc<dyn ContainerEngine>`, and the `integration-tests` feature supplies
/// `mock::MockContainerEngine`.
#[async_trait]
pub trait ContainerEngine: Send + Sync {
    /// Downcast hook, so a test that injected a concrete engine can read back
    /// what the handler did with it.
    fn as_any(&self) -> &dyn Any;

    /// Whether the engine answers at all. `GET /api/health` reports the result
    /// as `engine` (`SPEC.md`, "Health").
    async fn ping(&self) -> Result<()>;
}

/// The engine used until the container engine epic lands.
///
/// It answers `ping` with `Ok(())`, which keeps `GET /api/health` reporting
/// `engine: true` exactly as the scaffold's `engine_ready()` placeholder did.
/// The container engine epic deletes it.
#[derive(Debug, Clone, Copy, Default)]
pub struct PlaceholderEngine;

#[async_trait]
impl ContainerEngine for PlaceholderEngine {
    fn as_any(&self) -> &dyn Any {
        self
    }

    async fn ping(&self) -> Result<()> {
        Ok(())
    }
}
