//! `GET /api/health` through `TestApp` (`SPEC.md`, "Health").
//!
//! The ready case is the smoke test of the whole harness: a real Postgres with
//! the migrations applied answers the database probe, the mock engine answers
//! the ping, and the response comes back through the library's own router and
//! middleware stack.
//!
//! The unhealthy engine is here too, because the mock is what makes it one
//! line; the database's own 503 branch needs neither a container nor the
//! harness and is unit tested beside the handler in `src/routes/health.rs`.
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use axum::http::StatusCode;
use common::TestApp;
use mars_orchestrator::engine::ContainerEngine;
use mars_orchestrator::engine::mock::MockEngine;
use serde::Deserialize;
use std::sync::Arc;

/// The documented body (`SPEC.md`, "Health"), mirrored here so a renamed or
/// dropped field fails the test rather than passing unnoticed.
#[derive(Debug, Deserialize)]
struct Health {
    orchestrator: bool,
    database: bool,
    engine: bool,
}

#[tokio::test]
async fn health_reports_the_orchestrator_the_database_and_the_engine_ready() {
    let app = TestApp::spawn().await;

    let response = app.server.get("/api/health").await;

    response.assert_status(StatusCode::OK);
    let health = response.json::<Health>();
    assert!(health.orchestrator);
    assert!(health.database, "the migrated container answers the probe");
    assert!(health.engine, "the mock engine answers the ping");

    // The mock the handler called is the mock `TestApp` holds, which is what
    // makes every later assertion on the mocks meaningful.
    assert_eq!(app.engine().pings(), 1, "the handler pinged exactly once");
}

/// An engine that does not answer is `engine: false` and 503, with the same
/// three-field body (`SPEC.md`, "Health").
#[tokio::test]
async fn health_reports_503_when_the_engine_does_not_answer() {
    let app = TestApp::spawn().await;
    app.engine().set_unhealthy(true);

    let response = app.server.get("/api/health").await;

    response.assert_status(StatusCode::SERVICE_UNAVAILABLE);
    let health = response.json::<Health>();
    assert!(health.orchestrator);
    assert!(health.database);
    assert!(!health.engine, "the mock engine refused the ping");
}

/// The mocks in `AppState` are the same allocations as the fields on
/// `TestApp`, reachable either way: directly, or by downcasting the
/// `Arc<dyn Trait>` a handler sees.
#[tokio::test]
async fn the_state_holds_the_same_mocks_as_the_test_app() {
    let app = TestApp::spawn().await;

    app.server.get("/api/health").await.assert_status_ok();

    let engine: &Arc<dyn ContainerEngine> = &app.state.engine;
    let downcast = engine
        .as_any()
        .downcast_ref::<MockEngine>()
        .expect("the state holds the mock engine");
    assert_eq!(downcast.pings(), app.engine().pings());

    // Nothing has sent mail or asked for a credential yet; the accessors are
    // here so the epics that do can assert on them.
    assert!(app.mock_email().sent().is_empty());
    assert!(app.mock_git().requested().is_empty());
}

/// Two apps in one binary get two independent databases on the process's one
/// Postgres, which is what lets `#[tokio::test]`s in a binary run in parallel
/// without isolating anything.
#[tokio::test]
async fn two_apps_run_side_by_side_on_independent_databases() {
    let (first, second) = tokio::join!(TestApp::spawn(), TestApp::spawn());

    assert_ne!(
        first.state.config.database_url, second.state.config.database_url,
        "each app must get its own database"
    );
    assert_ne!(
        first.data_dir.path(),
        second.data_dir.path(),
        "each app must get its own data directory"
    );

    first.server.get("/api/health").await.assert_status_ok();
    second.server.get("/api/health").await.assert_status_ok();

    // The seeded administrator is gone from both, so a user test starts from
    // an empty table.
    for app in [&first, &second] {
        let users: i64 = sqlx::query_scalar("SELECT count(*) FROM users")
            .fetch_one(&app.pool)
            .await
            .expect("the users table is readable");
        assert_eq!(users, 0, "spawn must remove the seeded administrator");
    }
}
