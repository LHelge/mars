//! `POST /api/test/users` and `POST /api/test/throttle/reset` through the real
//! router (`SPEC.md`, "Test-only routes").
//!
//! The fixture endpoint Playwright and every later epic's tests create their
//! users with, so what is asserted here is that a user it made is an ordinary
//! one: the row carries the flags the specification fixes, the access token
//! reaches the routes its `admin` flag allows and the cookie it set refreshes
//! like one from a login. The failures matter as much — a Playwright run that
//! reuses a username must be told, not quietly handed somebody else's fixture.
//!
//! Every password in this file is obviously fake (`CLAUDE.md`, rule 3).
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use axum::http::StatusCode;
use axum::http::header::SET_COOKIE;
use common::TestApp;
use mars_orchestrator::auth::REFRESH_COOKIE;
use mars_orchestrator::prelude::*;
use serde_json::{Value, json};

const CREATE: &str = "/api/test/users";
const THROTTLE_RESET: &str = "/api/test/throttle/reset";
const LOGIN: &str = "/api/auth/login";

/// The client address the throttle scenario fails from (RFC 5737).
const CLIENT: &str = "203.0.113.7";

/// Obviously fake, and inside the documented 10–128 range.
const PASSWORD: &str = "correct-horse-battery-staple";

#[tokio::test]
async fn creating_a_user_answers_201_with_a_pair_and_the_refresh_cookie() {
    let app = TestApp::spawn().await;

    let response = app
        .server
        .post(CREATE)
        .json(&json!({
            "username": "ada",
            "email": "ada@example.test",
            "password": PASSWORD,
        }))
        .await;

    response.assert_status(StatusCode::CREATED);

    let body = response.json::<Value>();
    assert_eq!(body["user"]["username"], json!("ada"));
    assert_eq!(body["user"]["email"], json!("ada@example.test"));
    // The three flags the specification fixes: not gated, not an
    // administrator unless asked for, and notified by mail as every new row is
    // (`docs/data-model.md`, `users`).
    assert_eq!(body["user"]["must_change_password"], json!(false));
    assert_eq!(body["user"]["admin"], json!(false));
    assert_eq!(body["user"]["notify_email"], json!(true));
    // The `User` DTO, so the internal columns are absent (`SPEC.md`, "Users").
    assert_eq!(body["user"].get("password_hash"), None);
    assert_eq!(body["user"].get("auth_version"), None);
    // The refresh token travels in the cookie and nowhere else.
    assert!(body["access_token"].is_string());
    assert_eq!(body.get("refresh_token"), None);

    let cookie = response.cookie(REFRESH_COOKIE);
    assert!(!cookie.value().is_empty());

    // The same attributes login sets, because both go through
    // `routes::cookies` (`SPEC.md`, "Authentication").
    let header = response
        .headers()
        .get(SET_COOKIE)
        .expect("the response sets a cookie")
        .to_str()
        .expect("the header is ascii")
        .to_string();
    assert!(header.contains("HttpOnly"), "{header}");
    assert!(header.contains("SameSite=Lax"), "{header}");
    assert!(header.contains("Path=/"), "{header}");
}

/// The access token is a real one: it authenticates on an ordinary route and
/// carries the claims minted from the committed row.
#[tokio::test]
async fn the_access_token_it_returns_works_on_users_me() {
    let app = TestApp::spawn().await;
    let ada = app.create_user("ada", "ada@example.test", PASSWORD).await;

    let response = app.get_as(&ada, "/api/users/me").await;

    response.assert_status(StatusCode::OK);
    assert_eq!(
        response.json::<Value>()["id"],
        json!(ada.user.id.to_string())
    );

    let claims = Claims::decode(&ada.access_token, &app.state.config).expect("the token verifies");
    assert_eq!(claims.sub, ada.user.id);
    assert_eq!(claims.auth_version, ada.user.auth_version);
    assert!(!claims.must_change_password);
    assert!(!claims.admin);
}

/// The cookie it set is a stored refresh token, so the browser Playwright
/// drives can rotate it like any other.
#[tokio::test]
async fn the_cookie_it_sets_refreshes() {
    let app = TestApp::spawn().await;
    let ada = app.create_user("ada", "ada@example.test", PASSWORD).await;

    let response = app.refresh(&ada.refresh_cookie).await;

    response.assert_status(StatusCode::OK);
    let rotated = response.cookie(REFRESH_COOKIE).value().to_string();
    assert_ne!(rotated, ada.refresh_cookie, "the token must be rotated");
    assert_eq!(
        response.json::<Value>()["user"]["id"],
        json!(ada.user.id.to_string())
    );
}

/// The password is hashed, not stored, and it is the one the caller chose:
/// logging in with it through the real route is the proof.
#[tokio::test]
async fn the_password_it_stores_is_the_one_that_logs_in() {
    let app = TestApp::spawn().await;
    let ada = app.create_user("ada", "ada@example.test", PASSWORD).await;

    let (pair, cookie) = app.login("ada", PASSWORD).await;

    assert_eq!(pair.user["id"], json!(ada.user.id.to_string()));
    assert!(!cookie.is_empty());
}

#[tokio::test]
async fn admin_true_creates_a_user_who_can_reach_the_administrator_routes() {
    let app = TestApp::spawn().await;
    let grace = app
        .create_admin("grace", "grace@example.test", PASSWORD)
        .await;
    let ada = app.create_user("ada", "ada@example.test", PASSWORD).await;

    assert!(grace.user.admin);
    assert!(!ada.user.admin);

    let response = app.get_as(&grace, "/api/users").await;
    response.assert_status(StatusCode::OK);
    assert_eq!(
        response.json::<Vec<Value>>().len(),
        2,
        "the seeded administrator is removed by the harness"
    );

    // And the flag is the row's, not the request's: an ordinary user created
    // the same way is still refused.
    app.get_as(&ada, "/api/users")
        .await
        .assert_status(StatusCode::FORBIDDEN);
}

/// `TestApp::spawn` removes the seeded administrator, so the name is free —
/// which is what lets an end-to-end test create its own `admin`.
#[tokio::test]
async fn the_name_of_the_seeded_administrator_is_free() {
    let app = TestApp::spawn().await;

    let response = app
        .server
        .post(CREATE)
        .json(&json!({
            "username": "admin",
            "email": "admin@example.test",
            "password": PASSWORD,
            "admin": true,
        }))
        .await;

    response.assert_status(StatusCode::CREATED);
    assert_eq!(response.json::<Value>()["user"]["username"], json!("admin"));
}

#[tokio::test]
async fn a_duplicate_username_is_409() {
    let app = TestApp::spawn().await;
    app.create_user("ada", "ada@example.test", PASSWORD).await;

    let response = app
        .server
        .post(CREATE)
        .json(&json!({
            "username": "ada",
            "email": "someone-else@example.test",
            "password": PASSWORD,
        }))
        .await;

    response.assert_status(StatusCode::CONFLICT);
    assert_eq!(
        response.json::<Value>(),
        json!({ "status": 409, "error": "username already taken" })
    );
}

#[tokio::test]
async fn a_duplicate_email_is_409() {
    let app = TestApp::spawn().await;
    app.create_user("ada", "ada@example.test", PASSWORD).await;

    let response = app
        .server
        .post(CREATE)
        .json(&json!({
            "username": "grace",
            // Differently cased and padded: the address is normalised before
            // it reaches the unique index (`docs/data-model.md`).
            "email": "  Ada@Example.TEST  ",
            "password": PASSWORD,
        }))
        .await;

    response.assert_status(StatusCode::CONFLICT);
    assert_eq!(
        response.json::<Value>(),
        json!({ "status": 409, "error": "email already registered" })
    );
}

/// Every validation rejection is the model's, so the message names the field
/// and never echoes the value — least of all the password (rule 3).
#[tokio::test]
async fn invalid_input_is_400_from_the_user_models() {
    let app = TestApp::spawn().await;

    let cases = [
        (
            json!({ "username": "ada", "email": "ada@example.test", "password": "short" }),
            "password must be 10-128 characters",
        ),
        (
            json!({ "username": "no", "email": "ada@example.test", "password": PASSWORD }),
            "username must be 3-32 characters",
        ),
        (
            json!({ "username": "ada", "email": "not-an-address", "password": PASSWORD }),
            "email must be an address of the form local@domain",
        ),
    ];

    for (body, message) in cases {
        let response = app.server.post(CREATE).json(&body).await;

        response.assert_status(StatusCode::BAD_REQUEST);
        assert_eq!(
            response.json::<Value>(),
            json!({ "status": 400, "error": message }),
            "{body}"
        );
    }

    // Nothing was written by any of them.
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&app.pool)
        .await
        .expect("the count runs");
    assert_eq!(count, 0);
}

/// A rejected password is never hashed and never reaches a log line; the
/// assertion available from here is that the 400 body does not carry it.
#[tokio::test]
async fn a_rejected_password_is_not_echoed_back() {
    let app = TestApp::spawn().await;
    let too_short = "nope";

    let response = app
        .server
        .post(CREATE)
        .json(&json!({
            "username": "ada",
            "email": "ada@example.test",
            "password": too_short,
        }))
        .await;

    response.assert_status(StatusCode::BAD_REQUEST);
    assert!(!response.text().contains(too_short));
}

#[tokio::test]
async fn the_username_is_trimmed_and_the_email_normalised() {
    let app = TestApp::spawn().await;

    let response = app
        .server
        .post(CREATE)
        .json(&json!({
            "username": "  ada  ",
            "email": "  Ada@Example.TEST  ",
            "password": PASSWORD,
        }))
        .await;

    response.assert_status(StatusCode::CREATED);
    let body = response.json::<Value>();
    assert_eq!(body["user"]["username"], json!("ada"));
    assert_eq!(body["user"]["email"], json!("ada@example.test"));
}

/// A missing field is the `Json` extractor's 400, not a user created with an
/// empty one.
#[tokio::test]
async fn a_body_that_is_missing_a_field_is_400() {
    let app = TestApp::spawn().await;

    let response = app
        .server
        .post(CREATE)
        .json(&json!({ "username": "ada", "password": PASSWORD }))
        .await;

    response.assert_status(StatusCode::BAD_REQUEST);
}

/// `create_gated_user` is the other half of the arrangement helpers: a user
/// the password-change gate closes the ordinary routes to.
#[tokio::test]
async fn the_gated_arrangement_helper_produces_a_gated_user() {
    let app = TestApp::spawn().await;
    let ada = app.create_gated_user("ada", "ada@example.test").await;

    assert!(ada.user.must_change_password);

    // The exempt route answers, the gated one does not (`SPEC.md`,
    // "Authentication").
    app.get_as(&ada, "/api/users/me")
        .await
        .assert_status(StatusCode::OK);

    let response = app
        .get_as(&ada, &format!("/api/users/{}", ada.user.id))
        .await;
    response.assert_status(StatusCode::FORBIDDEN);
    assert_eq!(
        response.json::<Value>(),
        json!({ "status": 403, "error": "password change required" })
    );
}

/// `POST /api/test/throttle/reset` (`SPEC.md`, "Test-only routes"): a client
/// address the login throttle has blocked signs in again after it, which is
/// what lets the Playwright suite run any number of times against one stack.
/// Its absence from a release build is the feature gate's, asserted in
/// `routes::tests`.
#[tokio::test]
async fn resetting_the_throttle_lifts_a_block_on_the_client_address() {
    let app = TestApp::spawn().await;
    app.server
        .post(CREATE)
        .json(&json!({
            "username": "ada",
            "email": "ada@example.test",
            "password": PASSWORD,
        }))
        .await
        .assert_status(StatusCode::CREATED);

    // Ten failures spread over ten other usernames block the address, as the
    // suite's deliberate failures do; documentation addresses only (RFC 5737).
    for nth in 0..10 {
        app.server
            .post(LOGIN)
            .add_header("X-Forwarded-For", CLIENT)
            .json(&json!({ "username": format!("user{nth}"), "password": "wrong-password-here" }))
            .await
            .assert_status(StatusCode::UNAUTHORIZED);
    }

    let ada = json!({ "username": "ada", "password": PASSWORD });
    app.server
        .post(LOGIN)
        .add_header("X-Forwarded-For", CLIENT)
        .json(&ada)
        .await
        .assert_status(StatusCode::TOO_MANY_REQUESTS);

    let response = app.server.post(THROTTLE_RESET).await;
    response.assert_status(StatusCode::NO_CONTENT);
    assert!(response.text().is_empty());

    app.server
        .post(LOGIN)
        .add_header("X-Forwarded-For", CLIENT)
        .json(&ada)
        .await
        .assert_status(StatusCode::OK);
}

/// Resetting a throttle that counts nothing is not an error: the suite calls it
/// at the start of every run, the first one included.
#[tokio::test]
async fn resetting_an_empty_throttle_is_204() {
    let app = TestApp::spawn().await;

    app.server
        .post(THROTTLE_RESET)
        .await
        .assert_status(StatusCode::NO_CONTENT);
}
