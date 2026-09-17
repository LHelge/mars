//! Items every module imports with `use crate::prelude::*;`.
//!
//! `ARCHITECTURE.md`, "Orchestrator internals" lists what lives here once the
//! rest of the backend exists: `AppState`, `Config`, `Claims`, `Error` and the
//! crate-wide `Result` alias. Each of those is added by the task that defines
//! it; for now the prelude carries only the `tracing` macros.

pub mod error;

pub use error::{Error, Json, Result};
pub use tracing::{debug, error, info, warn};
