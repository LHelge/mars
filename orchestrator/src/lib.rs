//! Mars orchestrator library crate.
//!
//! The module tree mirrors `ARCHITECTURE.md`, "Orchestrator internals"; every
//! module is a directory module and does `use crate::prelude::*;`.
//!
//! The binary is a thin shell: it reads the configuration, opens the pool,
//! runs the migrations and binds the two listeners, then hands them to
//! [`run`]. Everything a test would want to drive lives here, so `main` and
//! the integration tests serve the same router over the same code path.

pub mod prelude;

pub mod agent;
pub mod cron;
pub mod email;
pub mod engine;
pub mod events;
pub mod git;
pub mod mcp;
pub mod models;
pub mod repositories;
pub mod routes;
pub mod secrets;
pub mod session;
pub mod sse;
pub mod ws;

use std::future::Future;
use std::net::SocketAddr;

use axum::Router;
use axum::extract::Request;
use futures_util::FutureExt;
use tokio::net::TcpListener;
use tower_http::trace::TraceLayer;

use crate::prelude::*;

/// Build the public API router: every resource module nested under `/api`,
/// with the state applied and request tracing attached.
///
/// No `CorsLayer`: the frontend is served same-origin behind nginx
/// (`ARCHITECTURE.md`, "Components"), so cross-origin headers would only widen
/// what the API accepts.
pub fn build_api_router(state: AppState) -> Router {
    Router::new()
        .nest("/api", routes::routes())
        .with_state(state)
        .layer(
            TraceLayer::new_for_http().make_span_with(|request: &Request| {
                // Method and path only. The query string is deliberately left out:
                // the WebSocket and SSE endpoints carry their access token in
                // `?token=` (`SPEC.md`, "WebSocket: session stream"), and rule 3
                // keeps tokens out of the orchestrator's own logs.
                tracing::info_span!(
                    "http",
                    method = %request.method(),
                    path = %request.uri().path(),
                )
            }),
        )
}

/// Serve the API and the MCP listener until `shutdown` resolves.
///
/// Both listeners are handed in already bound so that callers — `main` on the
/// configured ports, tests on port 0 — choose the addresses and can read them
/// back before anything is served. The single `shutdown` future drives both
/// graceful shutdowns; `run` returns once both have stopped accepting and
/// their in-flight requests have finished.
pub async fn run(
    state: AppState,
    api: TcpListener,
    mcp: TcpListener,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<()> {
    let shutdown = shutdown.shared();
    let api_shutdown = shutdown.clone();

    // `into_make_service_with_connect_info` so handlers can extract
    // `ConnectInfo<SocketAddr>`: the login throttle keys on the peer address
    // when nginx has not set `X-Forwarded-For`
    // (`routes::throttle::client_addr`).
    let api_server = axum::serve(
        api,
        build_api_router(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(api_shutdown);
    let mcp_server = axum::serve(mcp, mcp::placeholder_router()).with_graceful_shutdown(shutdown);

    let (api_result, mcp_result) = tokio::join!(api_server, mcp_server);

    if let Err(err) = api_result {
        error!(listener = "api", error = %err, "listener stopped with an error");
        return Err(Error::Internal("the api listener failed".to_string()));
    }
    if let Err(err) = mcp_result {
        error!(listener = "mcp", error = %err, "listener stopped with an error");
        return Err(Error::Internal("the mcp listener failed".to_string()));
    }

    info!("listeners stopped");
    Ok(())
}
