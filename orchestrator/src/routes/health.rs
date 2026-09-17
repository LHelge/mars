//! `GET /api/health` (`SPEC.md`, "Health").
//!
//! Unauthenticated, because the compose health check and nginx call it before
//! anyone has a token. It answers 200 when both dependencies are reachable and
//! 503 when either is not, always with the same three-field body, so a checker
//! can read *which* dependency is down from a failing response. It never
//! answers 500: a probe failure is information, not an internal error.

use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use serde::Serialize;

use crate::prelude::*;

/// How long the database probe may take before it counts as a failure. Short,
/// because a health check that blocks is itself a fault: an exhausted pool or
/// a hung server must answer 503 quickly rather than hold the checker open.
const DATABASE_PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// The documented body (`SPEC.md`, "Health").
#[derive(Debug, Serialize)]
struct HealthResponse {
    /// Always true: this process answered.
    orchestrator: bool,
    /// Whether a trivial query completed within [`DATABASE_PROBE_TIMEOUT`].
    database: bool,
    /// Whether the container engine is reachable.
    engine: bool,
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/health", get(health))
}

/// 200 when both dependencies are up, 503 otherwise; the body is the same
/// shape either way.
async fn health(State(state): State<AppState>) -> impl IntoResponse {
    let database = database_ready(&state.pool).await;
    let engine = state.engine_ready().await;

    let status = if database && engine {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    (
        status,
        Json(HealthResponse {
            orchestrator: true,
            database,
            engine,
        }),
    )
}

/// Acquire a connection and run the cheapest possible query, under a timeout
/// so an exhausted pool reports `database: false` instead of hanging.
async fn database_ready(pool: &PgPool) -> bool {
    match tokio::time::timeout(
        DATABASE_PROBE_TIMEOUT,
        sqlx::query("SELECT 1").execute(pool),
    )
    .await
    {
        Ok(Ok(_)) => true,
        Ok(Err(err)) => {
            warn!(error = %err, "health probe: database query failed");
            false
        }
        Err(_) => {
            warn!(
                timeout_secs = DATABASE_PROBE_TIMEOUT.as_secs(),
                "health probe: database query timed out"
            );
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use axum_test::TestServer;
    use serde_json::json;
    use sqlx::postgres::PgPoolOptions;

    use super::*;

    /// Obviously fake values; nothing here is a real credential (rule 3).
    fn test_config() -> Config {
        let vars: HashMap<&str, &str> = [
            ("PUBLIC_URL", "https://mars.example.invalid"),
            ("JWT_SECRET", "not-a-real-signing-secret"),
            // Nothing listens on port 1. The pool retries a refused
            // connection until its own acquire timeout, so what this asserts
            // is the handler's `DATABASE_PROBE_TIMEOUT` firing first — the
            // exhausted-pool case, reached without an exhausted pool.
            ("DATABASE_URL", "postgres://invalid:invalid@127.0.0.1:1/x"),
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

    /// The router with a pool that can never connect: no Postgres needed.
    fn server() -> TestServer {
        let config = test_config();
        let pool = PgPoolOptions::new()
            .connect_lazy(&config.database_url)
            .expect("a lazy pool never connects");
        let state = AppState::new(Arc::new(config), pool);

        TestServer::new(crate::build_api_router(state))
    }

    #[tokio::test]
    async fn an_unreachable_database_answers_503_with_the_documented_body() {
        let response = server().get("/api/health").await;

        response.assert_status(StatusCode::SERVICE_UNAVAILABLE);
        response.assert_json(&json!({
            "orchestrator": true,
            "database": false,
            "engine": true,
        }));
    }
}
