//! The MCP transport skeleton: the router, the handshake and the 404
//! (`ARCHITECTURE.md`, "MCP design").
//!
//! A connection is made as a real session: the bearer middleware authenticates
//! every request on `/mcp` (`tests/mcp_auth.rs` is its own suite). What this
//! suite pins is the shape of the listener: `/mcp` speaks MCP, everything else
//! answers the ordinary error body — unauthenticated, because the 404 fallback
//! is outside the middleware — and the handshake advertises the tools
//! capability with no tools behind it until the dispatch task lands.
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use axum::Router;
use axum::routing::any;
use common::TestApp;
use common::mcp::McpClient;
use mars_orchestrator::models::SessionState;
use serde_json::Value;

/// An app with one live session, and that session's raw bearer token.
async fn app_with_session() -> (TestApp, String) {
    let app = TestApp::spawn().await;
    let (project_id, profile_id) = app.seed_mcp_project().await;
    let seeded = app
        .seed_mcp_session(project_id, profile_id, SessionState::Running)
        .await;

    (app, seeded.token)
}

#[tokio::test]
async fn a_client_connects_and_the_server_lists_no_tools_yet() {
    let (app, token) = app_with_session().await;

    let client = McpClient::connect(&app, &token)
        .await
        .expect("a running session's token authenticates");

    assert!(
        client.list_tools().await.is_empty(),
        "no tool is registered until the dispatch task lands"
    );
}

#[tokio::test]
async fn every_other_path_is_the_ordinary_404() {
    let (app, token) = app_with_session().await;
    let client = McpClient::connect(&app, &token)
        .await
        .expect("the handshake succeeds");

    // The API's own health endpoint, asked of the MCP listener: this router
    // carries no `/api` nesting at all.
    let response = reqwest::get(format!("http://{}/health", client.addr()))
        .await
        .expect("the mcp listener answers");

    assert_eq!(response.status().as_u16(), 404);
    assert_eq!(
        response.json::<Value>().await.expect("a JSON body"),
        serde_json::json!({ "status": 404, "error": "not found" })
    );
}

/// The status of a refused `initialize` has to reach the caller, because the
/// bearer middleware's own suite asserts 401 and 403 on exactly this path. The
/// refusal is arranged with a stub router rather than the real middleware, so
/// this stays a test of the client helper's error reporting and of nothing
/// else.
#[tokio::test]
async fn a_refused_initialize_carries_its_http_status() {
    let refusing = Router::new().route(
        "/mcp",
        any(|| async { (axum::http::StatusCode::UNAUTHORIZED, "no") }),
    );

    let error = McpClient::connect_to_router(refusing, "not-a-real-session-token")
        .await
        .err()
        .expect("a 401 is not a successful handshake");

    assert_eq!(
        error.status().map(|status| status.as_u16()),
        Some(401),
        "{error:?}"
    );
}
