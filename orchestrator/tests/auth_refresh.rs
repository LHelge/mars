//! `POST /api/auth/refresh` through the real router (`SPEC.md`,
//! "Authentication"; ADR 0025).
//!
//! Rotation and revocation are one contract: a refresh either commits before a
//! password change and is revoked by it, or observes the revocation and fails
//! (`docs/data-model.md`, "Users and authentication"). The tests below drive
//! both sides of that — the rotated-away cookie, the revoked cookie, the
//! expired row, the deleted user — and check that a rejection always clears the
//! cookie, because a browser holding a token the database will never accept
//! again should stop sending it.
//!
//! The new pair is built from the *current* row, not from the old token's
//! claims, which is what a demotion between two refreshes proves.
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use axum::http::StatusCode;
use chrono::{TimeDelta, Utc};
use common::TestApp;
use common::app::assert_refresh_cookie_cleared;
use common::races::unrevoked_refresh_tokens;
use mars_orchestrator::auth::REFRESH_COOKIE;
use mars_orchestrator::models::{OpaqueToken, User};
use mars_orchestrator::prelude::*;
use serde_json::{Value, json};
use uuid::Uuid;

const REFRESH: &str = "/api/auth/refresh";

/// Obviously fake (`CLAUDE.md`, rule 3).
const PASSWORD: &str = "correct-horse-battery-staple";

/// The documented 401 body (`SPEC.md`, "Authentication").
fn unauthorized() -> Value {
    json!({ "status": 401, "error": "authentication required" })
}

/// A route behind `CurrentUser`, which is what an access token is checked
/// against here: `GET /users/{id}`, pointed at `user`'s own id so a 200 means
/// the extractor let the request through rather than that the lookup found
/// something (`SPEC.md`, "Users").
fn gated(user: &User) -> String {
    format!("/api/users/{}", user.id)
}

/// Obviously fake, and long enough for the documented 10–128 range.
const NEW_PASSWORD: &str = "a-completely-different-phrase";

/// `POST /users/{id}/password` for `id`.
fn password_route(id: Uuid) -> String {
    format!("/api/users/{id}/password")
}

/// Assert that `response` is the documented 401 *and* clears the cookie.
fn assert_rejected_and_cleared(response: &axum_test::TestResponse) {
    response.assert_status(StatusCode::UNAUTHORIZED);
    assert_eq!(response.json::<Value>(), unauthorized());
    assert_refresh_cookie_cleared(response);
}

#[tokio::test]
async fn a_refresh_returns_a_new_pair_and_a_new_cookie() {
    let app = TestApp::spawn().await;
    let user = app
        .insert_user_with_password("ada", "ada@example.test", PASSWORD, false, false)
        .await;
    let (first, first_cookie) = app.login("ada", PASSWORD).await;

    let response = app.refresh(&first_cookie).await;

    response.assert_status(StatusCode::OK);
    let second = response.json::<Value>();
    assert_eq!(second["user"]["id"], json!(user.id.to_string()));
    assert_eq!(second["user"].get("password_hash"), None);

    let access_token = second["access_token"].as_str().expect("an access token");
    let claims = Claims::decode(access_token, &app.state.config).expect("the token verifies");
    assert_eq!(claims.sub, user.id);
    assert_eq!(claims.exp - claims.iat, 900);

    let second_cookie = response.cookie(REFRESH_COOKIE).value().to_string();
    assert_ne!(
        second_cookie, first_cookie,
        "the refresh token must be rotated, not reissued"
    );

    // The old row is revoked and the new one is not, so exactly one usable
    // token remains for this user.
    assert_eq!(unrevoked_refresh_tokens(&app.pool, user.id).await, 1);

    // And the new access token works, while the first one is untouched: an
    // access token is not revoked by rotation, it simply expires.
    app.server
        .get(&gated(&user))
        .authorization_bearer(access_token)
        .await
        .assert_status(StatusCode::OK);
    app.server
        .get(&gated(&user))
        .authorization_bearer(&first.access_token)
        .await
        .assert_status(StatusCode::OK);
}

/// The rotated-away cookie. Also the two-concurrent-refreshes case: the second
/// one re-reads the row under the lock, sees `revoked_at` and answers 401
/// (there is no token-family reuse detection in v1).
#[tokio::test]
async fn the_previous_cookie_is_rejected_and_cleared() {
    let app = TestApp::spawn().await;
    app.insert_user_with_password("ada", "ada@example.test", PASSWORD, false, false)
        .await;
    let (_pair, first_cookie) = app.login("ada", PASSWORD).await;

    app.refresh(&first_cookie)
        .await
        .assert_status(StatusCode::OK);

    assert_rejected_and_cleared(&app.refresh(&first_cookie).await);
}

#[tokio::test]
async fn a_request_without_a_cookie_is_401() {
    let app = TestApp::spawn().await;

    let response = app.server.post(REFRESH).await;

    assert_rejected_and_cleared(&response);
}

#[tokio::test]
async fn a_cookie_that_matches_no_row_is_401() {
    let app = TestApp::spawn().await;
    app.insert_user_with_password("ada", "ada@example.test", PASSWORD, false, false)
        .await;
    app.login("ada", PASSWORD).await;

    // Well formed and never issued.
    let response = app
        .refresh("00000000-0000-4000-8000-000000000001.00000000-0000-4000-8000-000000000002")
        .await;

    assert_rejected_and_cleared(&response);
}

#[tokio::test]
async fn an_expired_token_is_401() {
    let app = TestApp::spawn().await;
    app.insert_user_with_password("ada", "ada@example.test", PASSWORD, false, false)
        .await;
    let (_pair, cookie) = app.login("ada", PASSWORD).await;

    // A row-level fact with no interface: the expiry lives in the row and no
    // route moves it, so the row is aged rather than the clock.
    sqlx::query(
        "UPDATE refresh_tokens SET expires_at = NOW() - INTERVAL '1 second' WHERE token_hash = $1",
    )
    .bind(OpaqueToken::hash_of(&cookie))
    .execute(&app.pool)
    .await
    .expect("the token is aged");

    assert_rejected_and_cleared(&app.refresh(&cookie).await);
}

#[tokio::test]
async fn a_cookie_for_a_deleted_user_is_401() {
    let app = TestApp::spawn().await;
    let user = app
        .insert_user_with_password("ada", "ada@example.test", PASSWORD, false, false)
        .await;
    let (_pair, cookie) = app.login("ada", PASSWORD).await;

    // Deleted the way a user is deleted: by an administrator, over the route
    // (`SPEC.md`, "Users").
    let admin = app
        .create_admin("root", "root@example.test", PASSWORD)
        .await;
    app.delete_as(&admin, &format!("/api/users/{}", user.id))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    assert_rejected_and_cleared(&app.refresh(&cookie).await);
}

/// The revocation race the epic is about: a password change revokes every
/// refresh token in the same transaction that raises `auth_version`, so a
/// cookie issued before it is dead (`SPEC.md`, "Authentication"; ADR 0025).
#[tokio::test]
async fn a_refresh_after_a_password_change_is_401() {
    let app = TestApp::spawn().await;
    let user = app
        .insert_user_with_password("ada", "ada@example.test", PASSWORD, false, false)
        .await;
    let (pair, cookie) = app.login("ada", PASSWORD).await;

    // An administrator's change, over the route: it issues no replacement, so
    // everything `ada` holds is dead afterwards (`SPEC.md`, "Users").
    let admin = app
        .create_admin("root", "root@example.test", PASSWORD)
        .await;
    app.post_as(&admin, &password_route(user.id))
        .json(&json!({ "password": NEW_PASSWORD }))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    assert_rejected_and_cleared(&app.refresh(&cookie).await);

    // The access token minted with it is dead too, through `auth_version`.
    app.server
        .get(&gated(&user))
        .authorization_bearer(&pair.access_token)
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

/// The new pair is built from the row read under the lock, so a demotion
/// between two refreshes is in the next access token's claims — and, more to
/// the point, in what the administrator routes decide.
#[tokio::test]
async fn the_refreshed_token_carries_the_current_admin_flag() {
    let app = TestApp::spawn().await;
    let user = app
        .insert_user_with_password("ada", "ada@example.test", PASSWORD, true, false)
        .await;
    let (pair, cookie) = app.login("ada", PASSWORD).await;

    let claims = Claims::decode(&pair.access_token, &app.state.config).expect("it verifies");
    assert!(claims.admin);

    // Demoted over the route by a second administrator, because demoting the
    // last one is refused (`SPEC.md`, "Users"). A demotion does not touch
    // `auth_version` and revokes nothing (ADR 0025), so the cookie stays
    // usable.
    let root = app
        .create_admin("root", "root@example.test", PASSWORD)
        .await;
    app.put_as(&root, &format!("/api/users/{}", user.id))
        .json(&json!({ "username": "ada", "admin": false }))
        .await
        .assert_status(StatusCode::OK);

    let response = app.refresh(&cookie).await;
    response.assert_status(StatusCode::OK);

    let body = response.json::<Value>();
    assert_eq!(body["user"]["admin"], json!(false));

    let claims = Claims::decode(
        body["access_token"].as_str().expect("a token"),
        &app.state.config,
    )
    .expect("it verifies");
    assert!(!claims.admin);
    assert_eq!(claims.auth_version, user.auth_version);
}

/// A refresh cookie is good for thirty days and the replacement starts the
/// clock again (`docs/data-model.md`, `refresh_tokens`).
#[tokio::test]
async fn the_replacement_token_is_valid_for_thirty_days() {
    let app = TestApp::spawn().await;
    app.insert_user_with_password("ada", "ada@example.test", PASSWORD, false, false)
        .await;
    let (_pair, cookie) = app.login("ada", PASSWORD).await;

    let response = app.refresh(&cookie).await;
    response.assert_status(StatusCode::OK);
    let replacement = response.cookie(REFRESH_COOKIE).value().to_string();

    // A row-level fact with no interface: the replacement's own expiry, thirty
    // days out, lives only in the row.
    let expires_at: chrono::DateTime<Utc> =
        sqlx::query_scalar("SELECT expires_at FROM refresh_tokens WHERE token_hash = $1")
            .bind(OpaqueToken::hash_of(&replacement))
            .fetch_one(&app.pool)
            .await
            .expect("the replacement row is there");

    let ttl = expires_at - Utc::now();
    assert!(
        ttl > REFRESH_TOKEN_TTL - TimeDelta::minutes(1) && ttl <= REFRESH_TOKEN_TTL,
        "unexpected expiry: {ttl}"
    );
}
