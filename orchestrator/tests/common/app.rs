//! `TestApp`: the integration-test entry point every backend epic uses
//! (`CLAUDE.md`, "Testing expectations").
//!
//! [`TestApp::spawn`] starts a throw-away Postgres, applies the migrations,
//! removes the seeded administrator, builds an [`AppState`] out of the mock
//! engine, the mock email client, the mock git credential provider and the
//! fixed test master key, and serves the library's router through
//! `axum-test`. Tests then drive the real middleware stack in-process: no
//! listener is bound, no cron job runs and no session owner is started. The
//! epics that own those start them explicitly.
//!
//! Every configuration value is obviously fake (rule 3). The only one that
//! points at anything real is `DATABASE_URL`, and what it points at is the
//! container this `TestApp` started and stops again when it drops.
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

use std::collections::HashMap;
use std::path::Path;

use axum_test::TestServer;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use mars_orchestrator::build_api_router;
use mars_orchestrator::email::EmailClient;
use mars_orchestrator::email::mock::MockEmailClient;
use mars_orchestrator::engine::ContainerEngine;
use mars_orchestrator::engine::mock::MockContainerEngine;
use mars_orchestrator::git::GitCredentialProvider;
use mars_orchestrator::git::mock::MockGitCredentialProvider;
use mars_orchestrator::prelude::*;
use mars_orchestrator::secrets::SecretsKeyring;
use tempfile::TempDir;
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::ContainerAsync;

use super::db;

/// Remove the administrator the `users` migration seeds at the fixed id
/// `00000000-0000-0000-0000-000000000001` (`docs/data-model.md`; ADR 0024).
///
/// It goes before the router is built, so no test can come to depend on it:
/// user tests create the users they need and a count over `users` starts at
/// zero.
///
/// A literal rather than a `query!`: this file introduces no compile-time
/// checked query, so `.sqlx/` never has to carry one for the harness
/// (`CLAUDE.md`, "Backend conventions").
const DELETE_SEEDED_ADMIN: &str =
    "DELETE FROM users WHERE id = '00000000-0000-0000-0000-000000000001'";

/// A running orchestrator with every collaborator mocked.
///
/// Field order is drop order: the server, the pool and the mocks go first and
/// the container guard `_db` last, so Postgres is still up while anything that
/// might still talk to it is torn down.
pub struct TestApp {
    /// The router under test, driven in-process by `axum-test`.
    pub server: TestServer,
    /// A pool on the same database the app uses, for arranging fixtures and
    /// asserting on rows.
    pub pool: PgPool,
    /// The state the router was built with, cloned into every handler.
    pub state: AppState,
    /// The same allocation as `state.engine`.
    pub engine: Arc<MockContainerEngine>,
    /// The same allocation as `state.email`; invite and password-reset flows
    /// are asserted through the messages it captured.
    pub email: Arc<MockEmailClient>,
    /// The same allocation as `state.git_credentials`.
    pub git: Arc<MockGitCredentialProvider>,
    /// `DATA_DIR` and `DATA_DIR_HOST`. Held here because it has to outlive the
    /// app: dropping it removes the directory.
    pub data_dir: TempDir,
    /// The Postgres container. Never read; it stops the container on drop.
    _db: ContainerAsync<Postgres>,
}

impl TestApp {
    /// Start a fresh app: one container, one migrated database, one router.
    ///
    /// One container per test is the expected cost; nothing is shared between
    /// tests, so there is nothing to isolate and nothing to clean up.
    pub async fn spawn() -> TestApp {
        let (postgres, pool) = db::test_pool().await;
        let database_url = db::connection_string(&postgres).await;

        // After the migrations and before the router: the first request must
        // never see the seeded administrator.
        sqlx::query(DELETE_SEEDED_ADMIN)
            .execute(&pool)
            .await
            .expect("the seeded administrator is removed");

        let data_dir = tempfile::tempdir().expect("a temporary data directory");
        let keyring = SecretsKeyring::test_key();
        let config = test_config(&database_url, data_dir.path(), &keyring);

        // Built as concrete mocks first and coerced afterwards, so the
        // `Arc<dyn Trait>` in the state and the `Arc<Mock…>` on `TestApp` are
        // the same allocation: what a handler sends, `app.email.sent()` sees.
        let engine = Arc::new(MockContainerEngine::new());
        let email = Arc::new(MockEmailClient::new());
        let git = Arc::new(MockGitCredentialProvider::new());

        // `AppState::new` takes everything the state holds today. The fields
        // later epics add in place — the `SessionRegistry` of the session
        // lifecycle epic and the broadcast senders for event fan-out
        // (`ARCHITECTURE.md`, "Orchestrator internals") — are constructed here
        // with their own `Default`/`new()` when they arrive, and `TestApp`
        // grows a field for the ones a test has to reach.
        let state = AppState::new(
            Arc::new(config),
            pool.clone(),
            Arc::clone(&engine) as Arc<dyn ContainerEngine>,
            Arc::clone(&email) as Arc<dyn EmailClient>,
            Arc::clone(&git) as Arc<dyn GitCredentialProvider>,
            keyring,
        );

        // The library's own router, so tests exercise the real middleware
        // stack. Once the authentication epic adds them, the
        // `integration-tests`-only routes (`SPEC.md`, "Test-only routes") are
        // part of `build_api_router` and reachable from here for free.
        let server = TestServer::new(build_api_router(state.clone()));

        TestApp {
            server,
            pool,
            state,
            engine,
            email,
            git,
            data_dir,
            _db: postgres,
        }
    }

    /// The engine the router calls. Also reachable from an
    /// `Arc<dyn ContainerEngine>` through `ContainerEngine::as_any`.
    pub fn mock_engine(&self) -> &MockContainerEngine {
        &self.engine
    }

    /// The email client the router sends through. Also reachable from an
    /// `Arc<dyn EmailClient>` through `EmailClient::as_any`.
    pub fn mock_email(&self) -> &MockEmailClient {
        &self.email
    }

    /// The credential provider the router asks. Also reachable from an
    /// `Arc<dyn GitCredentialProvider>` through
    /// `GitCredentialProvider::as_any`.
    pub fn mock_git(&self) -> &MockGitCredentialProvider {
        &self.git
    }
}

/// The configuration `spawn` builds, from values in code rather than from the
/// process environment: tests never depend on the developer's `.env` and never
/// mutate process-global state.
///
/// Everything here is obviously fake (rule 3). `SECRETS_MASTER_KEYS` is
/// rendered from `keyring` rather than pasted, so the string and the
/// [`SecretsKeyring`] injected beside it can never drift apart.
fn test_config(database_url: &str, data_dir: &Path, keyring: &SecretsKeyring) -> Config {
    let version = keyring.current_version();
    let key = keyring
        .key(version)
        .expect("the test keyring carries its current key");
    let data_dir = data_dir.display().to_string();

    let vars: HashMap<&str, String> = [
        ("PUBLIC_URL", "http://localhost".to_string()),
        (
            "JWT_SECRET",
            "test-jwt-secret-not-for-production".to_string(),
        ),
        ("DATABASE_URL", database_url.to_string()),
        // Required by `Config` and unused by the mock engine: nothing in a
        // test opens an engine socket, so this path deliberately does not
        // exist.
        (
            "DOCKER_HOST",
            "unix:///nonexistent/mars-test/podman.sock".to_string(),
        ),
        // The orchestrator's view and the "host" view are the same directory:
        // nothing bind-mounts anything here.
        ("DATA_DIR", data_dir.clone()),
        ("DATA_DIR_HOST", data_dir),
        (
            "SECRETS_MASTER_KEYS",
            format!("{version}={}", STANDARD.encode(key)),
        ),
        ("GIT_BOT_NAME", "Mars Test Bot".to_string()),
        ("GIT_BOT_EMAIL", "bot@example.test".to_string()),
        (
            "SESSION_IMAGE_DEFAULT",
            "mars-session-stub:test".to_string(),
        ),
        // Nothing binds a port here, and a test that does need a real listener
        // binds it on port 0 itself and reads the address back (`lib.rs`,
        // `run`). `MCP_PORT` keeps its default because `Config` rejects two
        // equal ports, so the two cannot both be 0.
        ("API_PORT", "0".to_string()),
        // No `RESEND_API_KEY`: mail goes to the mock, and setting a key would
        // only make `MAIL_FROM` required as well (ADR 0026).
    ]
    .into_iter()
    .collect();

    Config::from_vars(|name| vars.get(name).cloned()).expect("test config")
}
