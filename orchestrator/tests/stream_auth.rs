//! The authentication the WebSocket and SSE streams share (`SPEC.md`,
//! "Authentication"; ADR 0025).
//!
//! Two halves, because the contract has two moments. The *open* half is
//! driven through `GET /api/test/stream-whoami` (`SPEC.md`, "Test-only
//! routes"), a plain request that takes `StreamToken` and calls
//! `authenticate_stream`, so every assertion goes through the real router and
//! the real `?token=` extraction instead of a hand-built `Parts`. What it
//! asserts is that a token gets the same answer there as it would on any
//! header-authenticated route.
//!
//! The *continuation* half calls `reauthorize` directly against
//! `TestApp.pool`, because there is nothing to drive through a router: it is a
//! single unlocked read of the `users` row that the stream handlers run before
//! every incoming message and at every heartbeat tick. The last test is the
//! one this module exists for — a database that does not answer must not
//! authorize.
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use axum::http::StatusCode;
use chrono::Utc;
use common::TestApp;
use mars_orchestrator::prelude::*;
use mars_orchestrator::routes::{StreamAuthFailure, StreamPrincipal, reauthorize};
use serde_json::{Value, json};
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

/// The probe route (`SPEC.md`, "Test-only routes").
const WHOAMI: &str = "/api/test/stream-whoami";

/// The documented 401 body (`SPEC.md`, "Authentication").
fn unauthorized() -> Value {
    json!({ "status": 401, "error": "authentication required" })
}

// ---------------------------------------------------------------- opening

#[tokio::test]
async fn a_request_with_no_token_at_all_is_401() {
    let app = TestApp::spawn().await;

    let response = app.server.get(WHOAMI).await;

    response.assert_status(StatusCode::UNAUTHORIZED);
    response.assert_json(&unauthorized());
}

#[tokio::test]
async fn an_empty_token_parameter_is_401() {
    let app = TestApp::spawn().await;

    let response = app.server.get(WHOAMI).add_query_param("token", "").await;

    response.assert_status(StatusCode::UNAUTHORIZED);
    response.assert_json(&unauthorized());
}

#[tokio::test]
async fn a_token_that_is_not_a_jwt_is_401() {
    let app = TestApp::spawn().await;

    let response = app
        .server
        .get(WHOAMI)
        .add_query_param("token", "not-a-real-token")
        .await;

    response.assert_status(StatusCode::UNAUTHORIZED);
    response.assert_json(&unauthorized());
}

#[tokio::test]
async fn an_expired_token_is_401_at_open() {
    let app = TestApp::spawn().await;
    let user = app
        .insert_user("streamer", "streamer@example.invalid", false, false)
        .await;

    let response = app
        .server
        .get(WHOAMI)
        .add_query_param("token", app.expired_token_for(&user))
        .await;

    response.assert_status(StatusCode::UNAUTHORIZED);
    response.assert_json(&unauthorized());
}

/// The revocation contract: a token minted before a password change names a
/// `auth_version` the row no longer has, and opening a stream with it is 401
/// without anything having been blacklisted (ADR 0025).
#[tokio::test]
async fn a_token_one_auth_version_behind_is_401() {
    let app = TestApp::spawn().await;
    let user = app
        .insert_user("stale", "stale@example.invalid", false, false)
        .await;

    let claims = Claims {
        auth_version: user.auth_version - 1,
        ..Claims::for_user(&user, Utc::now())
    };

    let response = app
        .server
        .get(WHOAMI)
        .add_query_param("token", app.encode(&claims))
        .await;

    response.assert_status(StatusCode::UNAUTHORIZED);
    response.assert_json(&unauthorized());
}

#[tokio::test]
async fn a_gated_user_cannot_open_a_stream() {
    let app = TestApp::spawn().await;
    let gated = app
        .create_gated_user("gated", "gated@example.invalid")
        .await;

    let response = app
        .server
        .get(WHOAMI)
        .add_query_param("token", &gated.access_token)
        .await;

    response.assert_status(StatusCode::FORBIDDEN);
    response.assert_json(&json!({ "status": 403, "error": "password change required" }));
}

#[tokio::test]
async fn a_valid_token_in_the_query_string_authenticates() {
    let app = TestApp::spawn().await;
    let user = app
        .insert_user("opener", "opener@example.invalid", false, false)
        .await;

    let response = app
        .server
        .get(WHOAMI)
        .add_query_param("token", app.token_for(&user))
        .await;

    response.assert_status_ok();
    response.assert_json(&json!({ "user_id": user.id }));
}

/// Non-browser clients — the integration tests among them — may present the
/// header instead, and it is validated identically.
#[tokio::test]
async fn a_bearer_header_authenticates_when_the_query_string_has_no_token() {
    let app = TestApp::spawn().await;
    let user = app
        .insert_user("header", "header@example.invalid", false, false)
        .await;

    let response = app
        .server
        .get(WHOAMI)
        .authorization_bearer(app.token_for(&user))
        .await;

    response.assert_status_ok();
    response.assert_json(&json!({ "user_id": user.id }));
}

/// The documented precedence, asserted where it is observable: the header
/// names a *different* user, and the answer is the query string's. A header
/// that would fail on its own is likewise not an error.
#[tokio::test]
async fn the_query_parameter_wins_over_a_header() {
    let app = TestApp::spawn().await;
    let query_user = app
        .insert_user("query", "query@example.invalid", false, false)
        .await;
    let header_user = app
        .insert_user("head", "head@example.invalid", false, false)
        .await;

    let response = app
        .server
        .get(WHOAMI)
        .add_query_param("token", app.token_for(&query_user))
        .authorization_bearer(app.token_for(&header_user))
        .await;

    response.assert_status_ok();
    response.assert_json(&json!({ "user_id": query_user.id }));

    let response = app
        .server
        .get(WHOAMI)
        .add_query_param("token", app.token_for(&query_user))
        .authorization_bearer("not-a-real-token")
        .await;

    response.assert_status_ok();
    response.assert_json(&json!({ "user_id": query_user.id }));
}

/// The stream endpoints carry `after` as well, and the extractor has to ignore
/// it rather than reject the request.
#[tokio::test]
async fn other_query_parameters_are_ignored() {
    let app = TestApp::spawn().await;
    let user = app
        .insert_user("after", "after@example.invalid", false, false)
        .await;

    let response = app
        .server
        .get(WHOAMI)
        .add_query_param("after", 42)
        .add_query_param("token", app.token_for(&user))
        .await;

    response.assert_status_ok();
    response.assert_json(&json!({ "user_id": user.id }));
}

// --------------------------------------------------------- continuation

#[tokio::test]
async fn an_unchanged_account_stays_authorized() {
    let app = TestApp::spawn().await;
    let user = app
        .insert_user("live", "live@example.invalid", false, false)
        .await;

    let principal = StreamPrincipal::new(&user);

    assert_eq!(reauthorize(&app.state, &principal).await, Ok(()));
}

#[tokio::test]
async fn a_raised_auth_version_closes_the_stream() {
    let app = TestApp::spawn().await;
    let user = app
        .insert_user("bumped", "bumped@example.invalid", false, false)
        .await;
    let principal = StreamPrincipal::new(&user);

    sqlx::query("UPDATE users SET auth_version = auth_version + 1 WHERE id = $1")
        .bind(user.id)
        .execute(&app.pool)
        .await
        .expect("the update applies");

    assert_eq!(
        reauthorize(&app.state, &principal).await,
        Err(StreamAuthFailure::Revoked)
    );
}

#[tokio::test]
async fn the_password_change_gate_closes_an_open_stream() {
    let app = TestApp::spawn().await;
    let user = app
        .insert_user("tobegated", "tobegated@example.invalid", false, false)
        .await;
    let principal = StreamPrincipal::new(&user);

    sqlx::query("UPDATE users SET must_change_password = true WHERE id = $1")
        .bind(user.id)
        .execute(&app.pool)
        .await
        .expect("the update applies");

    assert_eq!(
        reauthorize(&app.state, &principal).await,
        Err(StreamAuthFailure::Revoked)
    );
}

#[tokio::test]
async fn a_deleted_user_closes_the_stream() {
    let app = TestApp::spawn().await;
    let user = app
        .insert_user("doomed", "doomed@example.invalid", false, false)
        .await;
    let principal = StreamPrincipal::new(&user);

    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(user.id)
        .execute(&app.pool)
        .await
        .expect("the delete applies");

    assert_eq!(
        reauthorize(&app.state, &principal).await,
        Err(StreamAuthFailure::Revoked)
    );
}

/// A principal that never existed is the same answer, so a stream cannot be
/// kept open by a row that is simply not there.
#[tokio::test]
async fn an_unknown_user_id_closes_the_stream() {
    let app = TestApp::spawn().await;

    let principal = StreamPrincipal {
        user_id: Uuid::new_v4(),
        auth_version: 1,
    };

    assert_eq!(
        reauthorize(&app.state, &principal).await,
        Err(StreamAuthFailure::Revoked)
    );
}

/// The rule this module exists for (`SPEC.md`, "Authentication"): "Database
/// failures must not authorize input or continued streaming."
///
/// A second pool over the same database, closed before the check, is the
/// cheapest reproduction of a database that does not answer: the state under
/// test is `TestApp`'s with only its pool replaced, so nothing else about the
/// application differs. The user row is present and unchanged throughout, so a
/// pass here could only come from the failure being swallowed.
#[tokio::test]
async fn a_database_failure_is_never_an_authorization() {
    let app = TestApp::spawn().await;
    let user = app
        .insert_user("severed", "severed@example.invalid", false, false)
        .await;
    let principal = StreamPrincipal::new(&user);

    assert_eq!(reauthorize(&app.state, &principal).await, Ok(()));

    let closed = PgPoolOptions::new()
        .connect(&app.state.config.database_url)
        .await
        .expect("a second pool connects");
    closed.close().await;

    let mut severed = app.state.clone();
    severed.pool = closed;

    assert_eq!(
        reauthorize(&severed, &principal).await,
        Err(StreamAuthFailure::Unavailable)
    );
}
