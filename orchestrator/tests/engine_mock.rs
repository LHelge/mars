//! The engine conformance suite against `MockEngine`, with no engine anywhere.
//!
//! `CLAUDE.md`, "Testing expectations": the engine contract tests run against
//! the mock on every run of the test suite — `cargo test --features
//! integration-tests`, which is the feature the mock itself lives behind — and
//! against `BollardEngine` when `DOCKER_HOST` is set. This is the first half — nothing here opens a socket,
//! starts a container or touches the database, so it runs in every environment
//! — and `tests/engine.rs` is the second.
//!
//! The suite itself is `common::engine_contract`, one scenario per line of the
//! normalised semantics of `ARCHITECTURE.md`, "Engine adapter". The mock is held
//! to exactly the same rules as the production adapter, because a mock that
//! answers something else makes every test built on it agree with the wrong
//! thing.
//!
//! **Every scenario runs.** Nothing here is `#[ignore]`d: the mock answers the
//! whole list, including the three idempotences (`start` of a running
//! container, `stop` of an exited one, `remove` of a missing one), signal
//! delivery and the failed write after an exit. Each scenario is also a test of
//! its own, so a regression names the rule it broke instead of only failing
//! `the_whole_contract`.

#![cfg(feature = "integration-tests")]

mod common;

use std::sync::Arc;

use common::engine_contract::{ContractEnv, EngineContract, assert_engine_contract};
use mars_orchestrator::engine::mock::MockEngine;
use mars_orchestrator::engine::{ContainerEngine, EngineKind};

/// The image the specs carry. The mock never runs anything, so this only has to
/// be an obviously fake reference (CLAUDE.md rule 3).
const MOCK_IMAGE: &str = "mars-session-claude:not-a-real-image";

/// The two networks, named as a deployment's are (`ARCHITECTURE.md`,
/// "Networks"). The mock records them and creates nothing.
const NETWORK_INTERNAL: &str = "mars-sessions";
const NETWORK_EGRESS: &str = "mars-egress";

/// A fresh contract over a fresh mock.
///
/// Per scenario rather than per file: the mock is an in-memory table, and a
/// scenario that inherited another's containers would be asserting against
/// state the contract never described.
fn contract() -> EngineContract {
    // Podman, as `MockEngine::default()` is: the kind production targets.
    let engine: Arc<dyn ContainerEngine> = Arc::new(MockEngine::new(EngineKind::Podman));

    EngineContract::new(
        engine,
        MOCK_IMAGE,
        ContractEnv::unmanaged(NETWORK_INTERNAL, NETWORK_EGRESS),
    )
}

#[tokio::test]
async fn ping_is_reachability() {
    contract().ping_is_reachability().await;
}

#[tokio::test]
async fn an_image_that_is_there_is_not_an_error() {
    contract().an_image_that_is_there_is_not_an_error().await;
}

#[tokio::test]
async fn an_existing_network_is_ok() {
    contract().an_existing_network_is_ok().await;
}

#[tokio::test]
async fn create_connect_start_and_wait_in_that_order() {
    contract()
        .create_connect_start_and_wait_in_that_order()
        .await;
}

#[tokio::test]
async fn a_name_in_use_is_a_conflict() {
    contract().a_name_in_use_is_a_conflict().await;
}

/// `ARCHITECTURE.md`: a start of a container that is already running is `Ok`,
/// because both engines answer 304 and the adapter reads it as success.
#[tokio::test]
async fn start_of_a_running_container_is_ok() {
    contract().start_of_a_running_container_is_ok().await;
}

/// `ARCHITECTURE.md`: stopping a container that is not running is `Ok`, whether
/// it has already exited or was never started.
#[tokio::test]
async fn stop_of_an_exited_container_is_ok() {
    contract().stop_of_an_exited_container_is_ok().await;
}

#[tokio::test]
async fn kill_of_an_exited_container_is_a_conflict() {
    contract().kill_of_an_exited_container_is_a_conflict().await;
}

/// `ARCHITECTURE.md`: removing a container that is not there is `Ok`, because a
/// container that is not there is already removed.
#[tokio::test]
async fn remove_of_a_missing_container_is_ok() {
    contract().remove_of_a_missing_container_is_ok().await;
}

#[tokio::test]
async fn remove_of_a_running_container_needs_force() {
    contract().remove_of_a_running_container_needs_force().await;
}

#[tokio::test]
async fn every_other_operation_on_a_missing_container_is_not_found() {
    contract()
        .every_other_operation_on_a_missing_container_is_not_found()
        .await;
}

#[tokio::test]
async fn list_by_label_reports_running_and_exited_containers() {
    contract()
        .list_by_label_reports_running_and_exited_containers()
        .await;
}

/// `ARCHITECTURE.md`: a named signal reaches the container's main process. The
/// mock records every signal and delivers it to the command, which exits on its
/// own `trap`; a command that traps nothing — every session's — is left running
/// for the test to end, which is the shape the stop-sequence tests need.
#[tokio::test]
async fn a_signal_reaches_the_containers_main_process() {
    contract()
        .a_signal_reaches_the_containers_main_process()
        .await;
}

/// `ARCHITECTURE.md`: dropping the stdin writer leaves the container's process
/// running, and a later attach is accepted (ADR 0034).
#[tokio::test]
async fn a_dropped_stdin_writer_leaves_the_container_running() {
    contract()
        .a_dropped_stdin_writer_leaves_the_container_running()
        .await;
}

/// `ARCHITECTURE.md`: a stdin write after the container exited returns an error,
/// never a silent success.
#[tokio::test]
async fn a_stdin_write_after_the_container_exited_fails() {
    contract()
        .a_stdin_write_after_the_container_exited_fails()
        .await;
}

#[tokio::test]
async fn exec_pty_on_a_container_that_is_not_running_is_a_conflict() {
    contract()
        .exec_pty_on_a_container_that_is_not_running_is_a_conflict()
        .await;
}

/// The whole suite in one call, which is how `tests/engine.rs` runs it: the
/// entry point any future adapter is held to.
#[tokio::test]
async fn the_whole_contract() {
    let engine: Arc<dyn ContainerEngine> = Arc::new(MockEngine::new(EngineKind::Podman));

    assert_engine_contract(
        engine,
        MOCK_IMAGE,
        ContractEnv::unmanaged(NETWORK_INTERNAL, NETWORK_EGRESS),
    )
    .await;
}
