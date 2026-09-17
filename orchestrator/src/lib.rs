//! Mars orchestrator library crate.
//!
//! The module tree mirrors `ARCHITECTURE.md`, "Orchestrator internals"; every
//! module is a directory module and does `use crate::prelude::*;`.

pub mod prelude;

pub mod agent;
pub mod cron;
pub mod email;
pub mod engine;
pub mod events;
pub mod git;
pub mod mcp;
pub mod models;
pub mod repositories;
pub mod routes;
pub mod secrets;
pub mod session;
pub mod sse;
pub mod ws;
