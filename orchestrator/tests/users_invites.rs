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
use common::{AuthenticatedUser, TestApp};
use mars_orchestrator::email::EmailMessage;
use mars_orchestrator::models::OpaqueToken;
use serde_json::{Value, json};
use uuid::Uuid;

/// The invite collection.
const INVITES: &str = "/api/users/invites";

/// Obviously fake, and long enough for the documented 10–128 range
/// (`CLAUDE.md`, rule 3).
const PASSWORD: &str = "correct-horse-battery-staple";

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
async fn admin(app: &TestApp) -> AuthenticatedUser {
    app.create_admin("grace", "grace@example.test", PASSWORD)
        .await
}

/// `POST /api/users/invites` as `token`.
async fn create(app: &TestApp, token: &str, body: Value) -> axum_test::TestResponse {
    app.server
        .post(INVITES)
        .authorization_bearer(token)
        .json(&body)
        .await
}

/// The `token_hash` column of one invite, whatever the response said.
///
/// A row-level fact with no interface: the point of these tests is that the
/// raw token is *not* anywhere else, so what the row stores can only be read
/// from the row.
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
///
/// A row-level fact with no interface: the expiry lives in the row and no
/// route moves it backwards, so the row is aged the way the clock would have
/// aged it.
async fn set_expiry(app: &TestApp, id: Uuid, expires_at: DateTime<Utc>) {
    sqlx::query("UPDATE user_invites SET expires_at = $1 WHERE id = $2")
        .bind(expires_at)
        .bind(id)
        .execute(&app.pool)
        .await
        .expect("the expiry is moved");
}

/// The single message the mock captured and the token in its link, with the
/// capture cleared afterwards.
fn only_message(app: &TestApp) -> (EmailMessage, String) {
    let sent = app.mock_email().sent();
    assert_eq!(sent.len(), 1, "expected exactly one message: {sent:?}");
    let token = app.invite_link_token(0);
    app.mock_email().clear();

    (sent.into_iter().next().expect("one message"), token)
}

/// Whether the emailed link still resolves, as `GET /auth/invite/{token}`
/// answers it (`SPEC.md`, "Auth (`/api/auth`)").
///
/// The interface behind every "the link is dead" assertion here: an open
/// invitation is 200 and an unknown, revoked, superseded, expired or accepted
/// one is the documented 400.
async fn link_resolves(app: &TestApp, token: &str) -> bool {
    let response = app.lookup_invite(token).await;

    match response.status_code() {
        StatusCode::OK => true,
        StatusCode::BAD_REQUEST => false,
        other => panic!("unexpected status looking the link up: {other}"),
    }
}

/// The open invites, as the administrator list route answers them.
async fn open_invites(app: &TestApp, caller: &AuthenticatedUser) -> Vec<Value> {
    let response = app.get_as(caller, INVITES).await;

    response.assert_status(StatusCode::OK);
    response
        .json::<Value>()
        .as_array()
        .expect("an array")
        .clone()
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
    let caller = admin(&app).await;

    let response = create(
        &app,
        &caller.access_token,
        json!({ "email": "  Ada@Example.TEST " }),
    )
    .await;

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
    assert_eq!(body["invited_by"], json!(caller.user.id.to_string()));

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
    let (_message, raw) = only_message(&app);
    assert!(!rendered.contains(&raw), "the token leaked: {rendered}");
}

#[tokio::test]
async fn the_emailed_link_carries_the_token_the_row_stores_the_hash_of() {
    let app = TestApp::spawn().await;
    let caller = admin(&app).await;

    let response = create(
        &app,
        &caller.access_token,
        json!({ "email": "ada@example.test" }),
    )
    .await;
    response.assert_status(StatusCode::CREATED);
    let body = response.json::<Value>();

    let (message, raw) = only_message(&app);
    assert_eq!(message.to, "ada@example.test");
    assert_eq!(message.subject, "You have been invited to Mars");

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

    // And the token resolves, which is what the invitee's lookup does with it.
    let looked_up = app.lookup_invite(&raw).await;
    looked_up.assert_status(StatusCode::OK);
    assert_eq!(
        looked_up.json::<Value>()["email"],
        json!("ada@example.test")
    );
}

#[tokio::test]
async fn an_invite_may_be_created_for_an_administrator() {
    let app = TestApp::spawn().await;
    let caller = admin(&app).await;

    let response = create(
        &app,
        &caller.access_token,
        json!({ "email": "ada@example.test", "admin": true }),
    )
    .await;

    response.assert_status(StatusCode::CREATED);
    assert_eq!(response.json::<Value>()["admin"], json!(true));

    let listed = open_invites(&app, &caller).await;
    assert_eq!(
        listed[0]["admin"],
        json!(true),
        "the flag is stored, not only answered"
    );
}

#[tokio::test]
async fn a_second_open_invite_for_the_same_address_is_409() {
    let app = TestApp::spawn().await;
    let caller = admin(&app).await;
    app.invite(&caller, "ada@example.test", false).await;

    // Including under a different spelling, because the address is normalised.
    let response = create(
        &app,
        &caller.access_token,
        json!({ "email": "ADA@example.test" }),
    )
    .await;

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
    let caller = admin(&app).await;
    app.insert_user("ada", "ada@example.test", false, false)
        .await;

    for address in ["ada@example.test", caller.user.email.as_str()] {
        let response = create(&app, &caller.access_token, json!({ "email": address })).await;

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
    let caller = admin(&app).await;
    let first = app.invite(&caller, "ada@example.test", false).await;

    // The partial unique index is blind to `expires_at`, so without the
    // reaping step in the route this would collide with the dead row.
    set_expiry(&app, first.id, Utc::now() - TimeDelta::days(1)).await;

    let second = app.invite(&caller, "ada@example.test", false).await;

    assert_ne!(second.id, first.id, "a new invite was created");
    assert!(expiry_of(&second.body) > Utc::now());
    assert_ne!(second.token, first.token);

    assert!(
        !link_resolves(&app, &first.token).await,
        "the expired link still resolves"
    );

    // And the expired invite is no longer anything an administrator can act
    // on: the replacement is the only one left open.
    let listed = open_invites(&app, &caller).await;
    assert_eq!(listed.len(), 1);
    assert_eq!(id_of(&listed[0]), second.id);
}

#[tokio::test]
async fn an_address_that_is_not_an_address_is_400() {
    let app = TestApp::spawn().await;
    let caller = admin(&app).await;

    let too_long = format!("{}@example.test", "a".repeat(250));
    for address in ["", "   ", "ada", "ada@", "@example.test", &too_long] {
        let response = create(&app, &caller.access_token, json!({ "email": address })).await;

        response.assert_status(StatusCode::BAD_REQUEST);
        assert_eq!(response.json::<Value>()["status"], json!(400), "{address}");
    }

    // And a body that is not the documented shape at all.
    for body in [
        json!({}),
        json!({ "email": "ada@example.test", "username": "ada" }),
        json!({ "email": "ada@example.test", "admin": "yes" }),
    ] {
        let response = create(&app, &caller.access_token, body.clone()).await;
        response.assert_status(StatusCode::BAD_REQUEST);
    }

    assert!(app.mock_email().sent().is_empty());
}

#[tokio::test]
async fn listing_invites_shows_the_open_ones_newest_first() {
    let app = TestApp::spawn().await;
    let caller = admin(&app).await;

    let older = app.invite(&caller, "ada@example.test", false).await;
    let expired = app.invite(&caller, "alan@example.test", false).await;
    // Accepted the way an invitee accepts one, so the row names the user the
    // acceptance created.
    app.invite_and_accept(&caller, "grete@example.test", "grete", PASSWORD, false)
        .await;
    let newest = app.invite(&caller, "edsger@example.test", false).await;

    set_expiry(&app, expired.id, Utc::now() - TimeDelta::hours(1)).await;

    let listed = open_invites(&app, &caller).await;

    let emails: Vec<&str> = listed
        .iter()
        .map(|invite| invite["email"].as_str().expect("email is a string"))
        .collect();
    assert_eq!(emails, vec!["edsger@example.test", "ada@example.test"]);
    assert_eq!(id_of(&listed[0]), newest.id);
    assert_eq!(id_of(&listed[1]), older.id);

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
    let caller = admin(&app).await;

    assert_eq!(open_invites(&app, &caller).await, Vec::<Value>::new());
}

#[tokio::test]
async fn revoking_an_invite_removes_it_and_kills_its_link() {
    let app = TestApp::spawn().await;
    let caller = admin(&app).await;
    let invited = app.invite(&caller, "ada@example.test", false).await;

    app.delete_as(&caller, &format!("{INVITES}/{}", invited.id))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    assert_eq!(open_invites(&app, &caller).await, Vec::<Value>::new());
    assert!(
        !link_resolves(&app, &invited.token).await,
        "the revoked link still resolves"
    );
}

#[tokio::test]
async fn revoking_an_unknown_or_accepted_invite_is_404() {
    let app = TestApp::spawn().await;
    let caller = admin(&app).await;
    let (invited, _ada) = app
        .invite_and_accept(&caller, "ada@example.test", "ada", PASSWORD, false)
        .await;

    for id in [invited.id, Uuid::new_v4()] {
        app.delete_as(&caller, &format!("{INVITES}/{id}"))
            .await
            .assert_status(StatusCode::NOT_FOUND);
    }

    // The accepted row is history and stays put — a row-level fact, because no
    // route answers an accepted invitation at all.
    assert_eq!(stored_hash(&app, invited.id).await.len(), 64);
}

#[tokio::test]
async fn resending_issues_a_new_token_and_a_new_expiry() {
    let app = TestApp::spawn().await;
    let caller = admin(&app).await;
    let first = app.invite(&caller, "ada@example.test", false).await;

    // An invite that has almost run out is the case resending is for.
    set_expiry(&app, first.id, Utc::now() + TimeDelta::hours(1)).await;

    let response = app
        .post_as(&caller, &format!("{INVITES}/{}/resend", first.id))
        .await;

    response.assert_status(StatusCode::OK);
    let body = response.json::<Value>();
    assert_eq!(id_of(&body), first.id, "the same invite, resent");
    assert!(
        expiry_of(&body) > Utc::now() + TimeDelta::days(6),
        "the expiry did not restart: {}",
        body["expires_at"]
    );

    let (message, second_raw) = only_message(&app);
    assert_eq!(message.to, "ada@example.test");
    assert_ne!(second_raw, first.token, "the old token was sent again");
    assert_eq!(
        OpaqueToken::hash_of(&second_raw),
        stored_hash(&app, first.id).await
    );

    assert!(
        !link_resolves(&app, &first.token).await,
        "the superseded link still resolves"
    );
    assert!(
        link_resolves(&app, &second_raw).await,
        "the new link does not resolve"
    );
}

#[tokio::test]
async fn an_expired_invite_may_be_resent() {
    let app = TestApp::spawn().await;
    let caller = admin(&app).await;
    let invited = app.invite(&caller, "ada@example.test", false).await;
    set_expiry(&app, invited.id, Utc::now() - TimeDelta::days(3)).await;

    let response = app
        .post_as(&caller, &format!("{INVITES}/{}/resend", invited.id))
        .await;

    response.assert_status(StatusCode::OK);
    assert!(expiry_of(&response.json::<Value>()) > Utc::now());
    assert_eq!(only_message(&app).0.to, "ada@example.test");
}

#[tokio::test]
async fn resending_an_unknown_or_accepted_invite_is_404() {
    let app = TestApp::spawn().await;
    let caller = admin(&app).await;
    let (invited, _ada) = app
        .invite_and_accept(&caller, "ada@example.test", "ada", PASSWORD, false)
        .await;
    // A row-level fact: an accepted invitation's token hash is the only thing
    // that says whether the refused resend rotated it.
    let hash_before = stored_hash(&app, invited.id).await;

    for id in [invited.id, Uuid::new_v4()] {
        app.post_as(&caller, &format!("{INVITES}/{id}/resend"))
            .await
            .assert_status(StatusCode::NOT_FOUND);
    }

    assert_eq!(
        stored_hash(&app, invited.id).await,
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
    let caller = admin(&app).await;

    app.mock_email().fail_next();
    let response = create(
        &app,
        &caller.access_token,
        json!({ "email": "ada@example.test" }),
    )
    .await;

    response.assert_status(StatusCode::INTERNAL_SERVER_ERROR);
    // The body never says why: a mail provider's failure is nobody's business
    // but the operator's log (`SPEC.md`, "REST API").
    assert_eq!(
        response.json::<Value>(),
        json!({ "status": 500, "error": "internal error" })
    );
    assert!(app.mock_email().sent().is_empty());

    // The row committed before the send, so the invite is there to recover.
    let listed = open_invites(&app, &caller).await;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["email"], json!("ada@example.test"));

    let resent = app
        .post_as(&caller, &format!("{INVITES}/{}/resend", id_of(&listed[0])))
        .await;

    resent.assert_status(StatusCode::OK);
    let (_message, raw) = only_message(&app);
    assert_eq!(
        OpaqueToken::hash_of(&raw),
        stored_hash(&app, id_of(&listed[0])).await
    );
    assert!(link_resolves(&app, &raw).await, "the resent link is dead");
}

#[tokio::test]
async fn a_failed_resend_is_a_500_that_still_rotated_the_token() {
    let app = TestApp::spawn().await;
    let caller = admin(&app).await;
    let invited = app.invite(&caller, "ada@example.test", false).await;

    app.mock_email().fail_next();
    let response = app
        .post_as(&caller, &format!("{INVITES}/{}/resend", invited.id))
        .await;

    response.assert_status(StatusCode::INTERNAL_SERVER_ERROR);
    assert!(app.mock_email().sent().is_empty());

    // The rotation committed, so the old link is dead either way and the
    // administrator's next move is another resend.
    assert!(
        !link_resolves(&app, &invited.token).await,
        "the token was not rotated"
    );
}

#[tokio::test]
async fn every_invite_route_refuses_a_non_administrator() {
    let app = TestApp::spawn().await;
    let caller = admin(&app).await;
    let invited = app.invite(&caller, "ada@example.test", false).await;

    let user = app
        .insert_user("alan", "alan@example.test", false, false)
        .await;
    let token = app.token_for(&user);
    let by_id = format!("{INVITES}/{}", invited.id);
    let resend = format!("{INVITES}/{}/resend", invited.id);

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
    let caller = admin(&app).await;
    let invited = app.invite(&caller, "ada@example.test", false).await;

    let by_id = format!("{INVITES}/{}", invited.id);
    let resend = format!("{INVITES}/{}/resend", invited.id);

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
