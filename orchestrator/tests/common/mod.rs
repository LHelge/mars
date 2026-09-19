//! Shared helpers for the integration tests (`CLAUDE.md`, "Testing
//! expectations").
//!
//! Every test binary declares `mod common;` and uses the subset it needs, so
//! items no single binary touches are expected here rather than a lint to fix.

#![allow(dead_code)]

/// The live Claude Code probe's plumbing (`tests/claude_probe.rs`): process
/// spawn, line reader, fixture recorder and the credential-leak scanner. Plain
/// `tokio::process` and the crate's public agent API, so it needs no mock and
/// stays ungated like `common::db`.
pub mod claude_probe;

pub mod db;

/// The live engine suite's helpers: connecting or skipping on `DOCKER_HOST`,
/// throwaway container specifications, unique names and the cleanup that runs
/// whether a scenario passed or panicked. Plain `bollard` and the crate's own
/// public engine API, so it needs no mock and stays ungated like `common::db`.
pub mod engine;

/// The engine conformance suite: the normalised semantics of
/// `ARCHITECTURE.md`, "Engine adapter", as one scenario per rule over an
/// `Arc<dyn ContainerEngine>`. `tests/engine.rs` runs it against
/// `BollardEngine` and `tests/engine_mock.rs` against `MockEngine`. Only the
/// crate's own public engine API, so it needs no mock and stays ungated like
/// `common::engine`.
pub mod engine_contract;

/// Bare upstream repositories a lifecycle test can also remove and recreate.
/// Plain `std::process::Command` and the `git` binary, so it needs no mock and
/// stays ungated like `common::db`.
pub mod git;

/// Locks, counts and timings for the concurrency suites. Plain SQL and the
/// crate's advisory-lock key, so it needs no mock and stays ungated.
pub mod races;

/// `TestApp` needs the mocks, which exist only behind the `integration-tests`
/// feature, so the module is gated rather than the items inside it: the test
/// binaries that only use `common::db` still compile without the feature.
#[cfg(feature = "integration-tests")]
pub mod app;

/// The code hand-off suites' shared arrangement: a ready project with a real
/// repository, sessions with real work clones and the tracker rows a hand-off
/// needs. Built on `TestApp`, so it is gated the same way.
#[cfg(feature = "integration-tests")]
pub mod handoffs;

/// `use common::TestApp;` for the binaries that want it; the ones that only
/// use `common::db` leave the re-export unused, which is the same situation
/// `#![allow(dead_code)]` above covers for items.
#[cfg(feature = "integration-tests")]
#[allow(unused_imports)]
pub use app::{AuthenticatedUser, TestApp};
