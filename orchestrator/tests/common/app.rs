//! `TestApp`: the integration-test entry point every backend epic uses
//! (`CLAUDE.md`, "Testing expectations").
//!
//! [`TestApp::spawn`] takes a throw-away database on the process's shared
//! Postgres, already migrated because it is a clone of the migrated template
//! (`tests/common/db.rs`), removes the seeded administrator, builds an
//! [`AppState`] out of the mock
//! engine, the mock email client, the mock git credential provider and the
//! fixed test master key, and serves the library's router through
//! `axum-test`. Tests then drive the real middleware stack in-process: no TCP
//! listener is bound, no cron job runs and no session owner is started. The
//! epics that own those start them explicitly.
//!
//! The one background task that *is* started is the shared Postgres listener
//! (`mars_orchestrator::events::spawn_listener`), because without it
//! `state.fanout` would stay silent and every stream test would be asserting
//! on a fan-out nothing publishes to. It stops when the `TestApp` drops.
//!
//! [`TestApp::spawn_with_engine`] is the same app over the engine
//! `DOCKER_HOST` names instead of the mock, for the one suite that runs
//! sessions in real containers (`tests/session_e2e.rs`). It is the only spawn
//! that creates anything outside the database and the temporary data
//! directory, and [`TestApp::cleanup_engine`] is what removes it again.
//!
//! Every configuration value is obviously fake (rule 3). The only ones that
//! point at anything real are `DATABASE_URL`, and — with the real engine —
//! `DOCKER_HOST` and the data directory the containers bind.
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use axum::http::StatusCode;
use axum_extra::extract::cookie::Cookie;
use axum_test::{TestRequest, TestResponse, TestServer, TestWebSocket};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use chrono::{TimeDelta, Utc};
use mars_orchestrator::auth::REFRESH_COOKIE;
use mars_orchestrator::build_api_router;
use mars_orchestrator::email::EmailClient;
use mars_orchestrator::email::mock::MockEmailClient;
use mars_orchestrator::engine::mock::MockEngine;
use mars_orchestrator::engine::{ContainerEngine, EngineKind, LABEL_SESSION_ID, bootstrap_engine};
use mars_orchestrator::git::GitCredentialProvider;
use mars_orchestrator::git::mock::MockGitCredentialProvider;
use mars_orchestrator::models::user::hash_password;
use mars_orchestrator::models::{Email, NewUser, User, Username};
use mars_orchestrator::prelude::*;
use mars_orchestrator::repositories::UserRepository;
use mars_orchestrator::secrets::SecretsKeyring;
use mars_orchestrator::session::{RecoveryReport, SessionRegistry, TAIL_POLL_INTERVAL};
use serde::Deserialize;
use serde_json::Value;
use tempfile::TempDir;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
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

/// The stream periods every [`TestApp`] runs on: fast enough to assert on,
/// far enough apart to tell the three of them apart in a log.
pub const TEST_TIMINGS: StreamTimings = StreamTimings {
    ping: Duration::from_millis(200),
    safety_read: Duration::from_millis(300),
    sse_keepalive: Duration::from_millis(100),
};

const DELETE_SEEDED_ADMIN: &str =
    "DELETE FROM users WHERE id = '00000000-0000-0000-0000-000000000001'";

/// The `PUBLIC_URL` [`test_config`] sets, which every emailed link is built
/// on.
const PUBLIC_URL: &str = "http://localhost";

/// The administrator invite collection.
const INVITES: &str = "/api/users/invites";

/// A running orchestrator with every collaborator mocked — or, from
/// [`TestApp::spawn_with_engine`], with the real container engine in the mock's
/// place.
///
/// Field order is drop order: the server, the pool and the mocks go first and
/// the database guard `_db` last, so the database is still there while
/// anything that might still talk to it is torn down.
pub struct TestApp {
    /// The router under test, driven in-process by `axum-test`.
    pub server: TestServer,
    /// The same router over a real HTTP transport on a random port.
    ///
    /// `server` uses `axum-test`'s mock transport, which a WebSocket upgrade
    /// cannot travel over and which the rest of the suite relies on (it has no
    /// peer address, which is what the throttle tests are written against), so
    /// the streaming endpoints get a second server rather than a changed one.
    /// Both serve the same [`AppState`], so a fixture arranged through one is
    /// visible to the other.
    pub http_server: TestServer,
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
    /// The shared Postgres listener `run` starts, started here too so that a
    /// test subscribing to `state.fanout` sees live notifications from the
    /// rows it writes (`ARCHITECTURE.md`, "Event delivery").
    pub listener: ListenerHandle,
    /// The two session networks [`TestApp::spawn_with_engine`] had the real
    /// engine create, and [`TestApp::cleanup_engine`] removes again; `None` for
    /// a [`TestApp::spawn`], which creates no network.
    networks: Option<(String, String)>,
    /// This app's database on the shared Postgres. Never read; it drops the
    /// database on drop.
    _db: db::TestDatabase,
}

/// The shared Postgres listener task and the token that stops it.
///
/// A test that wants to watch the listener reconnect cancels nothing and just
/// kills the backend; [`TestApp`]'s `Drop` is what stops it at the end of a
/// scenario, the way `run` does at shutdown.
pub struct ListenerHandle {
    /// The task `spawn_listener` returned. Never awaited by `Drop`, which is
    /// not async; cancelling it is enough, because the connection goes away
    /// with the database.
    pub handle: JoinHandle<()>,
    /// Cancelling this makes the task drop its connection and return.
    pub shutdown: CancellationToken,
}

impl Drop for TestApp {
    /// Stop the listener, the way `run` does once the servers have stopped.
    ///
    /// Nothing is awaited: `Drop` is not async and the test's runtime is
    /// already going away. The task's connection is closed by the pool and the
    /// database it was on is dropped `WITH (FORCE)` (`tests/common/db.rs`).
    fn drop(&mut self) {
        self.listener.shutdown.cancel();
    }
}

impl TestApp {
    /// Start a fresh app: one migrated database, one router.
    ///
    /// The database is this app's alone — cloned from the process's migrated
    /// template — so nothing is shared between tests and there is nothing to
    /// isolate or clean up. Only the first `spawn` in a binary pays a
    /// container start (`tests/common/db.rs`).
    pub async fn spawn() -> TestApp {
        Self::build(None).await
    }

    /// Start a fresh app over the **real** engine `DOCKER_HOST` names, with
    /// `image` as `SESSION_IMAGE_DEFAULT` (`tests/session_e2e.rs`).
    ///
    /// Everything else is [`TestApp::spawn`]'s: the same throw-away database,
    /// the same mock email client and git credential provider, the same fixed
    /// test master key. What differs is everything a container needs to exist:
    ///
    /// - `DATA_DIR` and `DATA_DIR_HOST` are the same temporary directory, as
    ///   they are for an orchestrator running on the host (`ARCHITECTURE.md`,
    ///   "Development on the host");
    /// - `MCP_URL` points at the host gateway under the name this engine uses
    ///   (`host.containers.internal` on Podman, `host.docker.internal` on
    ///   Docker) and `SESSION_EXTRA_HOSTS` maps that name to `host-gateway`,
    ///   so a session container's `mcp.json` names a reachable host even
    ///   though nothing in the end-to-end scenarios calls MCP;
    /// - the two session networks are this app's alone, so parallel scenarios
    ///   never remove each other's, and [`TestApp::cleanup_engine`] removes
    ///   them;
    /// - `STOP_GRACE_SECS` is 5 rather than 1: a real container has to be able
    ///   to handle its `SIGINT` before the owner escalates, or the recorded
    ///   signal would be `SIGTERM` (`ARCHITECTURE.md`, "Stop semantics").
    ///
    /// The whole of [`bootstrap_engine`] runs, startup probe included, so a
    /// host that does not honour the uid contract fails here with the probe's
    /// own message rather than later as a session that cannot write its
    /// checkout (`ARCHITECTURE.md`, "Uid contract").
    ///
    /// Panics when `DOCKER_HOST` is unset: a caller reaches this only after its
    /// own guard has established that there is an engine to talk to.
    pub async fn spawn_with_engine(image: &str) -> TestApp {
        let docker_host = super::engine::docker_host().expect("DOCKER_HOST is set");
        let kind = real_engine_kind(&docker_host).await;
        let suffix = Uuid::new_v4().simple().to_string()[..8].to_string();

        Self::build(Some(RealEngineSetup {
            docker_host,
            image: image.to_string(),
            network_internal: format!("mars-e2e-int-{suffix}"),
            network_egress: format!("mars-e2e-egress-{suffix}"),
            gateway_host: gateway_host(kind).to_string(),
        }))
        .await
    }

    /// The body of both spawns: `None` is the mock engine, `Some` the real one.
    async fn build(real: Option<RealEngineSetup>) -> TestApp {
        // One connection more than the default, because the shared listener
        // below holds one of them permanently — the same allowance the
        // production pool of 20 makes (`main.rs`).
        let (database, pool) = db::test_pool_with(db::DEFAULT_MAX_CONNECTIONS + 1).await;
        let database_url = database.url().to_string();

        // After the migrations and before the router: the first request must
        // never see the seeded administrator.
        sqlx::query(DELETE_SEEDED_ADMIN)
            .execute(&pool)
            .await
            .expect("the seeded administrator is removed");

        let data_dir = tempfile::tempdir().expect("a temporary data directory");
        let keyring = SecretsKeyring::test_key();
        let config = test_config(&database_url, data_dir.path(), &keyring, real.as_ref());

        // Built as concrete mocks first and coerced afterwards, so the
        // `Arc<dyn Trait>` in the state and the `Arc<Mock…>` on `TestApp` are
        // the same allocation: what a handler sends, `app.email.sent()` sees.
        let engine = Arc::new(MockEngine::default());
        let email = Arc::new(MockEmailClient::new());
        let git = Arc::new(MockGitCredentialProvider::new());

        // The networks and the startup probe, exactly as the binary runs them.
        // Held as `Arc<dyn ContainerEngine>` because that is what the state
        // takes; `app.engine` keeps the mock nothing then calls.
        let (state_engine, networks): (Arc<dyn ContainerEngine>, _) = match &real {
            Some(setup) => (
                bootstrap_engine(&config)
                    .await
                    .expect("the engine DOCKER_HOST names bootstraps"),
                Some((setup.network_internal.clone(), setup.network_egress.clone())),
            ),
            None => (Arc::clone(&engine) as Arc<dyn ContainerEngine>, None),
        };

        // `AppState::new` takes everything the state holds today, including
        // the empty `SessionRegistry` and the empty `EventFanout`: the fields
        // that hold no configuration and have no collaborator to mock are
        // constructed there with their own `new()`, so a test subscribes to
        // the very fan-out the handlers publish on, through `app.state`.
        let state = AppState::new(
            Arc::new(config),
            pool.clone(),
            state_engine,
            Arc::clone(&email) as Arc<dyn EmailClient>,
            Arc::clone(&git) as Arc<dyn GitCredentialProvider>,
            keyring,
        );

        // The hook `main` installs, so a test that ends a session through the
        // real path also runs the real release of the tasks it held
        // (`tracker::hooks`). A test that wants to observe the hook instead
        // installs its own on a clone of the state, which replaces this one.
        let state = {
            let hook = mars_orchestrator::tracker::session_ended_hook(&state);
            state.with_session_ended_hook(hook)
        };

        // Milliseconds rather than the documented tens of seconds: a stream
        // test asserts that a ping, a safety read or a keepalive *happens*,
        // and waiting out the real period would cost a minute per scenario
        // (`StreamTimings`).
        let state = state.with_realtime_timings(TEST_TIMINGS);

        // The library's own router, so tests exercise the real middleware
        // stack. Once the authentication epic adds them, the
        // `integration-tests`-only routes (`SPEC.md`, "Test-only routes") are
        // part of `build_api_router` and reachable from here for free.
        let server = TestServer::new(build_api_router(state.clone()));

        // The WebSocket and SSE endpoints, over a transport that can carry an
        // upgrade and a streaming body. A random port, so parallel test
        // binaries never collide.
        let http_server = TestServer::builder()
            .http_transport()
            .build(build_api_router(state.clone()));

        // The shared listener `run` starts, started here for the same reason:
        // a test that subscribes to `state.fanout` is subscribing to the very
        // channels a committed write wakes, over a real `LISTEN`.
        let listener_shutdown = CancellationToken::new();
        let listener = ListenerHandle {
            handle: mars_orchestrator::events::spawn_listener(
                pool.clone(),
                state.fanout.clone(),
                listener_shutdown.clone(),
            )
            .await,
            shutdown: listener_shutdown,
        };

        TestApp {
            server,
            http_server,
            pool,
            state,
            engine,
            email,
            git,
            data_dir,
            listener,
            networks,
            _db: database,
        }
    }

    /// The engine the router calls. Also reachable from an
    /// `Arc<dyn ContainerEngine>` through `ContainerEngine::as_any`.
    ///
    /// Only meaningful for a [`TestApp::spawn`]: an app built by
    /// [`TestApp::spawn_with_engine`] serves the real engine and this mock is
    /// then an allocation nothing calls, so asking for it is a mistake worth
    /// failing on.
    pub fn engine(&self) -> &MockEngine {
        assert!(
            self.networks.is_none(),
            "this app was spawned with the real engine; use state.engine",
        );
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

    /// The registry the handlers reach session owners through
    /// (`ARCHITECTURE.md`, "Session owner task").
    ///
    /// The same registry, not a copy: a test that registers an owner here sees
    /// what a handler forwards to it, and a test that asserts on a handler's
    /// input reads the queue from here.
    pub fn session_registry(&self) -> &SessionRegistry {
        &self.state.session_registry
    }

    /// Run startup recovery against this app's database, data directory and
    /// engine (`ARCHITECTURE.md`, "Restart procedure").
    ///
    /// What a restart amounts to in a test: the registry is emptied first,
    /// because a new process starts with an empty one and the queues in it are
    /// not durable (ADR 0020), and then the same `recover` `main.rs` calls runs
    /// over the rows and containers that survived. A test that wants to simulate
    /// a crash aborts its owner task, calls this, and asserts on what the new
    /// owner did.
    pub async fn recover(&self) -> RecoveryReport {
        self.state.session_registry.clear();

        mars_orchestrator::session::recover(&self.state)
            .await
            .expect("startup recovery runs")
    }

    /// Lose every session owner without touching a container, the way a killed
    /// process does (`ARCHITECTURE.md`, "Restart procedure").
    ///
    /// Clearing the registry drops the sender half of each owner's command
    /// channel, and an owner whose channel has closed leaves its loop and
    /// deliberately touches neither the registry nor the container — it is the
    /// same stand-down `OwnerCommand::Shutdown` asks for, without needing a
    /// handle to a live owner that a restarted process would not have either.
    ///
    /// The short settle is for the owner *task*, not for any state a test
    /// asserts: a closed channel wakes it on its next poll, and giving it that
    /// moment means the adopting owner is not committing a line while the old
    /// one still is. Correctness does not depend on it — the offset recheck
    /// under the session row lock is what prevents a double append — only the
    /// tidiness of the logs does.
    pub async fn simulate_restart(&self) {
        self.state.session_registry.clear();
        tokio::time::sleep(3 * TAIL_POLL_INTERVAL).await;
    }

    /// Remove what a real-engine app left on the engine, and report what could
    /// not be removed.
    ///
    /// The containers of *this app's* sessions — matched on the
    /// `mars.session_id` label against the `sessions` table, so a scenario
    /// running beside this one is never touched — and then this app's two
    /// networks, which cannot be removed while a container is still attached.
    /// Idempotent and safe to call on a [`TestApp::spawn`], where there is
    /// nothing to do.
    pub async fn cleanup_engine(&self) -> Vec<String> {
        let Some((internal, egress)) = self.networks.clone() else {
            return Vec::new();
        };
        let mut failures = Vec::new();

        let mine: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM sessions")
            .fetch_all(&self.pool)
            .await
            .unwrap_or_default();

        match self.state.engine.list_by_label(LABEL_SESSION_ID).await {
            Ok(listed) => {
                for summary in listed.iter().filter(|summary| {
                    summary
                        .labels
                        .get(LABEL_SESSION_ID)
                        .and_then(|label| label.parse::<Uuid>().ok())
                        .is_some_and(|id| mine.contains(&id))
                }) {
                    if let Err(error) = self.state.engine.remove(&summary.id, true).await {
                        failures.push(format!(
                            "container {} was not removed: {error}",
                            summary.name
                        ));
                    }
                }
            }
            Err(error) => failures.push(format!("the containers could not be listed: {error}")),
        }

        // The adapter does not remove networks — Mars never does — so this goes
        // through a client of the suite's own, as `tests/common/engine.rs`
        // does.
        let docker = super::engine::raw_docker();
        for network in [internal, egress] {
            if let Err(error) = docker.remove_network(&network).await {
                failures.push(format!("network {network} was not removed: {error}"));
            }
        }

        failures
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

    /// Invite `email` as `caller` and read the token back out of the email.
    ///
    /// The invite flow's arrangement step, in one call: the raw token exists
    /// only in the delivered message — the response never carries it
    /// (`SPEC.md`, "Users") — so this reads it out of the mock the way a person
    /// reads it out of their inbox, and clears the capture so the next message
    /// in the same test is unambiguous.
    ///
    /// Asserts 201. A test about a *refused* invitation posts to
    /// `/api/users/invites` itself.
    pub async fn invite(&self, caller: &AuthenticatedUser, email: &str, admin: bool) -> Invitation {
        let response = self
            .post_as(caller, INVITES)
            .json(&serde_json::json!({ "email": email, "admin": admin }))
            .await;

        response.assert_status(StatusCode::CREATED);
        let body = response.json::<Value>();

        assert_eq!(
            self.email.sent().len(),
            1,
            "exactly one invitation goes out"
        );
        let token = self.invite_link_token(0);
        self.email.clear();

        Invitation {
            id: body["id"]
                .as_str()
                .expect("the invitation carries an id")
                .parse()
                .expect("the id is a uuid"),
            token,
            body,
        }
    }

    /// The raw invitation token out of the `nth` captured message.
    ///
    /// Like [`TestApp::reset_link_token`]: the link is the only place the token
    /// exists, because the `Invite` response never carries it (`SPEC.md`,
    /// "Users").
    pub fn invite_link_token(&self, nth: usize) -> String {
        let sent = self.email.sent();
        let message = sent.get(nth).expect("a captured message");

        token_in_link(&message.text, "/invite/")
    }

    /// `GET /api/auth/invite/{token}`, unauthenticated.
    ///
    /// The interface that answers whether a link still resolves: 200 for an
    /// open invitation, the documented 400 for one that is unknown, expired,
    /// revoked, superseded or already accepted.
    pub async fn lookup_invite(&self, token: &str) -> TestResponse {
        self.server.get(&format!("/api/auth/invite/{token}")).await
    }

    /// `POST /api/auth/accept-invite`, unauthenticated.
    pub async fn accept_invite(&self, token: &str, username: &str, password: &str) -> TestResponse {
        self.server
            .post("/api/auth/accept-invite")
            .json(&serde_json::json!({
                "token": token,
                "username": username,
                "password": password,
            }))
            .await
    }

    /// Invite `email` as `caller` and accept the invitation, answering the
    /// signed-in user it created.
    ///
    /// The whole documented flow over HTTP, for the tests that need an invite
    /// in the *accepted* state rather than a test of the flow itself: the
    /// invitation row then names the user the acceptance created, exactly as it
    /// does in production.
    ///
    /// Asserts 201 on both halves.
    pub async fn invite_and_accept(
        &self,
        caller: &AuthenticatedUser,
        email: &str,
        username: &str,
        password: &str,
        admin: bool,
    ) -> (Invitation, AuthenticatedUser) {
        let invitation = self.invite(caller, email, admin).await;

        let response = self
            .accept_invite(&invitation.token, username, password)
            .await;
        response.assert_status(StatusCode::CREATED);

        let refresh_cookie = response.cookie(REFRESH_COOKIE).value().to_string();
        let pair = response.json::<TokenPair>();
        let user = self.read_back(&pair.user).await;

        (
            invitation,
            AuthenticatedUser {
                user,
                access_token: pair.access_token,
                refresh_cookie,
            },
        )
    }

    /// Ask for a password-reset link, asserting the documented 204.
    ///
    /// One response covers a known identifier, an unknown one, a rate-limited
    /// one and a failed delivery alike (`SPEC.md`, "Authentication"), so what
    /// actually happened is read afterwards from the captured messages with
    /// [`TestApp::reset_link_token`].
    pub async fn request_reset_link(&self, identifier: &str) {
        let response = self
            .server
            .post("/api/auth/request-password-reset")
            .json(&serde_json::json!({ "identifier": identifier }))
            .await;

        response.assert_status(StatusCode::NO_CONTENT);
        assert!(response.text().is_empty());
    }

    /// The raw reset token out of the `nth` captured message.
    ///
    /// The link is the only place it exists: the row carries the SHA-256 hex
    /// and the response body carries nothing at all, which is the point.
    pub fn reset_link_token(&self, nth: usize) -> String {
        let sent = self.email.sent();
        let message = sent.get(nth).expect("a captured message");

        assert_eq!(message.subject, "Reset your Mars password");
        token_in_link(&message.text, "/reset-password/")
    }

    /// `POST /api/auth/reset-password`, unauthenticated.
    pub async fn reset_password(&self, token: &str, password: &str) -> TestResponse {
        self.server
            .post("/api/auth/reset-password")
            .json(&serde_json::json!({ "token": token, "password": password }))
            .await
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

    /// Open the session WebSocket over the HTTP transport (`SPEC.md`,
    /// "WebSocket: session stream").
    ///
    /// `token` goes in the query string, where a browser has to put it, and
    /// `after` is the replay cursor. Asserts the upgrade succeeded, so it is
    /// the arrangement step for tests about what the stream *sends*; a test
    /// about a refused open requests the same path through
    /// [`TestApp::ws_request`] and asserts on the status.
    pub async fn ws(&self, session_id: Uuid, token: &str, after: i64) -> TestWebSocket {
        self.ws_request(session_id, Some(token), Some(after))
            .await
            .into_websocket()
            .await
    }

    /// The raw upgrade request the WebSocket is opened with, awaited but not
    /// upgraded: the response a refused open answers with.
    ///
    /// `None` leaves the parameter out of the query string altogether, which
    /// is how "no token" and "no cursor" are spelled.
    pub async fn ws_request(
        &self,
        session_id: Uuid,
        token: Option<&str>,
        after: Option<i64>,
    ) -> TestResponse {
        self.http_server
            .get_websocket(&ws_path(session_id, token, after.map(|a| a.to_string())))
            .await
    }

    /// [`TestApp::ws_request`] with `after` exactly as the client spelled it,
    /// for the cursors that are not numbers.
    pub async fn ws_request_raw_after(
        &self,
        session_id: Uuid,
        token: Option<&str>,
        after: &str,
    ) -> TestResponse {
        self.http_server
            .get_websocket(&ws_path(session_id, token, Some(after.to_string())))
            .await
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
        let user = self.read_back(&pair.user).await;

        AuthenticatedUser {
            user,
            access_token: pair.access_token,
            refresh_cookie,
        }
    }

    /// The `users` row behind a `User` DTO a route just answered.
    ///
    /// The DTO has no `Deserialize` — deriving one would make an empty
    /// `password_hash` constructible from JSON — so the row is read back
    /// instead, and every arrangement helper hands out the same `User`.
    async fn read_back(&self, dto: &Value) -> User {
        let id: Uuid = dto["id"]
            .as_str()
            .expect("the created user carries an id")
            .parse()
            .expect("the id is a uuid");

        UserRepository::new(&self.pool)
            .find(id)
            .await
            .expect("the lookup runs")
            .expect("the created user is there")
    }
}

/// Assert that `response` tells the browser to drop the refresh cookie.
///
/// The documented attribute string is asserted once, in
/// `tests/auth_credentials.rs`, against the cookie `auth::cookies` builds
/// (`SPEC.md`, "Authentication"). What a route test needs of it is the
/// behaviour: an empty value with an immediate expiry, so a browser holding a
/// token the database will never accept again stops sending it.
pub fn assert_refresh_cookie_cleared(response: &TestResponse) {
    let cookie = response
        .maybe_cookie(REFRESH_COOKIE)
        .expect("the response clears the refresh cookie");

    assert_eq!(cookie.value(), "");
    assert_eq!(cookie.max_age().expect("a max-age").whole_seconds(), 0);
}

/// The raw token out of the first `<PUBLIC_URL><path><token>` link in `text`.
///
/// Both credential links Mars emails have this shape, and in both of them the
/// token runs to the end of the word (`SPEC.md`, "Authentication").
fn token_in_link(text: &str, path: &str) -> String {
    let prefix = format!("{PUBLIC_URL}{path}");
    let start = text
        .find(&prefix)
        .unwrap_or_else(|| panic!("no {prefix} link in the message: {text}"))
        + prefix.len();

    text[start..]
        .split_whitespace()
        .next()
        .expect("the link has a token")
        .to_string()
}

/// An invitation as a test holds it: the created row's id, the raw token out
/// of the email, and the `Invite` DTO the route answered.
#[derive(Debug, Clone)]
pub struct Invitation {
    /// The `user_invites` row's id, which is what the by-id routes name.
    pub id: Uuid,
    /// The raw token from the emailed link; the row holds only its hash.
    pub token: String,
    /// The `Invite` DTO the creation answered (`SPEC.md`, "Users").
    pub body: Value,
}

/// `/ws/sessions/{id}` with the parameters that were given.
fn ws_path(session_id: Uuid, token: Option<&str>, after: Option<String>) -> String {
    let mut path = format!("/ws/sessions/{session_id}");
    let mut query: Vec<String> = Vec::new();

    if let Some(after) = after {
        query.push(format!("after={after}"));
    }
    if let Some(token) = token {
        query.push(format!("token={token}"));
    }
    if !query.is_empty() {
        path.push('?');
        path.push_str(&query.join("&"));
    }

    path
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

/// What [`TestApp::spawn_with_engine`] changes about the harness
/// configuration: the socket, the image and everything a container needs to
/// exist.
struct RealEngineSetup {
    /// `DOCKER_HOST`, as the process environment has it.
    docker_host: String,
    /// `SESSION_IMAGE_DEFAULT`, which is also the image the startup probe runs
    /// and the image a new project's default profile gets.
    image: String,
    /// `SESSION_NETWORK_INTERNAL`, this app's alone.
    network_internal: String,
    /// `SESSION_NETWORK_EGRESS`, this app's alone.
    network_egress: String,
    /// The name this engine gives the host gateway, used by both `MCP_URL` and
    /// `SESSION_EXTRA_HOSTS`.
    gateway_host: String,
}

/// The engine kind behind `docker_host`, which decides the gateway name.
///
/// A connection of its own, before [`bootstrap_engine`] opens the one the app
/// keeps: the kind comes from the engine's `/version`, and the name has to be
/// in the configuration `bootstrap_engine` is then handed.
async fn real_engine_kind(docker_host: &str) -> EngineKind {
    mars_orchestrator::engine::bollard::BollardEngine::connect(docker_host)
        .await
        .expect("the engine named by DOCKER_HOST answers")
        .kind()
}

/// The host gateway's name on this engine (`ARCHITECTURE.md`, "Development on
/// the host").
fn gateway_host(kind: EngineKind) -> &'static str {
    match kind {
        EngineKind::Podman => "host.containers.internal",
        EngineKind::Docker => "host.docker.internal",
    }
}

/// The configuration `spawn` builds, from values in code rather than from the
/// process environment: tests never depend on the developer's `.env` and never
/// mutate process-global state.
///
/// Everything here is obviously fake (rule 3). `SECRETS_MASTER_KEYS` is
/// rendered from `keyring` rather than pasted, so the string and the
/// [`SecretsKeyring`] injected beside it can never drift apart.
///
/// `real` is `None` for the mock engine and carries the overrides of
/// [`TestApp::spawn_with_engine`] otherwise; the values it replaces are the
/// only ones in this table that a container would ever resolve.
fn test_config(
    database_url: &str,
    data_dir: &Path,
    keyring: &SecretsKeyring,
    real: Option<&RealEngineSetup>,
) -> Config {
    let version = keyring.current_version();
    let key = keyring
        .key(version)
        .expect("the test keyring carries its current key");
    let data_dir = data_dir.display().to_string();

    let mut vars: HashMap<&str, String> = [
        ("PUBLIC_URL", PUBLIC_URL.to_string()),
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
        // One second, so a stop test can wait out the grace period between
        // `SIGINT` and `SIGTERM` without waiting out the production default of
        // 20 (`ARCHITECTURE.md`, "Stop semantics").
        ("STOP_GRACE_SECS", "1".to_string()),
        // No `RESEND_API_KEY`: mail goes to the mock, and setting a key would
        // only make `MAIL_FROM` required as well (ADR 0026).
    ]
    .into_iter()
    .collect();

    if let Some(real) = real {
        vars.insert("DOCKER_HOST", real.docker_host.clone());
        vars.insert("SESSION_IMAGE_DEFAULT", real.image.clone());
        vars.insert("SESSION_NETWORK_INTERNAL", real.network_internal.clone());
        vars.insert("SESSION_NETWORK_EGRESS", real.network_egress.clone());
        vars.insert(
            "SESSION_EXTRA_HOSTS",
            format!("{}:host-gateway", real.gateway_host),
        );
        // The default `MCP_PORT`, because nothing here binds one: what the file
        // in the container has to be is a URL that resolves, not one that
        // answers (the stub never calls MCP).
        vars.insert("MCP_URL", format!("http://{}:7001/mcp", real.gateway_host));
        // Long enough for a real container to act on its `SIGINT` before the
        // owner escalates to `SIGTERM`.
        vars.insert("STOP_GRACE_SECS", "5".to_string());
    }

    Config::from_vars(|name| vars.get(name).cloned()).expect("test config")
}
