//! Items every module imports with `use crate::prelude::*;`.
//!
//! `ARCHITECTURE.md`, "Orchestrator internals" lists what lives here:
//! [`AppState`], [`Config`], `Claims`, [`Error`] and the crate-wide [`Result`]
//! alias. `Claims` arrives with the authentication epic; everything else is
//! here, together with the handful of third-party names — [`Arc`], [`PgPool`]
//! and the `tracing` macros — that appear in nearly every module and would
//! otherwise be re-imported by hand each time.
//!
//! Nothing that is specific to one concern belongs here: traits, models and
//! repositories stay in their own modules and are imported explicitly.

pub mod config;
pub mod error;
pub mod state;
pub mod telemetry;

pub use config::{Config, ConfigError, SecretsMasterKeySource};
pub use error::{Error, Json, Result};
pub use state::AppState;
pub use telemetry::{TelemetryError, init_tracing};

pub use sqlx::PgPool;
pub use std::sync::Arc;
pub use tracing::{debug, error, info, instrument, warn};
