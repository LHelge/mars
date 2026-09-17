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

use axum_extra::extract::cookie::Cookie;
use axum_test::{TestResponse, TestServer};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use chrono::{TimeDelta, Utc};
use mars_orchestrator::build_api_router;
use mars_orchestrator::email::EmailClient;
use mars_orchestrator::email::mock::MockEmailClient;
use mars_orchestrator::engine::ContainerEngine;
use mars_orchestrator::engine::mock::MockContainerEngine;
use mars_orchestrator::git::GitCredentialProvider;
use mars_orchestrator::git::mock::MockGitCredentialProvider;
use mars_orchestrator::models::user::hash_password;
use mars_orchestrator::models::{Email, NewUser, User, Username};
use mars_orchestrator::prelude::*;
use mars_orchestrator::repositories::UserRepository;
use mars_orchestrator::secrets::SecretsKeyring;
use serde::Deserialize;
use serde_json::Value;
use tempfile::TempDir;
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::ContainerAsync;
use uuid::Uuid;

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
/// Not a credential: an obviously fake stand-in for an Argon2id PHC string,
/// so arranging a user costs no hashing (`CLAUDE.md`, rule 3). Nothing
/// verifies against it; a test that logs in hashes a real fake password
/// itself.
const FAKE_PASSWORD_HASH: &str = "$argon2id$fake$hash";

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

    /// Forget every failed login and password-reset request counted so far.
    ///
    /// A fresh `TestApp` already starts with both limiters empty, so this is
    /// only for a test that drives several scenarios through one app: one that
    /// deliberately blocks a key and then wants to log in again.
    ///
    /// The limiters key on the client address as well as on the username, and
    /// `axum-test`'s mock transport has no peer, so every request here comes
    /// from the loopback fallback (`routes::throttle::client_addr`). A test
    /// that needs two distinct clients sets `X-Forwarded-For` itself.
    pub fn reset_limiters(&self) {
        self.state.login_throttle.reset();
        self.state.reset_rate_limit.reset();
    }

    /// Insert a user directly, bypassing the routes.
    ///
    /// Every route test needs a row to authenticate as, and most of them do
    /// not care how it got there, so this is the one-call arrangement: a
    /// committed `users` row with [`FAKE_PASSWORD_HASH`] and the two
    /// authorization flags the caller asked for. A test that cares about the
    /// password itself hashes its own and inserts through
    /// [`UserRepository`].
    pub async fn insert_user(
        &self,
        username: &str,
        email: &str,
        admin: bool,
        must_change_password: bool,
    ) -> User {
        let user = NewUser {
            id: Uuid::new_v4(),
            username: Username::parse(username).expect("the test username is valid"),
            email: Email::parse(email).expect("the test email is valid"),
            password_hash: FAKE_PASSWORD_HASH.to_string(),
            admin,
            must_change_password,
        };

        let mut tx = self.pool.begin().await.expect("a transaction begins");
        let inserted = UserRepository::new(&self.pool)
            .insert(&mut tx, &user)
            .await
            .expect("the test user inserts");
        tx.commit().await.expect("the transaction commits");

        inserted
    }

    /// A valid access token for `user`, minted from the harness
    /// configuration.
    ///
    /// The same `Claims::for_user` the login route uses, so a test token is
    /// indistinguishable from a real one — including `auth_version`, which is
    /// what makes a token minted before a password change fail afterwards.
    /// A test that wants a *wrong* claim (an `admin` snapshot the row
    /// contradicts, a stale `auth_version`) edits the struct and calls
    /// [`TestApp::encode`].
    pub fn token_for(&self, user: &User) -> String {
        self.encode(&Claims::for_user(user, Utc::now()))
    }

    /// An access token for `user` that expired an hour ago.
    pub fn expired_token_for(&self, user: &User) -> String {
        self.encode(&Claims::for_user(user, Utc::now() - TimeDelta::hours(1)))
    }

    /// Sign `claims` with the harness `JWT_SECRET`.
    pub fn encode(&self, claims: &Claims) -> String {
        claims
            .encode(&self.state.config)
            .expect("the test claims sign")
    }

    /// Insert a user whose password actually verifies.
    ///
    /// [`TestApp::insert_user`] stores [`FAKE_PASSWORD_HASH`], which nothing
    /// can authenticate against; a test that drives `POST /api/auth/login`
    /// needs a real Argon2id hash of a known password, and hashing one costs
    /// tens of milliseconds, so it is a separate call rather than a cost every
    /// arrangement pays.
    ///
    /// `password` is an obviously fake test password like every other value
    /// here (rule 3).
    pub async fn insert_user_with_password(
        &self,
        username: &str,
        email: &str,
        password: &str,
        admin: bool,
        must_change_password: bool,
    ) -> User {
        let user = NewUser {
            id: Uuid::new_v4(),
            username: Username::parse(username).expect("the test username is valid"),
            email: Email::parse(email).expect("the test email is valid"),
            password_hash: hash_password(password).expect("the test password hashes"),
            admin,
            must_change_password,
        };

        let mut tx = self.pool.begin().await.expect("a transaction begins");
        let inserted = UserRepository::new(&self.pool)
            .insert(&mut tx, &user)
            .await
            .expect("the test user inserts");
        tx.commit().await.expect("the transaction commits");

        inserted
    }

    /// Log in through the real route and return the body and the raw refresh
    /// cookie value.
    ///
    /// Asserts 200, so it is the arrangement step for tests that are about
    /// what happens *after* a login; a test asserting on a failed login posts
    /// to `/api/auth/login` itself.
    ///
    /// The cookie is returned rather than stored because `axum-test` does not
    /// keep cookies between requests unless asked to, and the refresh tests
    /// need to present an old value deliberately.
    pub async fn login(&self, username: &str, password: &str) -> (TokenPair, String) {
        let response = self
            .server
            .post("/api/auth/login")
            .json(&serde_json::json!({ "username": username, "password": password }))
            .await;

        response.assert_status_ok();
        let cookie = response.cookie(REFRESH_COOKIE).value().to_string();

        (response.json::<TokenPair>(), cookie)
    }

    /// Present `cookie` at `POST /api/auth/refresh`.
    ///
    /// The whole response, not a parsed body: half of what the refresh tests
    /// assert is the status and the `Set-Cookie` of a *rejection*.
    pub async fn refresh(&self, cookie: &str) -> TestResponse {
        self.server
            .post("/api/auth/refresh")
            .add_cookie(Cookie::new(REFRESH_COOKIE, cookie.to_string()))
            .await
    }
}

/// The `{ user, access_token }` body the credential-issuing routes answer with
/// (`SPEC.md`, "Authentication").
///
/// `user` stays a `Value` on purpose: the route's own response type is
/// `pub(crate)` and `User` has no `Deserialize` (deriving one would make an
/// empty `password_hash` constructible from JSON), and what the tests assert
/// about it is partly which keys are *absent*.
#[derive(Debug, Clone, Deserialize)]
pub struct TokenPair {
    pub user: Value,
    pub access_token: String,
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
