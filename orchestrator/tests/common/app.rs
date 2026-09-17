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

use axum::http::StatusCode;
use axum_extra::extract::cookie::Cookie;
use axum_test::{TestRequest, TestResponse, TestServer};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use chrono::{TimeDelta, Utc};
use mars_orchestrator::build_api_router;
use mars_orchestrator::email::EmailClient;
use mars_orchestrator::email::mock::MockEmailClient;
use mars_orchestrator::engine::ContainerEngine;
use mars_orchestrator::engine::mock::MockEngine;
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
    pub engine: Arc<MockEngine>,
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
        let engine = Arc::new(MockEngine::default());
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
    pub fn engine(&self) -> &MockEngine {
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

    /// Create a signed-in ordinary user through `POST /api/test/users`
    /// (`SPEC.md`, "Test-only routes").
    ///
    /// The one-call arrangement every later epic's tests start from: a
    /// committed `users` row with `must_change_password` false, an access
    /// token that works on every route and the refresh cookie the route set.
    /// It goes through the real route rather than the repository, so a test
    /// that later refreshes or logs out is using credentials the application
    /// itself issued.
    ///
    /// Asserts 201. A test about this route's *failures* posts to
    /// `/api/test/users` itself.
    ///
    /// `password` is an obviously fake test password like every other value
    /// here (rule 3), and has to be 10–128 characters or the route answers
    /// 400.
    pub async fn create_user(
        &self,
        username: &str,
        email: &str,
        password: &str,
    ) -> AuthenticatedUser {
        self.create(username, email, password, false).await
    }

    /// [`TestApp::create_user`] with `admin: true`.
    ///
    /// Creating an administrator here never consults the
    /// administrator-membership invariant: the route only adds one
    /// (`SPEC.md`, "Users").
    pub async fn create_admin(
        &self,
        username: &str,
        email: &str,
        password: &str,
    ) -> AuthenticatedUser {
        self.create(username, email, password, true).await
    }

    /// A user who has to change their password, with a token that already
    /// works.
    ///
    /// Not [`TestApp::create_user`]: `POST /api/test/users` deliberately has
    /// no way to raise `must_change_password` — it exists to hand out a user
    /// who can go straight to the routes under test. So the row goes in
    /// through the repository and the pair is minted from it, which is enough
    /// for the gate tests: what they assert is which routes a *gated* token
    /// reaches, and there is no refresh cookie in that question.
    ///
    /// `refresh_cookie` is therefore empty. A gate test that also needs a
    /// usable cookie logs in with [`TestApp::insert_user_with_password`] and
    /// [`TestApp::login`], which is the flow a real gated user follows.
    pub async fn create_gated_user(&self, username: &str, email: &str) -> AuthenticatedUser {
        let user = self.insert_user(username, email, false, true).await;
        let access_token = self.token_for(&user);

        AuthenticatedUser {
            access_token,
            refresh_cookie: String::new(),
            user,
        }
    }

    /// `GET path` as `user`.
    ///
    /// The four verbs below are the request builders every route test uses:
    /// `axum-test` builds a request per call, so there is no persistent
    /// authenticated client to hand out — attaching the bearer header is what
    /// a "client" amounts to here. The returned [`TestRequest`] is still open
    /// for `.json(..)`, `.add_header(..)` and the rest before it is awaited.
    pub fn get_as(&self, user: &AuthenticatedUser, path: &str) -> TestRequest {
        self.server
            .get(path)
            .authorization_bearer(&user.access_token)
    }

    /// `POST path` as `user`.
    pub fn post_as(&self, user: &AuthenticatedUser, path: &str) -> TestRequest {
        self.server
            .post(path)
            .authorization_bearer(&user.access_token)
    }

    /// `PUT path` as `user`.
    pub fn put_as(&self, user: &AuthenticatedUser, path: &str) -> TestRequest {
        self.server
            .put(path)
            .authorization_bearer(&user.access_token)
    }

    /// `PATCH path` as `user`.
    pub fn patch_as(&self, user: &AuthenticatedUser, path: &str) -> TestRequest {
        self.server
            .patch(path)
            .authorization_bearer(&user.access_token)
    }

    /// `DELETE path` as `user`.
    pub fn delete_as(&self, user: &AuthenticatedUser, path: &str) -> TestRequest {
        self.server
            .delete(path)
            .authorization_bearer(&user.access_token)
    }

    /// The body of [`TestApp::create_user`] and [`TestApp::create_admin`].
    async fn create(
        &self,
        username: &str,
        email: &str,
        password: &str,
        admin: bool,
    ) -> AuthenticatedUser {
        let response = self
            .server
            .post("/api/test/users")
            .json(&serde_json::json!({
                "username": username,
                "email": email,
                "password": password,
                "admin": admin,
            }))
            .await;

        response.assert_status(StatusCode::CREATED);
        let refresh_cookie = response.cookie(REFRESH_COOKIE).value().to_string();
        let pair = response.json::<TokenPair>();

        // The route's response body carries the `User` DTO, which has no
        // `Deserialize`; the row is read back instead, so callers get the same
        // `User` the other arrangement helpers hand out.
        let id: Uuid = pair.user["id"]
            .as_str()
            .expect("the created user carries an id")
            .parse()
            .expect("the id is a uuid");
        let user = UserRepository::new(&self.pool)
            .find(id)
            .await
            .expect("the lookup runs")
            .expect("the created user is there");

        AuthenticatedUser {
            user,
            access_token: pair.access_token,
            refresh_cookie,
        }
    }
}

/// A user and the credentials they were signed in with.
///
/// What [`TestApp::create_user`] and its siblings return, and what the
/// `*_as` request builders take: a test that needs to act as somebody holds
/// one of these rather than a `(User, String)` pair it has to keep together
/// itself.
#[derive(Debug, Clone)]
pub struct AuthenticatedUser {
    /// The `users` row, read back after the route created it.
    pub user: User,
    /// A valid access token for [`AuthenticatedUser::user`].
    pub access_token: String,
    /// The raw value of the `refresh_token` cookie the route set, or empty
    /// when the user was not created through a route that sets one (see
    /// [`TestApp::create_gated_user`]).
    pub refresh_cookie: String,
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
