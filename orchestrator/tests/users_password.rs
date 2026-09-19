//! `POST /api/users/{id}/password` through the real router (`SPEC.md`, "Users
//! (`/api/users`)"; "Authentication").
//!
//! One path, two flows. What is asserted here is that each of them answers
//! what `SPEC.md` says and that both of them are the atomic mutation ADR 0025
//! requires: the hash, `auth_version`, `must_change_password` and every
//! refresh token move together. So the revocation tests present the *old*
//! credentials afterwards rather than trusting the status code, and the
//! rejection tests present them too — a refused change must revoke nothing.
//!
//! The administrator flow is also asserted from the other side: the acting
//! administrator's own cookie still refreshes afterwards, because changing
//! somebody else's password is not a change to yours.
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
use mars_orchestrator::auth::REFRESH_COOKIE;
use mars_orchestrator::models::User;
use mars_orchestrator::prelude::*;
use mars_orchestrator::repositories::UserRepository;
use serde_json::{Value, json};
use uuid::Uuid;

/// The self routes; also the gated route this file probes the gate with.
const ME: &str = "/api/users/me";

/// Obviously fake, and long enough for the documented 10–128 range.
const PASSWORD: &str = "correct-horse-battery-staple";

/// The replacement, equally fake and equally long enough.
const NEW_PASSWORD: &str = "a-completely-different-phrase";

/// The route under test, for `id`.
fn password_route(id: Uuid) -> String {
    format!("/api/users/{id}/password")
}

/// The documented 400 body for `message`.
fn bad_request(message: &str) -> Value {
    json!({ "status": 400, "error": message })
}

/// The row as the database has it now, whatever a response claimed.
async fn stored(app: &TestApp, id: Uuid) -> User {
    UserRepository::new(&app.pool)
        .find(id)
        .await
        .expect("the lookup succeeds")
        .expect("the user exists")
}

#[tokio::test]
async fn a_self_service_change_returns_a_working_pair_and_revokes_the_old_one() {
    let app = TestApp::spawn().await;
    let user = app
        .insert_user_with_password("ada", "ada@example.test", PASSWORD, false, false)
        .await;
    let (old_pair, old_cookie) = app.login("ada", PASSWORD).await;

    let response = app
        .server
        .post(&password_route(user.id))
        .authorization_bearer(&old_pair.access_token)
        .json(&json!({ "current_password": PASSWORD, "password": NEW_PASSWORD }))
        .await;

    response.assert_status(StatusCode::OK);
    let body = response.json::<Value>();
    assert_eq!(body["user"]["id"], json!(user.id.to_string()));
    assert_eq!(body["user"]["must_change_password"], json!(false));
    assert_eq!(body["user"].get("password_hash"), None);

    // The access token is minted from the committed row, so it carries the
    // incremented generation and outlives the mutation that revoked the old.
    let claims = Claims::decode(
        body["access_token"].as_str().expect("an access token"),
        &app.state.config,
    )
    .expect("the token verifies");
    assert_eq!(claims.sub, user.id);
    assert_eq!(claims.auth_version, user.auth_version + 1);
    assert!(!claims.must_change_password);

    // The replacement cookie is set, carries a token that is not the old one,
    // and never appears in the body.
    let new_cookie = response.cookie(REFRESH_COOKIE).value().to_string();
    assert!(!new_cookie.is_empty());
    assert_ne!(new_cookie, old_cookie);
    assert!(
        !response.text().contains(&new_cookie),
        "the refresh token must travel only in the cookie"
    );

    // This browser stays signed in: the new pair works on both halves.
    app.server
        .get(ME)
        .authorization_bearer(body["access_token"].as_str().unwrap())
        .await
        .assert_status(StatusCode::OK);
    app.refresh(&new_cookie).await.assert_status(StatusCode::OK);

    // Everything the old pair could do, it can no longer do.
    app.server
        .get(ME)
        .authorization_bearer(&old_pair.access_token)
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    app.refresh(&old_cookie)
        .await
        .assert_status(StatusCode::UNAUTHORIZED);

    // And the password itself changed: the old one no longer logs in, the new
    // one does.
    app.server
        .post("/api/auth/login")
        .json(&json!({ "username": "ada", "password": PASSWORD }))
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    app.login("ada", NEW_PASSWORD).await;
}

/// Exactly one refresh token survives the change — the replacement inserted
/// inside the same transaction (`SPEC.md`, "Authentication").
#[tokio::test]
async fn the_replacement_is_the_only_usable_refresh_token_left() {
    let app = TestApp::spawn().await;
    let user = app
        .insert_user_with_password("ada", "ada@example.test", PASSWORD, false, false)
        .await;

    // Two browsers signed in before the change; both should lose their token.
    let (pair, first) = app.login("ada", PASSWORD).await;
    let (_, second) = app.login("ada", PASSWORD).await;

    let response = app
        .server
        .post(&password_route(user.id))
        .authorization_bearer(&pair.access_token)
        .json(&json!({ "current_password": PASSWORD, "password": NEW_PASSWORD }))
        .await;
    response.assert_status(StatusCode::OK);
    let replacement = response.cookie(REFRESH_COOKIE).value().to_string();

    let live: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM refresh_tokens WHERE revoked_at IS NULL")
            .fetch_one(&app.pool)
            .await
            .expect("the count succeeds");
    assert_eq!(live, 1);

    for old in [first, second] {
        app.refresh(&old)
            .await
            .assert_status(StatusCode::UNAUTHORIZED);
    }
    app.refresh(&replacement)
        .await
        .assert_status(StatusCode::OK);
}

#[tokio::test]
async fn a_wrong_current_password_is_400_and_revokes_nothing() {
    let app = TestApp::spawn().await;
    let user = app
        .insert_user_with_password("ada", "ada@example.test", PASSWORD, false, false)
        .await;
    let (pair, cookie) = app.login("ada", PASSWORD).await;

    let response = app
        .server
        .post(&password_route(user.id))
        .authorization_bearer(&pair.access_token)
        .json(&json!({ "current_password": "not-the-old-password", "password": NEW_PASSWORD }))
        .await;

    response.assert_status(StatusCode::BAD_REQUEST);
    assert_eq!(
        response.json::<Value>(),
        bad_request("current password is incorrect")
    );
    assert_eq!(response.maybe_cookie(REFRESH_COOKIE), None);

    // Nothing moved: the row, the tokens and the old password all still stand.
    let row = stored(&app, user.id).await;
    assert_eq!(row.auth_version, user.auth_version);
    assert_eq!(row.password_hash, user.password_hash);
    app.refresh(&cookie).await.assert_status(StatusCode::OK);
    app.server
        .get(ME)
        .authorization_bearer(&pair.access_token)
        .await
        .assert_status(StatusCode::OK);
}

#[tokio::test]
async fn a_missing_current_password_is_400() {
    let app = TestApp::spawn().await;
    let user = app
        .insert_user_with_password("ada", "ada@example.test", PASSWORD, false, false)
        .await;
    let token = app.token_for(&user);

    let response = app
        .server
        .post(&password_route(user.id))
        .authorization_bearer(&token)
        .json(&json!({ "password": NEW_PASSWORD }))
        .await;

    response.assert_status(StatusCode::BAD_REQUEST);
    assert_eq!(
        response.json::<Value>(),
        bad_request("current password required")
    );
    assert_eq!(stored(&app, user.id).await.auth_version, user.auth_version);
}

/// The 10–128 rule, from `models::Password` (`SPEC.md`, "User-facing
/// features").
#[tokio::test]
async fn a_password_outside_the_documented_range_is_400() {
    let app = TestApp::spawn().await;
    let user = app
        .insert_user_with_password("ada", "ada@example.test", PASSWORD, false, false)
        .await;
    let (pair, _cookie) = app.login("ada", PASSWORD).await;

    let too_long = "p".repeat(129);
    for candidate in ["too-short", too_long.as_str()] {
        let response = app
            .server
            .post(&password_route(user.id))
            .authorization_bearer(&pair.access_token)
            .json(&json!({ "current_password": PASSWORD, "password": candidate }))
            .await;

        response.assert_status(StatusCode::BAD_REQUEST);
        assert_eq!(
            response.json::<Value>(),
            bad_request("password must be 10-128 characters")
        );
    }

    assert_eq!(stored(&app, user.id).await.auth_version, user.auth_version);
}

/// The primary first-login path: a gated user clears their own flag here and
/// nowhere else (`README.md`, "Start"; ADR 0024).
#[tokio::test]
async fn a_gated_user_completes_their_first_login_through_this_route() {
    let app = TestApp::spawn().await;
    let user = app
        .insert_user_with_password("admin", "admin@example.test", PASSWORD, true, true)
        .await;
    let (pair, _cookie) = app.login("admin", PASSWORD).await;

    // Before: the gate refuses an ordinary authenticated route.
    let refused = app
        .server
        .patch(ME)
        .authorization_bearer(&pair.access_token)
        .json(&json!({}))
        .await;
    refused.assert_status(StatusCode::FORBIDDEN);
    assert_eq!(
        refused.json::<Value>(),
        json!({ "status": 403, "error": "password change required" })
    );

    let response = app
        .server
        .post(&password_route(user.id))
        .authorization_bearer(&pair.access_token)
        .json(&json!({ "current_password": PASSWORD, "password": NEW_PASSWORD }))
        .await;

    response.assert_status(StatusCode::OK);
    let body = response.json::<Value>();
    assert_eq!(body["user"]["must_change_password"], json!(false));
    assert!(!stored(&app, user.id).await.must_change_password);

    // After: the pair the change handed back passes the gate.
    app.server
        .patch(ME)
        .authorization_bearer(body["access_token"].as_str().expect("an access token"))
        .json(&json!({}))
        .await
        .assert_status(StatusCode::OK);
}

#[tokio::test]
async fn an_ordinary_user_may_not_change_somebody_elses_password() {
    let app = TestApp::spawn().await;
    let caller = app
        .insert_user("ada", "ada@example.test", false, false)
        .await;
    let target = app
        .insert_user_with_password("grace", "grace@example.test", PASSWORD, false, false)
        .await;
    let token = app.token_for(&caller);

    let response = app
        .server
        .post(&password_route(target.id))
        .authorization_bearer(&token)
        .json(&json!({ "password": NEW_PASSWORD }))
        .await;

    response.assert_status(StatusCode::FORBIDDEN);
    assert_eq!(
        response.json::<Value>(),
        json!({ "status": 403, "error": "admin required" })
    );
    assert_eq!(
        stored(&app, target.id).await.auth_version,
        target.auth_version
    );

    // An unknown id gets the same 403, not a 404: whether a user exists is not
    // something a non-administrator learns here.
    app.server
        .post(&password_route(Uuid::new_v4()))
        .authorization_bearer(&token)
        .json(&json!({ "password": NEW_PASSWORD }))
        .await
        .assert_status(StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn an_administrator_changing_another_password_gets_204_and_keeps_their_own_login() {
    let app = TestApp::spawn().await;
    let admin = app
        .insert_user_with_password("grace", "grace@example.test", PASSWORD, true, false)
        .await;
    let target = app
        .insert_user_with_password("ada", "ada@example.test", PASSWORD, false, true)
        .await;

    let (admin_pair, admin_cookie) = app.login("grace", PASSWORD).await;
    let (target_pair, target_cookie) = app.login("ada", PASSWORD).await;

    let response = app
        .server
        .post(&password_route(target.id))
        .authorization_bearer(&admin_pair.access_token)
        // Ignored, not validated: the administrator does not know it.
        .json(&json!({ "current_password": "whatever-this-is", "password": NEW_PASSWORD }))
        .await;

    response.assert_status(StatusCode::NO_CONTENT);
    assert!(response.text().is_empty());
    assert_eq!(response.headers().get(SET_COOKIE), None);

    // The target: revoked, re-hashed, and no longer gated.
    let row = stored(&app, target.id).await;
    assert_eq!(row.auth_version, target.auth_version + 1);
    assert!(!row.must_change_password);
    app.refresh(&target_cookie)
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    app.server
        .get(ME)
        .authorization_bearer(&target_pair.access_token)
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    app.login("ada", NEW_PASSWORD).await;

    // The acting administrator: untouched.
    assert_eq!(
        stored(&app, admin.id).await.auth_version,
        admin.auth_version
    );
    app.refresh(&admin_cookie)
        .await
        .assert_status(StatusCode::OK);
    app.server
        .get(ME)
        .authorization_bearer(&admin_pair.access_token)
        .await
        .assert_status(StatusCode::OK);
}

#[tokio::test]
async fn an_administrator_aiming_at_an_unknown_id_is_404() {
    let app = TestApp::spawn().await;
    let admin = app
        .insert_user("grace", "grace@example.test", true, false)
        .await;
    let token = app.token_for(&admin);

    let response = app
        .server
        .post(&password_route(Uuid::new_v4()))
        .authorization_bearer(&token)
        .json(&json!({ "password": NEW_PASSWORD }))
        .await;

    response.assert_status(StatusCode::NOT_FOUND);
    assert_eq!(
        response.json::<Value>(),
        json!({ "status": 404, "error": "not found" })
    );
}

/// The gate exempts the route without qualification (`SPEC.md`,
/// "Authentication"), so an administrator who has to change their own password
/// can still set somebody else's.
#[tokio::test]
async fn a_gated_administrator_may_still_change_another_password() {
    let app = TestApp::spawn().await;
    let admin = app
        .insert_user("grace", "grace@example.test", true, true)
        .await;
    let target = app
        .insert_user_with_password("ada", "ada@example.test", PASSWORD, false, false)
        .await;
    let token = app.token_for(&admin);

    app.server
        .post(&password_route(target.id))
        .authorization_bearer(&token)
        .json(&json!({ "password": NEW_PASSWORD }))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    assert_eq!(
        stored(&app, target.id).await.auth_version,
        target.auth_version + 1
    );
    // The administrator's own flag is still set: they changed somebody else's
    // password, not their own.
    assert!(stored(&app, admin.id).await.must_change_password);
}

#[tokio::test]
async fn the_route_needs_authentication() {
    let app = TestApp::spawn().await;
    let user = app
        .insert_user("ada", "ada@example.test", false, false)
        .await;

    let response = app
        .server
        .post(&password_route(user.id))
        .json(&json!({ "current_password": PASSWORD, "password": NEW_PASSWORD }))
        .await;

    response.assert_status(StatusCode::UNAUTHORIZED);
    assert_eq!(
        response.json::<Value>(),
        json!({ "status": 401, "error": "authentication required" })
    );
}

/// No history rule in v1: setting the same password again is an ordinary
/// change, which still rotates the credentials.
#[tokio::test]
async fn the_new_password_may_be_the_old_one() {
    let app = TestApp::spawn().await;
    let user = app
        .insert_user_with_password("ada", "ada@example.test", PASSWORD, false, false)
        .await;
    let (pair, cookie) = app.login("ada", PASSWORD).await;

    app.server
        .post(&password_route(user.id))
        .authorization_bearer(&pair.access_token)
        .json(&json!({ "current_password": PASSWORD, "password": PASSWORD }))
        .await
        .assert_status(StatusCode::OK);

    let row = stored(&app, user.id).await;
    assert_eq!(row.auth_version, user.auth_version + 1);
    // A fresh salt, so the stored hash differs even though the password does
    // not.
    assert_ne!(row.password_hash, user.password_hash);
    app.refresh(&cookie)
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    app.login("ada", PASSWORD).await;
}

/// A token minted before the change is refused afterwards even though it has
/// not expired: `auth_version` is what makes revocation immediate (ADR 0025).
#[tokio::test]
async fn an_access_token_from_before_the_change_is_401() {
    let app = TestApp::spawn().await;
    let user = app
        .insert_user_with_password("ada", "ada@example.test", PASSWORD, false, false)
        .await;
    let stale = app.token_for(&user);
    let claims = Claims::decode(&stale, &app.state.config).expect("the token verifies");
    assert!(claims.exp > Utc::now().timestamp());

    app.server
        .post(&password_route(user.id))
        .authorization_bearer(&stale)
        .json(&json!({ "current_password": PASSWORD, "password": NEW_PASSWORD }))
        .await
        .assert_status(StatusCode::OK);

    app.server
        .get(ME)
        .authorization_bearer(&stale)
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}
