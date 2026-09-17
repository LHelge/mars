//! `AppState`, the cloneable handle axum passes to every handler.
//!
//! `ARCHITECTURE.md`, "Orchestrator internals" fixes what lives here. State is
//! passed explicitly through `Router::with_state`; there is no global
//! `OnceLock<AppState>` and no other way to reach the pool or the
//! configuration.

use axum::extract::FromRef;

use crate::prelude::*;

/// The state cloned into every handler.
///
/// `ARCHITECTURE.md`, "Orchestrator internals": "`AppState` is cloned into
/// every handler and holds: `Arc<Config>`, the `PgPool`, `Arc<dyn
/// ContainerEngine>`, `Arc<dyn EmailClient>`, `Arc<dyn GitCredentialProvider>`,
/// the `SecretsKeyring`, the `SessionRegistry` (handles to running session
/// owner tasks), and the broadcast senders for event fan-out. Every `Arc<dyn
/// Trait>` has a mock behind the `integration-tests` feature."
///
/// Only the two fields this epic can populate exist yet. The remaining fields
/// are added in place, each by the epic that owns the trait it names:
///
/// | Field | Owning epic |
/// | --- | --- |
/// | `engine: Arc<dyn ContainerEngine>` | Container engine |
/// | `email: Arc<dyn EmailClient>` | Authentication |
/// | `git_credentials: Arc<dyn GitCredentialProvider>` | Git operations |
/// | `keyring: SecretsKeyring` | Secrets manager |
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
}

impl AppState {
    /// Build the state from the pieces the startup task owns.
    pub fn new(config: Arc<Config>, pool: PgPool) -> Self {
        Self { config, pool }
    }

    /// Whether the container engine is reachable; the health endpoint reports
    /// it.
    ///
    /// Container engine epic: replace with `self.engine.ping().await.is_ok()`.
    pub async fn engine_ready(&self) -> bool {
        true
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

    use sqlx::postgres::PgPoolOptions;

    use super::*;

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
        AppState::new(Arc::new(config), pool)
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
    async fn engine_ready_is_true_until_the_container_engine_epic_lands() {
        assert!(test_state().engine_ready().await);
    }
}
