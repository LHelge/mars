//! `GET /api/auth/invite/{token}` and `POST /api/auth/accept-invite` through
//! the real router (`SPEC.md`, "Auth (`/api/auth`)").
//!
//! The invitee's half of the flow whose administrator half `users_invites.rs`
//! covers, and the only way a user who was not seeded comes into existence
//! (ADR 0013). Two things are asserted throughout:
//!
//! - *An invitation is spent exactly once.* Accepting twice, accepting an
//!   expired or revoked link, and looking one up after it has been accepted
//!   are all the same 400, and the user the first acceptance created is still
//!   there afterwards.
//! - *A rejected acceptance leaves the invitation open.* A username that is
//!   too short, a password that is too short and a username somebody else has
//!   taken all leave a link the invitee can simply use again — which is what
//!   makes the whole transaction, not just the insert, the unit of work.
//!
//! The lookup is also checked for what it does *not* answer: it is an
//! unauthenticated route, so the inviting administrator's id must not be in
//! its body.
//!
//! Every password here is obviously fake (`CLAUDE.md`, rule 3), and no test
//! prints a raw invitation token.
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use axum::http::StatusCode;
use chrono::{DateTime, TimeDelta, Utc};
use common::{AuthenticatedUser, TestApp};
use mars_orchestrator::auth::REFRESH_COOKIE;
use serde_json::{Value, json};
use uuid::Uuid;

/// The administrator route that creates the invitations under test.
const INVITES: &str = "/api/users/invites";

/// A route every authenticated user reaches.
const ME: &str = "/api/users/me";

/// A route only an administrator reaches.
const USERS: &str = "/api/users";

/// Obviously fake, and inside the documented 10–128 range.
const PASSWORD: &str = "correct-horse-battery-staple";

/// The three keys `SPEC.md` gives the lookup response, in sorted order.
const LOOKUP_KEYS: [&str; 3] = ["admin", "email", "expires_at"];

/// The documented 400 body of both invite routes.
fn invalid_invite() -> Value {
    json!({ "status": 400, "error": "invalid or expired invite" })
}

/// The documented 409 body for `message`.
fn conflict(message: &str) -> Value {
    json!({ "status": 409, "error": message })
}

/// An administrator to send invitations as.
async fn admin(app: &TestApp) -> AuthenticatedUser {
    app.create_admin("grace", "grace@example.test", PASSWORD)
        .await
}

/// Move an invitation's expiry into the past.
///
/// A row-level fact with no interface: the expiry lives in the row and no
/// route moves it backwards.
///
/// An unchecked query: this file introduces no compile-time checked query, so
/// `.sqlx/` never has to carry one for a test (`CLAUDE.md`, "Backend
/// conventions").
async fn expire(app: &TestApp, id: Uuid) {
    sqlx::query("UPDATE user_invites SET expires_at = $1 WHERE id = $2")
        .bind(Utc::now() - TimeDelta::minutes(1))
        .bind(id)
        .execute(&app.pool)
        .await
        .expect("the expiry is moved");
}

/// The `accepted_user_id` column of one invitation.
///
/// A row-level fact with no interface: no route answers a spent invitation, so
/// which user it created is only visible in the row.
async fn accepted_user_id(app: &TestApp, id: Uuid) -> Option<Uuid> {
    sqlx::query_scalar::<_, Option<Uuid>>("SELECT accepted_user_id FROM user_invites WHERE id = $1")
        .bind(id)
        .fetch_one(&app.pool)
        .await
        .expect("the invite row exists")
}

/// The id out of a `{ user, access_token }` body.
fn user_id(body: &Value) -> Uuid {
    Uuid::parse_str(body["user"]["id"].as_str().expect("id is a string")).expect("id is a UUID")
}

#[tokio::test]
async fn looking_up_an_open_invite_answers_the_three_documented_keys() {
    let app = TestApp::spawn().await;
    let caller = admin(&app).await;
    let invited = app.invite(&caller, "ada@example.test", false).await;

    let response = app.lookup_invite(&invited.token).await;

    response.assert_status_ok();
    let body = response.json::<Value>();

    let mut keys: Vec<&str> = body
        .as_object()
        .expect("the body is an object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    // Exactly these three: the id and `invited_by` would tell an
    // unauthenticated caller who sent the invitation.
    assert_eq!(keys, LOOKUP_KEYS);

    assert_eq!(body["email"], json!("ada@example.test"));
    assert_eq!(body["admin"], json!(false));

    // RFC 3339, seven days out, like every other timestamp (`SPEC.md`, "REST
    // API").
    let expires_at =
        DateTime::parse_from_rfc3339(body["expires_at"].as_str().expect("expires_at is a string"))
            .expect("expires_at is RFC 3339")
            .with_timezone(&Utc);
    let drift = expires_at - (Utc::now() + TimeDelta::days(7));
    assert!(
        drift.abs() < TimeDelta::minutes(1),
        "expiry is not seven days out"
    );

    // The lookup is about the invitation, not about the token.
    let rendered = body.to_string();
    assert!(!rendered.contains(&invited.token), "the token came back");
}

#[tokio::test]
async fn looking_up_an_invite_that_makes_an_administrator_says_so() {
    let app = TestApp::spawn().await;
    let caller = admin(&app).await;
    let invited = app.invite(&caller, "ada@example.test", true).await;

    let response = app.lookup_invite(&invited.token).await;

    response.assert_status_ok();
    assert_eq!(response.json::<Value>()["admin"], json!(true));
}

#[tokio::test]
async fn looking_up_an_unknown_token_is_a_bad_request() {
    let app = TestApp::spawn().await;

    // Well formed and never issued: two UUIDs joined by a dot is the shape of
    // a real token.
    let response = app
        .lookup_invite("00000000-0000-0000-0000-0000000000ff.00000000-0000-0000-0000-0000000000ee")
        .await;

    response.assert_status(StatusCode::BAD_REQUEST);
    assert_eq!(response.json::<Value>(), invalid_invite());
}

#[tokio::test]
async fn looking_up_an_expired_invite_is_a_bad_request() {
    let app = TestApp::spawn().await;
    let caller = admin(&app).await;
    let invited = app.invite(&caller, "ada@example.test", false).await;

    expire(&app, invited.id).await;

    let response = app.lookup_invite(&invited.token).await;
    response.assert_status(StatusCode::BAD_REQUEST);
    assert_eq!(response.json::<Value>(), invalid_invite());
}

#[tokio::test]
async fn looking_up_a_revoked_invite_is_a_bad_request() {
    let app = TestApp::spawn().await;
    let caller = admin(&app).await;
    let invited = app.invite(&caller, "ada@example.test", false).await;

    app.delete_as(&caller, &format!("{INVITES}/{}", invited.id))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    let response = app.lookup_invite(&invited.token).await;
    response.assert_status(StatusCode::BAD_REQUEST);
    assert_eq!(response.json::<Value>(), invalid_invite());
}

#[tokio::test]
async fn looking_up_an_accepted_invite_is_a_bad_request() {
    let app = TestApp::spawn().await;
    let caller = admin(&app).await;
    let invited = app.invite(&caller, "ada@example.test", false).await;

    app.accept_invite(&invited.token, "ada", PASSWORD)
        .await
        .assert_status(StatusCode::CREATED);

    let response = app.lookup_invite(&invited.token).await;
    response.assert_status(StatusCode::BAD_REQUEST);
    assert_eq!(response.json::<Value>(), invalid_invite());
}

#[tokio::test]
async fn accepting_an_invite_creates_the_user_and_signs_them_in() {
    let app = TestApp::spawn().await;
    let caller = admin(&app).await;
    let invited = app.invite(&caller, "ada@example.test", false).await;

    let response = app.accept_invite(&invited.token, "ada", PASSWORD).await;

    response.assert_status(StatusCode::CREATED);
    let cookie = response.cookie(REFRESH_COOKIE).value().to_string();
    let body = response.json::<Value>();

    assert_eq!(body["user"]["username"], json!("ada"));
    // The address comes from the invitation; the body never carried one.
    assert_eq!(body["user"]["email"], json!("ada@example.test"));
    assert_eq!(body["user"]["admin"], json!(false));
    // The invitee chose this password seconds ago (`docs/data-model.md`,
    // `users`).
    assert_eq!(body["user"]["must_change_password"], json!(false));
    assert_eq!(body["user"]["notify_email"], json!(true));
    // The `User` DTO, so the internal columns are absent (`SPEC.md`, "Users").
    assert_eq!(body["user"].get("password_hash"), None);
    assert_eq!(body["user"].get("auth_version"), None);
    // The refresh token travels in the cookie and nowhere else.
    assert!(body["access_token"].is_string());
    assert_eq!(body.get("refresh_token"), None);
    assert!(!cookie.is_empty(), "the refresh cookie is set");

    let token = body["access_token"].as_str().expect("a token").to_string();
    let me = app.server.get(ME).authorization_bearer(&token).await;
    me.assert_status_ok();
    assert_eq!(me.json::<Value>()["id"], body["user"]["id"]);

    // The cookie is a working one, like login's (`SPEC.md`,
    // "Authentication").
    app.refresh(&cookie).await.assert_status_ok();

    // The invitation is spent: gone from the administrator's list, and the row
    // names the user it created.
    let open = app.get_as(&caller, INVITES).await;
    open.assert_status_ok();
    assert_eq!(open.json::<Value>(), json!([]));
    assert_eq!(
        accepted_user_id(&app, invited.id).await,
        Some(user_id(&body))
    );
}

#[tokio::test]
async fn a_token_with_surrounding_whitespace_is_still_accepted() {
    let app = TestApp::spawn().await;
    let caller = admin(&app).await;
    let invited = app.invite(&caller, "ada@example.test", false).await;

    // A token pasted out of a mail client, newline and all, is the same
    // invitation.
    let padded = format!("  {}\n", invited.token);
    app.accept_invite(&padded, "ada", PASSWORD)
        .await
        .assert_status(StatusCode::CREATED);
}

#[tokio::test]
async fn accepting_the_same_invite_twice_is_a_bad_request() {
    let app = TestApp::spawn().await;
    let caller = admin(&app).await;
    let invited = app.invite(&caller, "ada@example.test", false).await;

    let first = app.accept_invite(&invited.token, "ada", PASSWORD).await;
    first.assert_status(StatusCode::CREATED);
    let first_body = first.json::<Value>();

    let second = app
        .accept_invite(&invited.token, "grace-two", PASSWORD)
        .await;
    second.assert_status(StatusCode::BAD_REQUEST);
    assert_eq!(second.json::<Value>(), invalid_invite());

    // The refused second acceptance changed nothing about the first user.
    let token = first_body["access_token"].as_str().expect("a token");
    let me = app.server.get(ME).authorization_bearer(token).await;
    me.assert_status_ok();
    assert_eq!(me.json::<Value>()["username"], json!("ada"));
    assert_eq!(
        accepted_user_id(&app, invited.id).await,
        Some(user_id(&first_body))
    );
}

#[tokio::test]
async fn accepting_an_expired_invite_is_a_bad_request() {
    let app = TestApp::spawn().await;
    let caller = admin(&app).await;
    let invited = app.invite(&caller, "ada@example.test", false).await;

    expire(&app, invited.id).await;

    let response = app.accept_invite(&invited.token, "ada", PASSWORD).await;
    response.assert_status(StatusCode::BAD_REQUEST);
    assert_eq!(response.json::<Value>(), invalid_invite());
    assert_eq!(accepted_user_id(&app, invited.id).await, None);
}

#[tokio::test]
async fn a_username_that_is_too_short_is_rejected_and_the_invite_stays_open() {
    let app = TestApp::spawn().await;
    let caller = admin(&app).await;
    let invited = app.invite(&caller, "ada@example.test", false).await;

    let response = app.accept_invite(&invited.token, "ad", PASSWORD).await;

    response.assert_status(StatusCode::BAD_REQUEST);
    assert_eq!(
        response.json::<Value>(),
        json!({ "status": 400, "error": "username must be 3-32 characters" })
    );

    // Validated before the transaction opened, so the link is untouched.
    app.lookup_invite(&invited.token).await.assert_status_ok();
    assert_eq!(accepted_user_id(&app, invited.id).await, None);
}

#[tokio::test]
async fn a_password_that_is_too_short_is_rejected_and_the_invite_stays_open() {
    let app = TestApp::spawn().await;
    let caller = admin(&app).await;
    let invited = app.invite(&caller, "ada@example.test", false).await;

    let response = app.accept_invite(&invited.token, "ada", "too-short").await;

    response.assert_status(StatusCode::BAD_REQUEST);
    assert_eq!(
        response.json::<Value>(),
        json!({ "status": 400, "error": "password must be 10-128 characters" })
    );

    app.lookup_invite(&invited.token).await.assert_status_ok();
    assert_eq!(accepted_user_id(&app, invited.id).await, None);
}

#[tokio::test]
async fn a_username_somebody_else_has_is_a_conflict_and_the_invite_stays_open() {
    let app = TestApp::spawn().await;
    let caller = admin(&app).await;
    app.create_user("ada", "other@example.test", PASSWORD).await;
    let invited = app.invite(&caller, "ada@example.test", false).await;

    let response = app.accept_invite(&invited.token, "ada", PASSWORD).await;

    response.assert_status(StatusCode::CONFLICT);
    assert_eq!(response.json::<Value>(), conflict("username already taken"));

    // The whole acceptance rolled back, so the invitee can pick another name.
    app.lookup_invite(&invited.token).await.assert_status_ok();
    assert_eq!(accepted_user_id(&app, invited.id).await, None);

    app.accept_invite(&invited.token, "ada-lovelace", PASSWORD)
        .await
        .assert_status(StatusCode::CREATED);
}

#[tokio::test]
async fn an_address_that_has_become_a_user_is_a_conflict_and_the_invite_stays_open() {
    let app = TestApp::spawn().await;
    let caller = admin(&app).await;
    let invited = app.invite(&caller, "ada@example.test", false).await;

    // The address becomes a user by another path while the invitation is open
    // — here the test route, in production an administrator's own seeding.
    app.create_user("ada-elsewhere", "ada@example.test", PASSWORD)
        .await;

    let response = app.accept_invite(&invited.token, "ada", PASSWORD).await;

    response.assert_status(StatusCode::CONFLICT);
    assert_eq!(
        response.json::<Value>(),
        conflict("email already registered")
    );

    // Still open, so an administrator can revoke a link that can no longer be
    // accepted.
    app.lookup_invite(&invited.token).await.assert_status_ok();
    assert_eq!(accepted_user_id(&app, invited.id).await, None);
}

#[tokio::test]
async fn an_administrator_invite_creates_an_administrator() {
    let app = TestApp::spawn().await;
    let caller = admin(&app).await;
    let invited = app.invite(&caller, "ada@example.test", true).await;

    let response = app.accept_invite(&invited.token, "ada", PASSWORD).await;
    response.assert_status(StatusCode::CREATED);
    let body = response.json::<Value>();
    assert_eq!(body["user"]["admin"], json!(true));

    // The `admin` flag comes from the invitation, and it works immediately.
    let token = body["access_token"].as_str().expect("a token");
    app.server
        .get(USERS)
        .authorization_bearer(token)
        .await
        .assert_status_ok();
}

#[tokio::test]
async fn an_ordinary_invite_creates_a_user_who_is_not_an_administrator() {
    let app = TestApp::spawn().await;
    let caller = admin(&app).await;
    let invited = app.invite(&caller, "ada@example.test", false).await;

    let response = app.accept_invite(&invited.token, "ada", PASSWORD).await;
    response.assert_status(StatusCode::CREATED);
    let body = response.json::<Value>();
    assert_eq!(body["user"]["admin"], json!(false));

    let token = body["access_token"].as_str().expect("a token");
    let forbidden = app.server.get(USERS).authorization_bearer(token).await;
    forbidden.assert_status(StatusCode::FORBIDDEN);
    assert_eq!(
        forbidden.json::<Value>(),
        json!({ "status": 403, "error": "admin required" })
    );
}

#[tokio::test]
async fn the_accepted_user_can_log_in_with_the_password_they_chose() {
    let app = TestApp::spawn().await;
    let caller = admin(&app).await;
    let invited = app.invite(&caller, "ada@example.test", false).await;

    app.accept_invite(&invited.token, "ada", PASSWORD)
        .await
        .assert_status(StatusCode::CREATED);

    // The hash the acceptance stored is a real one: `TestApp::login` asserts
    // 200, and a user whose password did not round-trip would fail here rather
    // than at some later epic's test.
    let (pair, _cookie) = app.login("ada", PASSWORD).await;
    assert_eq!(pair.user["username"], json!("ada"));
}

/// `SPEC.md`, "REST API": every error is `{ "status", "error" }`, extractor
/// rejections included. The invite token is a string rather than a UUID, so
/// the one way its path segment fails to parse is a percent-encoding that is
/// not UTF-8; that answers 400 in the documented shape through the prelude's
/// `Path` wrapper rather than axum's plain-text rejection.
#[tokio::test]
async fn a_path_segment_that_is_not_utf8_is_400_in_the_documented_shape() {
    let app = TestApp::spawn().await;

    let response = app.lookup_invite("%FF%FE").await;

    response.assert_status(StatusCode::BAD_REQUEST);
    let body = response.json::<Value>();
    assert_eq!(body["status"], json!(400), "unexpected body: {body}");
    assert!(
        body["error"].as_str().is_some_and(|text| !text.is_empty()),
        "the rejection carried no message: {body}"
    );
}
