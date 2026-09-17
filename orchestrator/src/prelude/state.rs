//! `AppState`, the cloneable handle axum passes to every handler.
//!
//! `ARCHITECTURE.md`, "Orchestrator internals" fixes what lives here. State is
//! passed explicitly through `Router::with_state`; there is no global
//! `OnceLock<AppState>` and no other way to reach the pool or the
//! configuration.

use axum::extract::FromRef;

use crate::email::EmailClient;
use crate::engine::ContainerEngine;
use crate::git::GitCredentialProvider;
use crate::prelude::*;
use crate::routes::throttle::{LoginThrottle, ResetRateLimit};
use crate::secrets::SecretsKeyring;

/// The state cloned into every handler.
///
/// `ARCHITECTURE.md`, "Orchestrator internals": "`AppState` is cloned into
/// every handler and holds: `Arc<Config>`, the `PgPool`, `Arc<dyn
/// ContainerEngine>`, `Arc<dyn EmailClient>`, `Arc<dyn GitCredentialProvider>`,
/// the `SecretsKeyring`, the in-memory login throttle and password-reset rate
/// limiter, the `SessionRegistry` (handles to running session owner tasks), and
/// the broadcast senders for event fan-out. Every `Arc<dyn Trait>` has a mock
/// behind the `integration-tests` feature."
///
/// The three collaborator traits are here as trait objects so the whole API
/// can be tested without an engine, a mail provider or GitHub; each has a mock
/// behind the `integration-tests` feature. The fields still missing are added
/// in place by the epic that owns them:
///
/// | Field | Owning epic |
/// | --- | --- |
/// | `registry: SessionRegistry` | Session lifecycle |
/// | the broadcast senders for event fan-out | Real-time delivery |
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
}

impl AppState {
    /// Build the state from the pieces the startup task owns.
    ///
    /// The two limiters are not parameters: they hold no configuration, have
    /// no collaborator to mock and start empty, so every caller — `main`, the
    /// unit tests below and `TestApp` — wants exactly the same pair on the
    /// system clock. A test that has to forget what they counted calls
    /// `reset()` on the field; a unit test of the limiters themselves builds
    /// its own with an injected clock.
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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::net::{IpAddr, Ipv4Addr};

    use sqlx::postgres::PgPoolOptions;

    use super::*;
    use crate::email::LogEmailClient;
    use crate::engine::PlaceholderEngine;
    use crate::git::{CommitIdentity, PlaceholderCredentialProvider};
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

        // The startup clients, not the mocks: these unit tests also
        // compile without the `integration-tests` feature.
        AppState::new(
            Arc::new(config),
            pool,
            Arc::new(PlaceholderEngine),
            Arc::new(LogEmailClient),
            Arc::new(PlaceholderCredentialProvider::new(identity)),
            SecretsKeyring::from_entries(vec![(1, [0u8; MASTER_KEY_LEN])])
                .expect("one entry is a valid keyring"),
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
}
