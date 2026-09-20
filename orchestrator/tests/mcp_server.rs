//! The MCP transport skeleton: the router, the handshake and the 404
//! (`ARCHITECTURE.md`, "MCP design").
//!
//! No authentication yet — the bearer middleware is its own task — so a
//! connection here succeeds whatever token it carries. What the suite pins is
//! the shape of the listener: `/mcp` speaks MCP, everything else answers the
//! ordinary error body, and the handshake advertises the tools capability with
//! no tools behind it until the dispatch task lands.
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use axum::Router;
use axum::routing::any;
use common::TestApp;
use common::mcp::McpClient;
use serde_json::Value;

#[tokio::test]
async fn a_client_connects_and_the_server_lists_no_tools_yet() {
    let app = TestApp::spawn().await;

    let client = McpClient::connect(&app, "not-a-real-session-token")
        .await
        .expect("the handshake succeeds while nothing authenticates it");

    assert!(
        client.list_tools().await.is_empty(),
        "no tool is registered until the dispatch task lands"
    );
}

#[tokio::test]
async fn every_other_path_is_the_ordinary_404() {
    let app = TestApp::spawn().await;
    let client = McpClient::connect(&app, "not-a-real-session-token")
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
/// bearer middleware's own suite asserts 401 and 403 on exactly this path.
/// Nothing refuses anything yet, so the refusal is arranged with a stub router
/// in the real one's place.
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
