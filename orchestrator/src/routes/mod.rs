//! Axum routers, one module per resource, nested under `/api`. Each module
//! exports `routes() -> Router<AppState>` and keeps its DTOs private.
//!
//! This module only merges them; the `/api` prefix is applied once in
//! `build_api_router`, so a module's own paths are written without it.

use axum::Router;

use crate::prelude::*;

pub mod auth;
pub mod extractors;
pub mod git;
pub mod health;
pub mod profiles;
pub mod projects;
pub mod secrets;
pub mod sessions;
pub mod shared_dirs;
pub mod stream_auth;
pub mod task_states;
pub mod tasks;
pub mod throttle;
pub mod users;

/// The fixture route the integration and end-to-end tests create their users
/// with (`SPEC.md`, "Test-only routes"). Never compiled into a release build;
/// the test below is the assertion that the gate holds.
#[cfg(feature = "integration-tests")]
pub mod test;

pub use extractors::{AdminUser, CurrentUser, UngatedUser, authenticate_access_token};
pub use stream_auth::{
    AUTH_REQUIRED, StreamAuthFailure, StreamPrincipal, StreamToken, authenticate_stream,
    reauthorize,
};

/// Transitional: [`Path`](crate::prelude::Path) and
/// [`Query`](crate::prelude::Query) now live in the prelude beside `Json`, and
/// a converted route module gets them from `use crate::prelude::*;`. This
/// re-export only keeps the modules that still spell `use crate::routes::{…,
/// Path, Query}` compiling until they are converted too, and goes away with
/// the last of them.
pub use crate::prelude::{Path, Query};

/// Every resource router, merged into the one router nested under `/api`.
pub fn routes() -> Router<AppState> {
    let router = Router::new()
        .merge(health::routes())
        .nest("/auth", auth::routes())
        // One `nest` per prefix — axum panics on two at the same path — so the
        // git, profile, shared-directory and project-scoped session routes,
        // which carry their own `{pid}/…` paths, are merged into the projects
        // router rather than nested beside it.
        .nest(
            "/projects",
            projects::routes()
                .merge(git::routes())
                .merge(profiles::routes())
                .merge(sessions::project_routes())
                .merge(shared_dirs::routes())
                .merge(task_states::routes())
                .merge(tasks::project_routes()),
        )
        // `/projects/{pid}/tasks/stream`, spelled in full rather than nested:
        // the `/projects` prefix above already has its one `nest`, and the SSE
        // handler lives in `sse/` beside `ws/` rather than among the resource
        // modules (`ARCHITECTURE.md`, "Orchestrator internals").
        .merge(crate::sse::routes())
        .nest("/secrets", secrets::routes())
        .nest("/sessions", sessions::routes())
        .nest("/tasks", tasks::routes())
        .nest("/users", users::routes());

    // The only routes with a prefix of their own, because `SPEC.md`,
    // "Test-only routes" gives them one, and because a single `nest` is what
    // keeps them all behind the one feature gate.
    #[cfg(feature = "integration-tests")]
    let router = router.nest("/test", test::routes());

    router
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use axum::http::StatusCode;
    use axum_test::TestServer;
    use sqlx::postgres::PgPoolOptions;

    use super::*;
    use crate::build_api_router;
    use crate::email::LogEmailClient;
    use crate::engine::PlaceholderEngine;
    use crate::git::{CommitIdentity, PatCredentialProvider};
    use crate::secrets::{MASTER_KEY_LEN, SecretsKeyring};

    /// A router over a state nothing in this test ever reaches: the assertion
    /// below is about routing, which axum decides before it calls a handler,
    /// so the pool is lazy and never connects and the collaborators are the
    /// placeholders — the mocks live behind `integration-tests` and this test
    /// has to compile in both configurations.
    ///
    /// Every value is obviously fake (`CLAUDE.md`, rule 3).
    fn test_server() -> TestServer {
        let vars: HashMap<&str, &str> = [
            ("PUBLIC_URL", "https://mars.example.invalid"),
            ("JWT_SECRET", "not-a-real-signing-secret"),
            ("DATABASE_URL", "postgres://mars:fake@localhost:5432/mars"),
            ("DOCKER_HOST", "unix:///nonexistent/mars-test/podman.sock"),
            ("DATA_DIR_HOST", "/srv/mars/data"),
            ("SECRETS_MASTER_KEYS", "1=not-a-real-key"),
            ("GIT_BOT_NAME", "Mars Bot"),
            ("GIT_BOT_EMAIL", "mars-bot@example.invalid"),
            ("SESSION_IMAGE_DEFAULT", "mars-session-claude:dev"),
        ]
        .into_iter()
        .collect();

        let config = Config::from_vars(|name| vars.get(name).map(|value| value.to_string()))
            .expect("a complete required set loads");
        let pool = PgPoolOptions::new()
            .connect_lazy(&config.database_url)
            .expect("a lazy pool never connects");
        let identity = CommitIdentity {
            name: config.git_bot_name.clone(),
            email: config.git_bot_email.clone(),
        };

        let keyring = SecretsKeyring::from_entries(vec![(1, [0u8; MASTER_KEY_LEN])])
            .expect("one entry is a valid keyring");

        let state = AppState::new(
            Arc::new(config),
            pool.clone(),
            Arc::new(PlaceholderEngine),
            Arc::new(LogEmailClient),
            Arc::new(PatCredentialProvider::new(pool, keyring.clone(), identity)),
            keyring,
        );

        TestServer::new(build_api_router(state))
    }

    /// `SPEC.md`, "Test-only routes": "Compiled only with the
    /// `integration-tests` cargo feature, never into a release build."
    ///
    /// The same test in both configurations, because a gate is only worth
    /// anything if something asserts both sides of it. Without the feature
    /// `/api/test/users` is not a path this router knows, so the request falls
    /// through to 404; with it the path exists and accepts only `POST`, so a
    /// `GET` is 405 — the router answering, not a handler.
    #[tokio::test]
    async fn the_test_routes_exist_only_behind_the_integration_tests_feature() {
        let status = test_server().get("/api/test/users").await.status_code();

        if cfg!(feature = "integration-tests") {
            assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
        } else {
            assert_eq!(status, StatusCode::NOT_FOUND);
        }
    }
}
