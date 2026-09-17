//! `GET /api/health` against a real Postgres (`SPEC.md`, "Health").
//!
//! Self-contained on purpose: `TestApp` does not exist yet, so this file
//! starts its own container. When the database epic lands `TestApp::spawn()`,
//! this file is deleted and the ready case moves there with the rest.
//!
//! Needs a container engine (`DOCKER_HOST`); it runs in CI, not on a machine
//! without one.

use std::collections::HashMap;

use axum::http::StatusCode;
use axum_test::TestServer;
use mars_orchestrator::build_api_router;
use mars_orchestrator::email::PlaceholderEmailClient;
use mars_orchestrator::engine::PlaceholderEngine;
use mars_orchestrator::git::{CommitIdentity, PlaceholderCredentialProvider};
use mars_orchestrator::prelude::*;
use mars_orchestrator::secrets::{MASTER_KEY_LEN, SecretsKeyring};
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::ImageExt;
use testcontainers_modules::testcontainers::runners::AsyncRunner;

/// Obviously fake values apart from `DATABASE_URL`, which points at the
/// container this test started (rule 3).
fn test_config(database_url: &str) -> Config {
    let vars: HashMap<&str, String> = [
        ("PUBLIC_URL", "https://mars.example.invalid".to_string()),
        ("JWT_SECRET", "not-a-real-signing-secret".to_string()),
        ("DATABASE_URL", database_url.to_string()),
        (
            "DOCKER_HOST",
            "unix:///run/user/1000/podman/podman.sock".to_string(),
        ),
        ("DATA_DIR_HOST", "/srv/mars/data".to_string()),
        ("SECRETS_MASTER_KEYS", "1=not-a-real-key".to_string()),
        ("GIT_BOT_NAME", "Mars Bot".to_string()),
        ("GIT_BOT_EMAIL", "mars-bot@example.invalid".to_string()),
        (
            "SESSION_IMAGE_DEFAULT",
            "mars-session-claude:dev".to_string(),
        ),
    ]
    .into_iter()
    .collect();

    Config::from_vars(|name| vars.get(name).cloned()).expect("a complete required set loads")
}

#[tokio::test]
async fn health_endpoint_reports_ready_with_real_postgres() {
    let postgres = Postgres::default()
        .with_tag("18")
        .start()
        .await
        .expect("postgres starts");

    let database_url = format!(
        "postgres://postgres:postgres@{}:{}/postgres",
        postgres.get_host().await.expect("the container has a host"),
        postgres
            .get_host_port_ipv4(5432)
            .await
            .expect("the container publishes 5432"),
    );

    let config = test_config(&database_url);
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&config.database_url)
        .await
        .expect("the pool connects");

    // The startup placeholders, which is what `main` wires today; `ping`
    // succeeds, so this asserts the database half against a real Postgres.
    let identity = CommitIdentity {
        name: config.git_bot_name.clone(),
        email: config.git_bot_email.clone(),
    };
    let state = AppState::new(
        Arc::new(config),
        pool,
        Arc::new(PlaceholderEngine),
        Arc::new(PlaceholderEmailClient),
        Arc::new(PlaceholderCredentialProvider::new(identity)),
        SecretsKeyring::from_entries(vec![(1, [0u8; MASTER_KEY_LEN])])
            .expect("one entry is a valid keyring"),
    );
    let server = TestServer::new(build_api_router(state));

    let response = server.get("/api/health").await;

    response.assert_status(StatusCode::OK);
    response.assert_json(&json!({
        "orchestrator": true,
        "database": true,
        "engine": true,
    }));
}
