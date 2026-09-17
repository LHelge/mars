//! The four administrator invite routes through the real router (`SPEC.md`,
//! "Users (`/api/users`)").
//!
//! Every test here is really about one sentence: "the token is delivered in
//! the email ... and is never included in the `Invite` response". So the
//! assertions run in both directions — the response is checked for the exact
//! six documented keys and for the absence of anything token-shaped, and the
//! captured message is checked for a link whose token actually hashes to the
//! row the route stored. A route that answered the token, or stored it raw,
//! would fail one or the other.
//!
//! The other half is what "open" means: unaccepted *and* unexpired. An
//! expired invite is invisible to the list and is silently replaced by a new
//! one for the same address, while an accepted invite is neither replaced nor
//! revocable and is a 404 to every route that names it by id.
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use axum::http::StatusCode;
use chrono::{DateTime, TimeDelta, Utc};
use common::TestApp;
use mars_orchestrator::email::EmailMessage;
use mars_orchestrator::models::{OpaqueToken, User, UserInvite};
use mars_orchestrator::repositories::UserInviteRepository;
use serde_json::{Value, json};
use uuid::Uuid;

/// The invite collection.
const INVITES: &str = "/api/users/invites";

/// The six keys `SPEC.md` gives the `Invite` DTO, in sorted order.
const INVITE_KEYS: [&str; 6] = [
    "admin",
    "created_at",
    "email",
    "expires_at",
    "id",
    "invited_by",
];

/// The `PUBLIC_URL` the test harness configures, which is what the link in the
/// email is built on.
const PUBLIC_URL: &str = "http://localhost";

/// The documented 401 body (`SPEC.md`, "Authentication").
fn unauthorized() -> Value {
    json!({ "status": 401, "error": "authentication required" })
}

/// The documented 403 body of an administrator-only route.
fn admin_required() -> Value {
    json!({ "status": 403, "error": "admin required" })
}

/// The documented 409 body for `message`.
fn conflict(message: &str) -> Value {
    json!({ "status": 409, "error": message })
}

/// An administrator to send invites as.
async fn admin(app: &TestApp) -> (User, String) {
    let user = app
        .insert_user("grace", "grace@example.test", true, false)
        .await;
    let token = app.token_for(&user);
    (user, token)
}

/// `POST /api/users/invites` as `token`.
async fn create(app: &TestApp, token: &str, body: Value) -> axum_test::TestResponse {
    app.server
        .post(INVITES)
        .authorization_bearer(token)
        .json(&body)
        .await
}

/// Create an invite that is expected to succeed, and answer the parsed body.
async fn create_ok(app: &TestApp, token: &str, email: &str) -> Value {
    let response = create(app, token, json!({ "email": email })).await;
    response.assert_status(StatusCode::CREATED);
    response.json::<Value>()
}

/// The `token_hash` column of one invite, whatever the response said.
///
/// An unchecked query: this file introduces no compile-time checked query, so
/// `.sqlx/` never has to carry one for a test (`CLAUDE.md`, "Backend
/// conventions").
async fn stored_hash(app: &TestApp, id: Uuid) -> String {
    sqlx::query_scalar::<_, String>("SELECT token_hash FROM user_invites WHERE id = $1")
        .bind(id)
        .fetch_one(&app.pool)
        .await
        .expect("the invite row exists")
}

/// Move an invite's expiry, to arrange an invite that has lapsed.
async fn set_expiry(app: &TestApp, id: Uuid, expires_at: DateTime<Utc>) {
    sqlx::query("UPDATE user_invites SET expires_at = $1 WHERE id = $2")
        .bind(expires_at)
        .bind(id)
        .execute(&app.pool)
        .await
        .expect("the expiry is moved");
}

/// Mark an invite accepted without going through the acceptance route, which
/// is a later task.
async fn mark_accepted(app: &TestApp, id: Uuid, user_id: Uuid) {
    let mut tx = app.pool.begin().await.expect("a transaction begins");
    let accepted = UserInviteRepository::new(&app.pool)
        .mark_accepted(&mut tx, id, user_id)
        .await
        .expect("the update runs");
    tx.commit().await.expect("the transaction commits");
    assert!(accepted, "the invite was open before this");
}

/// The single message the mock captured, with the capture cleared afterwards.
fn only_message(app: &TestApp) -> EmailMessage {
    let sent = app.mock_email().sent();
    assert_eq!(sent.len(), 1, "expected exactly one message: {sent:?}");
    app.mock_email().clear();
    sent.into_iter().next().expect("one message")
}

/// The raw token out of the `<PUBLIC_URL>/invite/<token>` link in `message`.
fn token_in(message: &EmailMessage) -> String {
    let prefix = format!("{PUBLIC_URL}/invite/");
    let start = message
        .text
        .find(&prefix)
        .unwrap_or_else(|| panic!("no invite link in the message: {}", message.text))
        + prefix.len();

    message.text[start..]
        .split_whitespace()
        .next()
        .expect("the link has a token")
        .to_string()
}

/// The id of an invite body.
fn id_of(invite: &Value) -> Uuid {
    Uuid::parse_str(invite["id"].as_str().expect("id is a string")).expect("id is a UUID")
}

/// The `expires_at` of an invite body.
fn expiry_of(invite: &Value) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(
        invite["expires_at"]
            .as_str()
            .expect("expires_at is a string"),
    )
    .expect("expires_at is RFC 3339")
    .with_timezone(&Utc)
}

#[tokio::test]
async fn creating_an_invite_answers_the_documented_shape_and_nothing_else() {
    let app = TestApp::spawn().await;
    let (caller, token) = admin(&app).await;

    let response = create(&app, &token, json!({ "email": "  Ada@Example.TEST " })).await;

    response.assert_status(StatusCode::CREATED);
    let body = response.json::<Value>();

    let mut keys: Vec<&str> = body
        .as_object()
        .expect("the body is an object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, INVITE_KEYS);

    // Normalised on the way in, so the two unique indexes see what the caller
    // meant (`docs/data-model.md`, `users`).
    assert_eq!(body["email"], json!("ada@example.test"));
    assert_eq!(body["admin"], json!(false), "admin defaults to false");
    assert_eq!(body["invited_by"], json!(caller.id.to_string()));

    // Seven days, give or take the time the request took.
    let expected = Utc::now() + TimeDelta::days(7);
    let drift = expiry_of(&body) - expected;
    assert!(
        drift.abs() < TimeDelta::minutes(1),
        "expiry is not seven days out: {}",
        body["expires_at"]
    );

    // No field carries the token or its hash, under any name.
    let hash = stored_hash(&app, id_of(&body)).await;
    let rendered = body.to_string();
    assert!(!rendered.contains(&hash), "the hash leaked: {rendered}");
    let raw = token_in(&only_message(&app));
    assert!(!rendered.contains(&raw), "the token leaked: {rendered}");
}

#[tokio::test]
async fn the_emailed_link_carries_the_token_the_row_stores_the_hash_of() {
    let app = TestApp::spawn().await;
    let (_caller, token) = admin(&app).await;

    let body = create_ok(&app, &token, "ada@example.test").await;

    let sent = app.mock_email().sent();
    assert_eq!(sent.len(), 1, "one invitation goes out");
    let message = &sent[0];
    assert_eq!(message.to, "ada@example.test");
    assert_eq!(message.subject, "You have been invited to Mars");

    let raw = token_in(message);
    assert!(
        message.text.contains(&format!("{PUBLIC_URL}/invite/{raw}")),
        "the link is not <PUBLIC_URL>/invite/<token>: {}",
        message.text
    );
    assert_eq!(
        OpaqueToken::hash_of(&raw),
        stored_hash(&app, id_of(&body)).await,
        "the row does not store the hash of the token in the link"
    );

    // And the token resolves, which is what the acceptance route will do.
    let invite = UserInviteRepository::new(&app.pool)
        .find_open_by_hash(&OpaqueToken::hash_of(&raw))
        .await
        .expect("the lookup succeeds")
        .expect("the token resolves");
    assert_eq!(invite.id, id_of(&body));
}

#[tokio::test]
async fn an_invite_may_be_created_for_an_administrator() {
    let app = TestApp::spawn().await;
    let (_caller, token) = admin(&app).await;

    let response = create(
        &app,
        &token,
        json!({ "email": "ada@example.test", "admin": true }),
    )
    .await;

    response.assert_status(StatusCode::CREATED);
    assert_eq!(response.json::<Value>()["admin"], json!(true));

    let stored: Vec<UserInvite> = UserInviteRepository::new(&app.pool)
        .list_open()
        .await
        .expect("the list succeeds");
    assert!(stored[0].admin, "the flag is stored, not only answered");
}

#[tokio::test]
async fn a_second_open_invite_for_the_same_address_is_409() {
    let app = TestApp::spawn().await;
    let (_caller, token) = admin(&app).await;
    create_ok(&app, &token, "ada@example.test").await;
    app.mock_email().clear();

    // Including under a different spelling, because the address is normalised.
    let response = create(&app, &token, json!({ "email": "ADA@example.test" })).await;

    response.assert_status(StatusCode::CONFLICT);
    assert_eq!(
        response.json::<Value>(),
        conflict("an open invite already exists for this email")
    );
    assert!(
        app.mock_email().sent().is_empty(),
        "a refused invite sends nothing"
    );
}

#[tokio::test]
async fn inviting_an_address_that_already_belongs_to_a_user_is_409() {
    let app = TestApp::spawn().await;
    let (caller, token) = admin(&app).await;
    app.insert_user("ada", "ada@example.test", false, false)
        .await;

    for address in ["ada@example.test", &caller.email] {
        let response = create(&app, &token, json!({ "email": address })).await;

        response.assert_status(StatusCode::CONFLICT);
        assert_eq!(
            response.json::<Value>(),
            conflict("email already belongs to a user"),
            "address {address}"
        );
    }

    assert!(app.mock_email().sent().is_empty());
}

#[tokio::test]
async fn an_expired_invite_is_replaced_rather_than_refused() {
    let app = TestApp::spawn().await;
    let (_caller, token) = admin(&app).await;
    let first = create_ok(&app, &token, "ada@example.test").await;
    let first_raw = token_in(&only_message(&app));

    // The partial unique index is blind to `expires_at`, so without the
    // reaping step in the route this would collide with the dead row.
    set_expiry(&app, id_of(&first), Utc::now() - TimeDelta::days(1)).await;

    let second = create_ok(&app, &token, "ada@example.test").await;

    assert_ne!(id_of(&second), id_of(&first), "a new invite was created");
    assert!(expiry_of(&second) > Utc::now());

    let invites = UserInviteRepository::new(&app.pool);
    assert!(
        invites
            .find_open_by_hash(&OpaqueToken::hash_of(&first_raw))
            .await
            .expect("the lookup succeeds")
            .is_none(),
        "the expired link still resolves"
    );
    assert_eq!(
        invites
            .find_open_by_id(id_of(&first))
            .await
            .expect("the lookup succeeds"),
        None,
        "the expired row was not deleted"
    );
    let second_raw = token_in(&only_message(&app));
    assert_ne!(second_raw, first_raw);
}

#[tokio::test]
async fn an_address_that_is_not_an_address_is_400() {
    let app = TestApp::spawn().await;
    let (_caller, token) = admin(&app).await;

    let too_long = format!("{}@example.test", "a".repeat(250));
    for address in ["", "   ", "ada", "ada@", "@example.test", &too_long] {
        let response = create(&app, &token, json!({ "email": address })).await;

        response.assert_status(StatusCode::BAD_REQUEST);
        assert_eq!(response.json::<Value>()["status"], json!(400), "{address}");
    }

    // And a body that is not the documented shape at all.
    for body in [
        json!({}),
        json!({ "email": "ada@example.test", "username": "ada" }),
        json!({ "email": "ada@example.test", "admin": "yes" }),
    ] {
        let response = create(&app, &token, body.clone()).await;
        response.assert_status(StatusCode::BAD_REQUEST);
    }

    assert!(app.mock_email().sent().is_empty());
}

#[tokio::test]
async fn listing_invites_shows_the_open_ones_newest_first() {
    let app = TestApp::spawn().await;
    let (_caller, token) = admin(&app).await;

    let older = create_ok(&app, &token, "ada@example.test").await;
    let expired = create_ok(&app, &token, "alan@example.test").await;
    let accepted = create_ok(&app, &token, "grete@example.test").await;
    let newest = create_ok(&app, &token, "edsger@example.test").await;
    app.mock_email().clear();

    set_expiry(&app, id_of(&expired), Utc::now() - TimeDelta::hours(1)).await;
    let user = app
        .insert_user("grete", "grete@example.test", false, false)
        .await;
    mark_accepted(&app, id_of(&accepted), user.id).await;

    let response = app.server.get(INVITES).authorization_bearer(&token).await;

    response.assert_status(StatusCode::OK);
    let body = response.json::<Value>();
    let listed = body.as_array().expect("an array");

    let emails: Vec<&str> = listed
        .iter()
        .map(|invite| invite["email"].as_str().expect("email is a string"))
        .collect();
    assert_eq!(emails, vec!["edsger@example.test", "ada@example.test"]);
    assert_eq!(id_of(&listed[0]), id_of(&newest));
    assert_eq!(id_of(&listed[1]), id_of(&older));

    let mut keys: Vec<&str> = listed[0]
        .as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, INVITE_KEYS);
}

#[tokio::test]
async fn listing_invites_answers_an_empty_array_when_there_are_none() {
    let app = TestApp::spawn().await;
    let (_caller, token) = admin(&app).await;

    let response = app.server.get(INVITES).authorization_bearer(&token).await;

    response.assert_status(StatusCode::OK);
    assert_eq!(response.json::<Value>(), json!([]));
}

#[tokio::test]
async fn revoking_an_invite_removes_it_and_kills_its_link() {
    let app = TestApp::spawn().await;
    let (_caller, token) = admin(&app).await;
    let invite = create_ok(&app, &token, "ada@example.test").await;
    let raw = token_in(&only_message(&app));

    let response = app
        .server
        .delete(&format!("{INVITES}/{}", id_of(&invite)))
        .authorization_bearer(&token)
        .await;

    response.assert_status(StatusCode::NO_CONTENT);

    let listed = app
        .server
        .get(INVITES)
        .authorization_bearer(&token)
        .await
        .json::<Value>();
    assert_eq!(listed, json!([]));

    assert!(
        UserInviteRepository::new(&app.pool)
            .find_open_by_hash(&OpaqueToken::hash_of(&raw))
            .await
            .expect("the lookup succeeds")
            .is_none(),
        "the revoked link still resolves"
    );
}

#[tokio::test]
async fn revoking_an_unknown_or_accepted_invite_is_404() {
    let app = TestApp::spawn().await;
    let (_caller, token) = admin(&app).await;
    let invite = create_ok(&app, &token, "ada@example.test").await;
    app.mock_email().clear();
    let user = app
        .insert_user("ada", "ada@example.test", false, false)
        .await;
    mark_accepted(&app, id_of(&invite), user.id).await;

    for id in [id_of(&invite), Uuid::new_v4()] {
        let response = app
            .server
            .delete(&format!("{INVITES}/{id}"))
            .authorization_bearer(&token)
            .await;

        response.assert_status(StatusCode::NOT_FOUND);
    }

    // The accepted row is history and stays put.
    assert_eq!(stored_hash(&app, id_of(&invite)).await.len(), 64);
}

#[tokio::test]
async fn resending_issues_a_new_token_and_a_new_expiry() {
    let app = TestApp::spawn().await;
    let (_caller, token) = admin(&app).await;
    let first = create_ok(&app, &token, "ada@example.test").await;
    let first_raw = token_in(&only_message(&app));

    // An invite that has almost run out is the case resending is for.
    set_expiry(&app, id_of(&first), Utc::now() + TimeDelta::hours(1)).await;

    let response = app
        .server
        .post(&format!("{INVITES}/{}/resend", id_of(&first)))
        .authorization_bearer(&token)
        .await;

    response.assert_status(StatusCode::OK);
    let body = response.json::<Value>();
    assert_eq!(id_of(&body), id_of(&first), "the same invite, resent");
    assert!(
        expiry_of(&body) > Utc::now() + TimeDelta::days(6),
        "the expiry did not restart: {}",
        body["expires_at"]
    );

    let message = only_message(&app);
    assert_eq!(message.to, "ada@example.test");
    let second_raw = token_in(&message);
    assert_ne!(second_raw, first_raw, "the old token was sent again");
    assert_eq!(
        OpaqueToken::hash_of(&second_raw),
        stored_hash(&app, id_of(&first)).await
    );

    let invites = UserInviteRepository::new(&app.pool);
    assert!(
        invites
            .find_open_by_hash(&OpaqueToken::hash_of(&first_raw))
            .await
            .expect("the lookup succeeds")
            .is_none(),
        "the superseded link still resolves"
    );
    assert!(
        invites
            .find_open_by_hash(&OpaqueToken::hash_of(&second_raw))
            .await
            .expect("the lookup succeeds")
            .is_some(),
        "the new link does not resolve"
    );
}

#[tokio::test]
async fn an_expired_invite_may_be_resent() {
    let app = TestApp::spawn().await;
    let (_caller, token) = admin(&app).await;
    let invite = create_ok(&app, &token, "ada@example.test").await;
    app.mock_email().clear();
    set_expiry(&app, id_of(&invite), Utc::now() - TimeDelta::days(3)).await;

    let response = app
        .server
        .post(&format!("{INVITES}/{}/resend", id_of(&invite)))
        .authorization_bearer(&token)
        .await;

    response.assert_status(StatusCode::OK);
    assert!(expiry_of(&response.json::<Value>()) > Utc::now());
    assert_eq!(only_message(&app).to, "ada@example.test");
}

#[tokio::test]
async fn resending_an_unknown_or_accepted_invite_is_404() {
    let app = TestApp::spawn().await;
    let (_caller, token) = admin(&app).await;
    let invite = create_ok(&app, &token, "ada@example.test").await;
    app.mock_email().clear();
    let user = app
        .insert_user("ada", "ada@example.test", false, false)
        .await;
    mark_accepted(&app, id_of(&invite), user.id).await;
    let hash_before = stored_hash(&app, id_of(&invite)).await;

    for id in [id_of(&invite), Uuid::new_v4()] {
        let response = app
            .server
            .post(&format!("{INVITES}/{id}/resend"))
            .authorization_bearer(&token)
            .await;

        response.assert_status(StatusCode::NOT_FOUND);
    }

    assert_eq!(
        stored_hash(&app, id_of(&invite)).await,
        hash_before,
        "an accepted invite's token was rotated"
    );
    assert!(
        app.mock_email().sent().is_empty(),
        "a 404 sent a message anyway"
    );
}

#[tokio::test]
async fn a_failed_send_is_a_500_that_leaves_the_invite_for_a_resend() {
    let app = TestApp::spawn().await;
    let (_caller, token) = admin(&app).await;

    app.mock_email().fail_next();
    let response = create(&app, &token, json!({ "email": "ada@example.test" })).await;

    response.assert_status(StatusCode::INTERNAL_SERVER_ERROR);
    // The body never says why: a mail provider's failure is nobody's business
    // but the operator's log (`SPEC.md`, "REST API").
    assert_eq!(
        response.json::<Value>(),
        json!({ "status": 500, "error": "internal error" })
    );
    assert!(app.mock_email().sent().is_empty());

    // The row committed before the send, so the invite is there to recover.
    let listed = app
        .server
        .get(INVITES)
        .authorization_bearer(&token)
        .await
        .json::<Value>();
    let listed = listed.as_array().expect("an array");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["email"], json!("ada@example.test"));

    let resent = app
        .server
        .post(&format!("{INVITES}/{}/resend", id_of(&listed[0])))
        .authorization_bearer(&token)
        .await;

    resent.assert_status(StatusCode::OK);
    let message = only_message(&app);
    assert_eq!(
        OpaqueToken::hash_of(&token_in(&message)),
        stored_hash(&app, id_of(&listed[0])).await
    );
}

#[tokio::test]
async fn a_failed_resend_is_a_500_that_still_rotated_the_token() {
    let app = TestApp::spawn().await;
    let (_caller, token) = admin(&app).await;
    let invite = create_ok(&app, &token, "ada@example.test").await;
    let first_raw = token_in(&only_message(&app));

    app.mock_email().fail_next();
    let response = app
        .server
        .post(&format!("{INVITES}/{}/resend", id_of(&invite)))
        .authorization_bearer(&token)
        .await;

    response.assert_status(StatusCode::INTERNAL_SERVER_ERROR);
    assert!(app.mock_email().sent().is_empty());

    // The rotation committed, so the old link is dead either way and the
    // administrator's next move is another resend.
    assert_ne!(
        stored_hash(&app, id_of(&invite)).await,
        OpaqueToken::hash_of(&first_raw),
        "the token was not rotated"
    );
}

#[tokio::test]
async fn every_invite_route_refuses_a_non_administrator() {
    let app = TestApp::spawn().await;
    let (_caller, admin_token) = admin(&app).await;
    let invite = create_ok(&app, &admin_token, "ada@example.test").await;
    app.mock_email().clear();

    let user = app
        .insert_user("alan", "alan@example.test", false, false)
        .await;
    let token = app.token_for(&user);
    let by_id = format!("{INVITES}/{}", id_of(&invite));
    let resend = format!("{INVITES}/{}/resend", id_of(&invite));

    let responses = vec![
        app.server.get(INVITES).authorization_bearer(&token).await,
        create(&app, &token, json!({ "email": "grete@example.test" })).await,
        app.server.delete(&by_id).authorization_bearer(&token).await,
        app.server.post(&resend).authorization_bearer(&token).await,
    ];

    for response in responses {
        response.assert_status(StatusCode::FORBIDDEN);
        assert_eq!(response.json::<Value>(), admin_required());
    }

    assert!(app.mock_email().sent().is_empty());
}

#[tokio::test]
async fn every_invite_route_requires_a_token() {
    let app = TestApp::spawn().await;
    let (_caller, admin_token) = admin(&app).await;
    let invite = create_ok(&app, &admin_token, "ada@example.test").await;
    app.mock_email().clear();

    let by_id = format!("{INVITES}/{}", id_of(&invite));
    let resend = format!("{INVITES}/{}/resend", id_of(&invite));

    let responses = vec![
        app.server.get(INVITES).await,
        app.server
            .post(INVITES)
            .json(&json!({ "email": "grete@example.test" }))
            .await,
        app.server.delete(&by_id).await,
        app.server.post(&resend).await,
    ];

    for response in responses {
        response.assert_status(StatusCode::UNAUTHORIZED);
        assert_eq!(response.json::<Value>(), unauthorized());
    }

    assert!(app.mock_email().sent().is_empty());
}

#[tokio::test]
async fn inviting_is_refused_while_the_callers_own_password_change_is_pending() {
    let app = TestApp::spawn().await;
    let gated = app
        .insert_user("grace", "grace@example.test", true, true)
        .await;
    let token = app.token_for(&gated);

    let response = create(&app, &token, json!({ "email": "ada@example.test" })).await;

    response.assert_status(StatusCode::FORBIDDEN);
    assert_eq!(
        response.json::<Value>(),
        json!({ "status": 403, "error": "password change required" })
    );
    assert!(app.mock_email().sent().is_empty());
}
