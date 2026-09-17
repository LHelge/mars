//! `POST /api/auth/login` through the real router (`SPEC.md`, "Auth
//! (`/api/auth`)"; "Authentication").
//!
//! What is asserted here is the whole documented contract of one endpoint: the
//! body shape, the access token's claims and lifetime, the exact `Set-Cookie`
//! attributes for the harness `PUBLIC_URL`, the single indistinguishable 401
//! for a wrong password and an unknown name, and the throttle — including that
//! it keys on the client address as well as on the username.
//!
//! Every password in this file is obviously fake (`CLAUDE.md`, rule 3).
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use axum::http::StatusCode;
use axum::http::header::SET_COOKIE;
use chrono::Utc;
use common::TestApp;
use mars_orchestrator::prelude::*;
use serde_json::{Value, json};

const LOGIN: &str = "/api/auth/login";

/// The route behind `UngatedUser`: what an access token is checked against
/// here, because a user with `must_change_password` is refused by every gated
/// one (`SPEC.md`, "Authentication").
const UNGATED: &str = "/api/users/me";

/// Obviously fake, and long enough for the documented 10–128 range.
const PASSWORD: &str = "correct-horse-battery-staple";

/// The documented 401 body for a failed login.
fn invalid_credentials() -> Value {
    json!({ "status": 401, "error": "invalid username or password" })
}

/// The `Set-Cookie` a login must produce for the harness `PUBLIC_URL`
/// (`http://localhost`, so no `Secure`).
fn expected_set_cookie(raw: &str) -> String {
    format!("refresh_token={raw}; HttpOnly; SameSite=Lax; Path=/; Max-Age=2592000")
}

#[tokio::test]
async fn a_correct_password_returns_a_user_an_access_token_and_the_cookie() {
    let app = TestApp::spawn().await;
    let user = app
        .insert_user_with_password("ada", "ada@example.test", PASSWORD, true, false)
        .await;

    let response = app
        .server
        .post(LOGIN)
        .json(&json!({ "username": "ada", "password": PASSWORD }))
        .await;

    response.assert_status(StatusCode::OK);
    let body = response.json::<Value>();

    // The documented `User` DTO, and nothing beyond it (`SPEC.md`, "Users").
    assert_eq!(body["user"]["id"], json!(user.id.to_string()));
    assert_eq!(body["user"]["username"], json!("ada"));
    assert_eq!(body["user"]["email"], json!("ada@example.test"));
    assert_eq!(body["user"]["admin"], json!(true));
    assert_eq!(body["user"]["must_change_password"], json!(false));
    assert_eq!(body["user"].get("password_hash"), None);
    assert_eq!(body["user"].get("auth_version"), None);

    // The access token is this user's, at the row's current generation, and
    // lives exactly fifteen minutes (`SPEC.md`, "User-facing features").
    let token = body["access_token"].as_str().expect("an access token");
    let claims = Claims::decode(token, &app.state.config).expect("the token verifies");
    assert_eq!(claims.sub, user.id);
    assert_eq!(claims.auth_version, user.auth_version);
    assert_eq!(claims.username, "ada");
    assert!(claims.admin);
    assert!(!claims.must_change_password);
    assert_eq!(claims.exp - claims.iat, 900);

    // The cookie carries the raw refresh token, which is never in the body.
    let cookie = response.cookie(REFRESH_COOKIE);
    assert!(!cookie.value().is_empty());
    assert!(
        !response.text().contains(cookie.value()),
        "the refresh token must travel only in the cookie"
    );

    let header = response
        .headers()
        .get(SET_COOKIE)
        .expect("a Set-Cookie header")
        .to_str()
        .expect("an ASCII header");
    assert_eq!(header, expected_set_cookie(cookie.value()));
}

/// The token a login hands out is one the rest of the API accepts.
#[tokio::test]
async fn the_access_token_authenticates_the_next_request() {
    let app = TestApp::spawn().await;
    let user = app
        .insert_user_with_password("ada", "ada@example.test", PASSWORD, false, false)
        .await;

    let (pair, _cookie) = app.login("ada", PASSWORD).await;
    let response = app
        .server
        .get(UNGATED)
        .authorization_bearer(&pair.access_token)
        .await;

    response.assert_status(StatusCode::OK);
    assert_eq!(response.json::<Value>()["id"], json!(user.id.to_string()));
}

#[tokio::test]
async fn a_wrong_password_is_401() {
    let app = TestApp::spawn().await;
    app.insert_user_with_password("ada", "ada@example.test", PASSWORD, false, false)
        .await;

    let response = app
        .server
        .post(LOGIN)
        .json(&json!({ "username": "ada", "password": "not-the-right-password" }))
        .await;

    response.assert_status(StatusCode::UNAUTHORIZED);
    assert_eq!(response.json::<Value>(), invalid_credentials());
    assert_eq!(response.maybe_cookie(REFRESH_COOKIE), None);
}

/// Byte for byte the same answer as a wrong password: the endpoint must not
/// say whether a username exists.
#[tokio::test]
async fn an_unknown_username_is_the_same_401() {
    let app = TestApp::spawn().await;
    app.insert_user_with_password("ada", "ada@example.test", PASSWORD, false, false)
        .await;

    let response = app
        .server
        .post(LOGIN)
        .json(&json!({ "username": "nobody", "password": PASSWORD }))
        .await;

    response.assert_status(StatusCode::UNAUTHORIZED);
    assert_eq!(response.json::<Value>(), invalid_credentials());
}

#[tokio::test]
async fn a_body_missing_a_field_is_400() {
    let app = TestApp::spawn().await;

    for body in [
        json!({ "username": "ada" }),
        json!({ "password": PASSWORD }),
        json!({}),
    ] {
        let response = app.server.post(LOGIN).json(&body).await;

        response.assert_status(StatusCode::BAD_REQUEST);
        assert_eq!(response.json::<Value>()["status"], json!(400), "{body}");
    }
}

/// The username key is trimmed before the lookup; the password is not touched.
#[tokio::test]
async fn the_username_is_trimmed_and_the_password_is_used_verbatim() {
    let app = TestApp::spawn().await;
    app.insert_user_with_password("ada", "ada@example.test", PASSWORD, false, false)
        .await;

    app.server
        .post(LOGIN)
        .json(&json!({ "username": "  ada  ", "password": PASSWORD }))
        .await
        .assert_status(StatusCode::OK);

    // A padded password is a different password.
    app.server
        .post(LOGIN)
        .json(&json!({ "username": "ada", "password": format!(" {PASSWORD} ") }))
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn ten_failures_block_the_username_with_429() {
    let app = TestApp::spawn().await;
    app.insert_user_with_password("ada", "ada@example.test", PASSWORD, false, false)
        .await;

    for attempt in 0..10 {
        let response = app
            .server
            .post(LOGIN)
            .json(&json!({ "username": "ada", "password": "wrong-password-here" }))
            .await;
        response.assert_status(StatusCode::UNAUTHORIZED);
        assert_eq!(response.json::<Value>(), invalid_credentials(), "{attempt}");
    }

    // The eleventh is refused before the password is looked at, so even the
    // correct one does not get through.
    let response = app
        .server
        .post(LOGIN)
        .json(&json!({ "username": "ada", "password": PASSWORD }))
        .await;

    response.assert_status(StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        response.json::<Value>(),
        json!({ "status": 429, "error": "too many login attempts" })
    );

    // And the block is not a fact about the database: clearing the counters
    // lets the same credentials through.
    app.reset_limiters();
    app.server
        .post(LOGIN)
        .json(&json!({ "username": "ada", "password": PASSWORD }))
        .await
        .assert_status(StatusCode::OK);
}

/// The address is a key of its own, so ten failures spread over ten usernames
/// block an eleventh username from the same client (`SPEC.md`,
/// "Authentication").
#[tokio::test]
async fn ten_failures_from_one_address_block_a_different_username() {
    let app = TestApp::spawn().await;
    app.insert_user_with_password("ada", "ada@example.test", PASSWORD, false, false)
        .await;

    // `axum-test`'s mock transport has no peer, so the client address comes
    // from `X-Forwarded-For`; documentation addresses only (RFC 5737).
    for nth in 0..10 {
        app.server
            .post(LOGIN)
            .add_header("X-Forwarded-For", "203.0.113.7")
            .json(&json!({ "username": format!("user{nth}"), "password": "wrong-password-here" }))
            .await
            .assert_status(StatusCode::UNAUTHORIZED);
    }

    app.server
        .post(LOGIN)
        .add_header("X-Forwarded-For", "203.0.113.7")
        .json(&json!({ "username": "ada", "password": PASSWORD }))
        .await
        .assert_status(StatusCode::TOO_MANY_REQUESTS);

    // Another client is unaffected: the block is on that address, not on the
    // account.
    app.server
        .post(LOGIN)
        .add_header("X-Forwarded-For", "203.0.113.8")
        .json(&json!({ "username": "ada", "password": PASSWORD }))
        .await
        .assert_status(StatusCode::OK);
}

/// Login is one of the five routes the password-change gate exempts, so the
/// seeded-administrator case works: log in, then change the password.
#[tokio::test]
async fn a_user_who_must_change_their_password_can_still_log_in() {
    let app = TestApp::spawn().await;
    let user = app
        .insert_user_with_password("ada", "ada@example.test", PASSWORD, false, true)
        .await;

    let (pair, cookie) = app.login("ada", PASSWORD).await;

    assert_eq!(pair.user["must_change_password"], json!(true));
    assert!(!cookie.is_empty());

    let claims = Claims::decode(&pair.access_token, &app.state.config).expect("the token verifies");
    assert!(claims.must_change_password);

    // The gate itself is unchanged: that token still cannot reach a gated
    // route (`SPEC.md`, "Authentication"). `GET /users/{id}` is one, pointed
    // at the caller's own id so the 403 can only be the gate.
    app.server
        .get(&format!("/api/users/{}", user.id))
        .authorization_bearer(&pair.access_token)
        .await
        .assert_status(StatusCode::FORBIDDEN);
}

/// The login is what wrote the row, and it is usable for thirty days
/// (`docs/data-model.md`, `refresh_tokens`).
#[tokio::test]
async fn login_stores_one_usable_refresh_token_hashed() {
    let app = TestApp::spawn().await;
    let user = app
        .insert_user_with_password("ada", "ada@example.test", PASSWORD, false, false)
        .await;

    let (_pair, cookie) = app.login("ada", PASSWORD).await;

    let row: (
        uuid::Uuid,
        String,
        chrono::DateTime<Utc>,
        Option<chrono::DateTime<Utc>>,
    ) = sqlx::query_as("SELECT user_id, token_hash, expires_at, revoked_at FROM refresh_tokens")
        .fetch_one(&app.pool)
        .await
        .expect("exactly one refresh token row");

    assert_eq!(row.0, user.id);
    assert_eq!(
        row.1,
        mars_orchestrator::models::OpaqueToken::hash_of(&cookie)
    );
    assert_ne!(row.1, cookie, "the raw token must never be stored");
    assert_eq!(row.3, None);

    let ttl = row.2 - Utc::now();
    assert!(
        ttl > REFRESH_TOKEN_TTL - chrono::TimeDelta::minutes(1) && ttl <= REFRESH_TOKEN_TTL,
        "unexpected expiry: {ttl}"
    );
}
