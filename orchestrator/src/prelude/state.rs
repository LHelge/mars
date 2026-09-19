//! `AppState`, the cloneable handle axum passes to every handler.
//!
//! `ARCHITECTURE.md`, "Orchestrator internals" fixes what lives here. State is
//! passed explicitly through `Router::with_state`; there is no global
//! `OnceLock<AppState>` and no other way to reach the pool or the
//! configuration.

use axum::extract::FromRef;
use futures_util::future::BoxFuture;
use uuid::Uuid;

use crate::email::EmailClient;
use crate::engine::ContainerEngine;
use crate::events::EventFanout;
use crate::git::{GitCredentialProvider, ProjectGitLocks};
use crate::prelude::*;
use crate::routes::throttle::{LoginThrottle, ResetRateLimit};
use crate::secrets::SecretsKeyring;
use crate::session::SessionRegistry;

/// What runs when a session reaches `done` or `failed`.
///
/// One hook rather than a dependency: the session owner, the launcher and the
/// session service all end sessions, and what has to happen when one ends —
/// releasing the tasks it held (`ARCHITECTURE.md`, "Task tracker", leases) —
/// belongs to the tracker, which is a layer the session code must not reach
/// into. The tracker installs itself here at startup with
/// [`AppState::with_session_ended_hook`]; everywhere else the field is `None`
/// and the ending paths do nothing extra.
pub type SessionEndedHook = Arc<dyn Fn(Uuid) -> BoxFuture<'static, ()> + Send + Sync>;

/// The state cloned into every handler.
///
/// `ARCHITECTURE.md`, "Orchestrator internals": "`AppState` is cloned into
/// every handler and holds: `Arc<Config>`, the `PgPool`, `Arc<dyn
/// ContainerEngine>`, `Arc<dyn EmailClient>`, `Arc<dyn GitCredentialProvider>`,
/// the `SecretsKeyring`, the in-memory login throttle and password-reset rate
/// limiter, the `ProjectGitLocks` table the per-project git lock is taken from,
/// the `SessionRegistry` (handles to running session owner tasks), and the
/// broadcast senders for event fan-out. Every `Arc<dyn Trait>` has a mock
/// behind the `integration-tests` feature."
///
/// The three collaborator traits are here as trait objects so the whole API
/// can be tested without an engine, a mail provider or GitHub; each has a mock
/// behind the `integration-tests` feature.
///
/// Every field is cheap to clone: an `Arc`, a `PgPool` handle or a broadcast
/// sender. Cloning the state never clones the data behind it.
#[derive(Clone)]
pub struct AppState {
    /// The process configuration, shared by every handler and background task.
    pub config: Arc<Config>,
    /// The Postgres pool; repositories borrow it.
    pub pool: PgPool,
    /// The container engine; `GET /api/health` pings it and the session
    /// lifecycle drives containers through it.
    pub engine: Arc<dyn ContainerEngine>,
    /// Outgoing mail: invitations and password resets.
    pub email: Arc<dyn EmailClient>,
    /// Credentials and the bot identity for upstream git operations.
    pub git_credentials: Arc<dyn GitCredentialProvider>,
    /// The master keys envelope encryption wraps data keys under. Cloning it
    /// shares the keys rather than copying them.
    pub keyring: SecretsKeyring,
    /// Failed-login counters and blocks, in process memory: v1 runs a single
    /// orchestrator instance (`SPEC.md`, "Non-goals for v1").
    pub login_throttle: Arc<LoginThrottle>,
    /// Password-reset request counters, in process memory for the same reason.
    pub reset_rate_limit: Arc<ResetRateLimit>,
    /// The per-project git locks that serialise every orchestrator mutation of
    /// a project repository (`ARCHITECTURE.md`, "Git model", Serialization).
    /// The REST handlers, the MCP tools, the session launcher, the cron jobs
    /// and the deletion paths share this one table, which is the whole point
    /// of it living here.
    pub git_locks: Arc<ProjectGitLocks>,
    /// The handles to the running session owner tasks (`ARCHITECTURE.md`,
    /// "Session owner task"). The REST input endpoint, the WebSocket handler,
    /// the launcher and the cron reapers reach a session's owner only through
    /// this registry, which is what makes the CLI's stdin single-writer. It is
    /// `Clone` with its map behind an `Arc` of its own, so it needs no second
    /// `Arc` here.
    pub session_registry: SessionRegistry,
    /// The broadcast senders for event fan-out (`ARCHITECTURE.md`, "Event
    /// delivery"): the shared Postgres listener publishes a [`Notice`] here
    /// and every WebSocket and SSE subscriber of that session or project wakes
    /// up and reads from its own cursor. It is `Clone` with its maps behind an
    /// `Arc` of its own, so it needs no second `Arc` here.
    ///
    /// [`Notice`]: crate::events::Notice
    pub fanout: EventFanout,
    /// What to run when a session reaches `done` or `failed`, or `None` when
    /// nothing is installed. See [`SessionEndedHook`].
    pub on_session_ended: Option<SessionEndedHook>,
}

impl AppState {
    /// Build the state from the pieces the startup task owns.
    ///
    /// The two limiters are not parameters: they hold no configuration, have
    /// no collaborator to mock and start empty, so every caller — `main`, the
    /// unit tests below and `TestApp` — wants exactly the same pair on the
    /// system clock. A test that has to forget what they counted calls
    /// `reset()` on the field; a unit test of the limiters themselves builds
    /// its own with an injected clock. The git lock table is built here for
    /// the same reason, and a test that wants to take a git lock takes it
    /// through `state.git_locks`, the very table the handlers wait on. The
    /// session registry and the event fan-out start empty for the same reason
    /// again: a test that registers an owner registers it in the registry the
    /// handlers forward through, and a test that subscribes to a session's
    /// notices subscribes to the channel the listener publishes on.
    pub fn new(
        config: Arc<Config>,
        pool: PgPool,
        engine: Arc<dyn ContainerEngine>,
        email: Arc<dyn EmailClient>,
        git_credentials: Arc<dyn GitCredentialProvider>,
        keyring: SecretsKeyring,
    ) -> Self {
        Self {
            config,
            pool,
            engine,
            email,
            git_credentials,
            keyring,
            login_throttle: Arc::new(LoginThrottle::new()),
            reset_rate_limit: Arc::new(ResetRateLimit::new()),
            git_locks: Arc::new(ProjectGitLocks::new()),
            session_registry: SessionRegistry::new(),
            fanout: EventFanout::new(),
            on_session_ended: None,
        }
    }

    /// The same state with `hook` run whenever a session ends.
    ///
    /// Installed once at startup, before the router is built, so every clone of
    /// the state carries it.
    #[must_use]
    pub fn with_session_ended_hook(mut self, hook: SessionEndedHook) -> Self {
        self.on_session_ended = Some(hook);
        self
    }

    /// The session launcher over this state (`ARCHITECTURE.md`, "Launch
    /// sequence").
    ///
    /// A constructor rather than a field, for the reason
    /// [`GitService::from_state`](crate::git::GitService::from_state) is one:
    /// the launcher needs every collaborator the state already holds, so a
    /// field pointing back at a type that holds the state would be a reference
    /// cycle. One call, so the routes, the session service and recovery all
    /// launch through the same implementation.
    pub fn launcher(&self) -> crate::session::Launcher {
        crate::session::Launcher::from_state(self)
    }

    /// Run the end-of-session hook, if one is installed.
    ///
    /// Here rather than at each call site so that "no hook" is one branch in
    /// one place; the ending paths call it unconditionally.
    pub async fn session_ended(&self, session_id: Uuid) {
        if let Some(hook) = &self.on_session_ended {
            hook(session_id).await;
        }
    }
}

impl FromRef<AppState> for Arc<Config> {
    fn from_ref(state: &AppState) -> Self {
        Arc::clone(&state.config)
    }
}

impl FromRef<AppState> for PgPool {
    fn from_ref(state: &AppState) -> Self {
        state.pool.clone()
    }
}

impl FromRef<AppState> for EventFanout {
    fn from_ref(state: &AppState) -> Self {
        state.fanout.clone()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::net::{IpAddr, Ipv4Addr};

    use sqlx::postgres::PgPoolOptions;
    use uuid::Uuid;

    use super::*;
    use crate::email::LogEmailClient;
    use crate::engine::PlaceholderEngine;
    use crate::git::{CommitIdentity, PatCredentialProvider};
    use crate::secrets::MASTER_KEY_LEN;

    /// Obviously fake values; nothing here is a real credential (rule 3).
    fn test_config() -> Config {
        let vars: HashMap<&str, &str> = [
            ("PUBLIC_URL", "https://mars.example.invalid"),
            ("JWT_SECRET", "not-a-real-signing-secret"),
            ("DATABASE_URL", "postgres://mars:fake@localhost:5432/mars"),
            ("DOCKER_HOST", "unix:///run/user/1000/podman/podman.sock"),
            ("DATA_DIR_HOST", "/srv/mars/data"),
            ("SECRETS_MASTER_KEYS", "1=not-a-real-key"),
            ("GIT_BOT_NAME", "Mars Bot"),
            ("GIT_BOT_EMAIL", "mars-bot@example.invalid"),
            ("SESSION_IMAGE_DEFAULT", "mars-session-claude:dev"),
        ]
        .into_iter()
        .collect();

        Config::from_vars(|name| vars.get(name).map(|value| value.to_string()))
            .expect("a complete required set loads")
    }

    /// Lazy: the pool is never connected, so the unit tests need no Postgres.
    /// It still wants a tokio context for its idle reaper, hence `#[tokio::test]`.
    fn test_state() -> AppState {
        let config = test_config();
        let pool = PgPoolOptions::new()
            .connect_lazy(&config.database_url)
            .expect("a lazy pool never connects");
        let identity = CommitIdentity {
            name: config.git_bot_name.clone(),
            email: config.git_bot_email.clone(),
        };

        let keyring = SecretsKeyring::from_entries(vec![(1, [0u8; MASTER_KEY_LEN])])
            .expect("one entry is a valid keyring");

        // The startup clients, not the mocks: these unit tests also
        // compile without the `integration-tests` feature.
        AppState::new(
            Arc::new(config),
            pool.clone(),
            Arc::new(PlaceholderEngine),
            Arc::new(LogEmailClient),
            Arc::new(PatCredentialProvider::new(pool, keyring.clone(), identity)),
            keyring,
        )
    }

    fn assert_send_sync<T: Send + Sync + 'static>() {}

    #[tokio::test]
    async fn app_state_is_clone_send_sync_and_static() {
        assert_send_sync::<AppState>();

        let state = test_state();
        let clone = state.clone();

        assert!(
            Arc::ptr_eq(&state.config, &clone.config),
            "cloning the state must share the configuration, not copy it"
        );
    }

    #[tokio::test]
    async fn from_ref_hands_out_the_config_and_the_pool() {
        let state = test_state();

        let config = Arc::<Config>::from_ref(&state);
        assert!(Arc::ptr_eq(&config, &state.config));

        let pool = PgPool::from_ref(&state);
        assert_eq!(pool.size(), state.pool.size());
    }

    #[tokio::test]
    async fn the_collaborators_are_shared_by_a_clone_not_rebuilt() {
        let state = test_state();
        let clone = state.clone();

        assert!(Arc::ptr_eq(&state.engine, &clone.engine));
        assert!(Arc::ptr_eq(&state.email, &clone.email));
        assert!(Arc::ptr_eq(&state.git_credentials, &clone.git_credentials));
        assert_eq!(clone.keyring.current_version(), 1);
    }

    #[tokio::test]
    async fn the_limiters_are_shared_by_a_clone_and_start_empty() {
        let state = test_state();
        let clone = state.clone();

        assert!(Arc::ptr_eq(&state.login_throttle, &clone.login_throttle));
        assert!(Arc::ptr_eq(
            &state.reset_rate_limit,
            &clone.reset_rate_limit
        ));

        assert_eq!(state.login_throttle.tracked_keys(), 0);
        assert_eq!(state.reset_rate_limit.tracked_keys(), 0);

        // A handler holding the clone counts against the same maps the rest of
        // the process reads.
        clone
            .login_throttle
            .record_failure("bob", IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert!(clone.reset_rate_limit.allow("bob@example.invalid"));

        assert_eq!(state.login_throttle.tracked_keys(), 2);
        assert_eq!(state.reset_rate_limit.tracked_keys(), 1);

        state.login_throttle.reset();
        state.reset_rate_limit.reset();
        assert_eq!(clone.login_throttle.tracked_keys(), 0);
        assert_eq!(clone.reset_rate_limit.tracked_keys(), 0);
    }

    #[tokio::test]
    async fn the_session_registry_is_shared_by_a_clone_and_starts_empty() {
        use crate::models::SessionKind;
        use crate::session::Phase;

        let state = test_state();
        let clone = state.clone();

        assert_eq!(state.session_registry.tracked_sessions(), 0);

        // A launcher holding the clone registers into the same map the
        // WebSocket handler holding the original forwards through: that
        // sharing is what makes stdin single-writer.
        let session = Uuid::new_v4();
        let _rx =
            clone
                .session_registry
                .register(session, SessionKind::Conversational, Phase::Creating);

        assert_eq!(state.session_registry.tracked_sessions(), 1);
        assert!(state.session_registry.is_live(session));
    }

    #[tokio::test]
    async fn the_fanout_is_shared_by_a_clone_and_starts_empty() {
        use crate::events::Notice;

        let state = test_state();
        let clone = state.clone();

        let session = Uuid::new_v4();
        assert_eq!(state.fanout.session_subscribers(session), 0);

        // A stream handler holding the clone subscribes to the very channel
        // the shared listener, holding the original, publishes on.
        let mut receiver = clone.fanout.subscribe_session(session);
        assert_eq!(state.fanout.session_subscribers(session), 1);

        state
            .fanout
            .publish_session(session, Notice::SessionEvents { seq: 1 });
        assert_eq!(receiver.try_recv(), Ok(Notice::SessionEvents { seq: 1 }));

        // And the extractor hands out the same fan-out, not a new one.
        let extracted = EventFanout::from_ref(&state);
        extracted.publish_session(session, Notice::Resync);
        assert_eq!(receiver.try_recv(), Ok(Notice::Resync));
    }

    #[tokio::test]
    async fn the_git_locks_are_shared_by_a_clone_and_start_empty() {
        let state = test_state();
        let clone = state.clone();

        assert!(Arc::ptr_eq(&state.git_locks, &clone.git_locks));
        assert_eq!(state.git_locks.tracked_projects(), 0);

        // A handler holding the clone waits on the same table as a cron job
        // holding the original: that sharing is what serialises them.
        let project = Uuid::new_v4();
        let guard = clone.git_locks.lock(project).await;

        assert_eq!(guard.project_id(), project);
        assert_eq!(state.git_locks.tracked_projects(), 1);

        drop(guard);
        state.git_locks.forget(project);
        assert_eq!(clone.git_locks.tracked_projects(), 0);
    }
}
