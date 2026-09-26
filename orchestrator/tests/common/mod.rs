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

/// Bare upstream repositories a lifecycle test can also empty, remove and
/// recreate.
/// Plain `std::process::Command` and the `git` binary, so it needs no mock and
/// stays ungated like `common::db`.
pub mod git;

/// The tracker-product names no agent-facing text of ours may contain. Plain
/// string constants, so it stays ungated like `common::db`; `mcp_descriptions`
/// pulls it in with `#[path]` rather than `mod common;` so that it keeps
/// needing neither a database nor a container engine.
pub mod tracker_products;

/// Locks, counts and timings for the concurrency suites. Plain SQL and the
/// crate's advisory-lock key, so it needs no mock and stays ungated.
pub mod races;

/// The Server-Sent Events wire format, parsed: what `TestApp::sse` hands a
/// scenario once it has the bytes. Generic over the byte stream, so it stays
/// ungated like `common::db`.
pub mod sse;

/// `TestApp` needs the mocks, which exist only behind the `integration-tests`
/// feature, so the module is gated rather than the items inside it: the test
/// binaries that only use `common::db` still compile without the feature.
#[cfg(feature = "integration-tests")]
pub mod app;

/// The in-process MCP client every MCP suite drives the server through
/// (`CLAUDE.md`, "Testing expectations"). Built on `TestApp`, so it is gated
/// the same way.
#[cfg(feature = "integration-tests")]
pub mod mcp;

/// The recorded requests of a pinned CLI's MCP conversation and the recorder
/// that writes them (`tests/mcp_conformance.rs`). Plain axum and serde, so it
/// stays ungated.
pub mod mcp_fixtures;

/// What the pinned CLI accepts on MCP protocol revision 2026-07-28, as checks
/// over JSON (`tests/mcp_conformance.rs`). Plain `serde_json`, so it stays
/// ungated.
pub mod mcp_2026_07_28;

/// Arranging leases, attempt counters, states, blocked flags and current
/// hand-offs through the `tracker/` verbs, for every suite that needs one of
/// them as a precondition (`CLAUDE.md`, "Testing expectations", "Tracker
/// tests"). Only the crate's own public tracker API, so it stays ungated like
/// `common::db`.
pub mod tracker;

/// The code hand-off suites' shared arrangement: a ready project with a real
/// repository, sessions with real work clones and the tracker rows a hand-off
/// needs. Built on `TestApp`, so it is gated the same way.
#[cfg(feature = "integration-tests")]
pub mod handoffs;

/// The project route suites' shared helpers: signing in, creating a project
/// and waiting for its clone, a session in a chosen state and the per-table
/// row counts of a deletion. Built on `TestApp`, so it is gated the same way.
#[cfg(feature = "integration-tests")]
pub mod projects;

/// `use common::TestApp;` for the binaries that want it; the ones that only
/// use `common::db` leave the re-export unused, which is the same situation
/// `#![allow(dead_code)]` above covers for items.
#[cfg(feature = "integration-tests")]
#[allow(unused_imports)]
pub use app::{
    AuthenticatedUser, ListenerHandle, TEST_TIMINGS, TestApp, collect_ws_events,
    terminate_listener_backend,
};
