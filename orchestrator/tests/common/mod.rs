//! Shared helpers for the integration tests (`CLAUDE.md`, "Testing
//! expectations").
//!
//! Every test binary declares `mod common;` and uses the subset it needs, so
//! items no single binary touches are expected here rather than a lint to fix.

#![allow(dead_code)]

pub mod db;

/// The live engine suite's helpers: connecting or skipping on `DOCKER_HOST`,
/// throwaway container specifications, unique names and the cleanup that runs
/// whether a scenario passed or panicked. Plain `bollard` and the crate's own
/// public engine API, so it needs no mock and stays ungated like `common::db`.
pub mod engine;

/// Locks, counts and timings for the concurrency suites. Plain SQL and the
/// crate's advisory-lock key, so it needs no mock and stays ungated.
pub mod races;

/// `TestApp` needs the mocks, which exist only behind the `integration-tests`
/// feature, so the module is gated rather than the items inside it: the test
/// binaries that only use `common::db` still compile without the feature.
#[cfg(feature = "integration-tests")]
pub mod app;

/// `use common::TestApp;` for the binaries that want it; the ones that only
/// use `common::db` leave the re-export unused, which is the same situation
/// `#![allow(dead_code)]` above covers for items.
#[cfg(feature = "integration-tests")]
#[allow(unused_imports)]
pub use app::{AuthenticatedUser, TestApp};
