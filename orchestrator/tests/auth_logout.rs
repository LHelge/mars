//! `POST /api/auth/logout` through the real router (`SPEC.md`, "Auth
//! (`/api/auth`)").
//!
//! Logging out is not an operation that can be refused: 204 with a cookie,
//! 204 without one, and 204 for a cookie that names nothing — telling a caller
//! which of those they sent would be an oracle over stored tokens. What it does
//! do is revoke the presented token and clear the cookie.
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use axum::http::StatusCode;
use axum_extra::extract::cookie::Cookie;
use chrono::{DateTime, Utc};
use common::TestApp;
use common::app::assert_refresh_cookie_cleared;
use mars_orchestrator::auth::REFRESH_COOKIE;
use mars_orchestrator::models::OpaqueToken;

const LOGOUT: &str = "/api/auth/logout";

/// Obviously fake (`CLAUDE.md`, rule 3).
const PASSWORD: &str = "correct-horse-battery-staple";

/// `POST /api/auth/logout` carrying `cookie` as the refresh cookie.
async fn logout_with(app: &TestApp, cookie: &str) -> axum_test::TestResponse {
    app.server
        .post(LOGOUT)
        .add_cookie(Cookie::new(REFRESH_COOKIE, cookie.to_string()))
        .await
}

/// `revoked_at` of the row behind `raw`, if the row is still there.
///
/// A row-level fact with no interface: no route answers *when* a token was
/// revoked, and a second logout not moving that moment is what these tests are
/// about.
async fn revoked_at(app: &TestApp, raw: &str) -> Option<DateTime<Utc>> {
    sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
        "SELECT revoked_at FROM refresh_tokens WHERE token_hash = $1",
    )
    .bind(OpaqueToken::hash_of(raw))
    .fetch_one(&app.pool)
    .await
    .expect("the token row is there")
}

#[tokio::test]
async fn logout_revokes_the_presented_token_and_clears_the_cookie() {
    let app = TestApp::spawn().await;
    app.insert_user_with_password("ada", "ada@example.test", PASSWORD, false, false)
        .await;
    let (_pair, cookie) = app.login("ada", PASSWORD).await;

    assert_eq!(revoked_at(&app, &cookie).await, None);

    let response = logout_with(&app, &cookie).await;

    response.assert_status(StatusCode::NO_CONTENT);
    assert_refresh_cookie_cleared(&response);

    assert!(
        revoked_at(&app, &cookie).await.is_some(),
        "the presented token must be revoked"
    );

    // And it is dead: the cookie cannot be exchanged any more.
    app.refresh(&cookie)
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn logout_without_a_cookie_is_still_204() {
    let app = TestApp::spawn().await;

    let response = app.server.post(LOGOUT).await;

    response.assert_status(StatusCode::NO_CONTENT);
    assert_refresh_cookie_cleared(&response);
}

/// A cookie that names no row, and a second logout with an already revoked
/// one: both 204. Repeating a request whose response was never seen must not
/// produce an error.
#[tokio::test]
async fn logout_is_204_for_an_unknown_and_for_an_already_revoked_token() {
    let app = TestApp::spawn().await;
    app.insert_user_with_password("ada", "ada@example.test", PASSWORD, false, false)
        .await;
    let (_pair, cookie) = app.login("ada", PASSWORD).await;

    logout_with(
        &app,
        "00000000-0000-4000-8000-000000000001.00000000-0000-4000-8000-000000000002",
    )
    .await
    .assert_status(StatusCode::NO_CONTENT);

    logout_with(&app, &cookie)
        .await
        .assert_status(StatusCode::NO_CONTENT);
    let first = revoked_at(&app, &cookie).await.expect("revoked once");

    logout_with(&app, &cookie)
        .await
        .assert_status(StatusCode::NO_CONTENT);
    assert_eq!(
        revoked_at(&app, &cookie).await,
        Some(first),
        "a second logout must not move the original revocation time"
    );
}

/// Only the presented token: another browser of the same user stays signed in
/// (`SPEC.md`, "Authentication" reserves wholesale revocation for password
/// changes).
#[tokio::test]
async fn logout_leaves_the_user_s_other_sessions_alone() {
    let app = TestApp::spawn().await;
    app.insert_user_with_password("ada", "ada@example.test", PASSWORD, false, false)
        .await;
    let (_first, one) = app.login("ada", PASSWORD).await;
    let (_second, other) = app.login("ada", PASSWORD).await;

    logout_with(&app, &one)
        .await
        .assert_status(StatusCode::NO_CONTENT);

    assert_eq!(revoked_at(&app, &other).await, None);
    app.refresh(&other).await.assert_status(StatusCode::OK);
}
