//! `/api/users` through the real router (`SPEC.md`, "Users (`/api/users`)").
//!
//! Two contracts are asserted here. The ordinary one — who may call what, and
//! what each route answers — and the administrator-membership invariant:
//! "Both deleting an administrator and changing `admin` from true to false are
//! rejected with 409 if they would remove the last administrator ... A
//! rejected request changes no fields." The rejection tests therefore read the
//! row back afterwards rather than trusting the status code alone, because the
//! whole point of the transaction is that nothing before the check survives it.
//!
//! The demotion tests also pin the other half of ADR 0025: a role change is
//! *not* a credential change, so the demoted administrator's existing access
//! token keeps working — it simply stops reaching an administrator route.
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use std::time::Duration;

use axum::http::StatusCode;
use chrono::Utc;
use common::TestApp;
use mars_orchestrator::models::User;
use mars_orchestrator::prelude::*;
use mars_orchestrator::repositories::users::ADMIN_MEMBERSHIP_LOCK_KEY;
use mars_orchestrator::repositories::{RefreshTokenRepository, UserRepository};
use serde_json::{Value, json};
use uuid::Uuid;

/// `GET /users` and the two administrator mutations live under this prefix.
const USERS: &str = "/api/users";

/// The self routes.
const ME: &str = "/api/users/me";

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

/// The row as the database has it now, whatever a response claimed.
async fn stored(app: &TestApp, id: Uuid) -> User {
    UserRepository::new(&app.pool)
        .find(id)
        .await
        .expect("the lookup succeeds")
        .expect("the user exists")
}

#[tokio::test]
async fn me_answers_the_signed_in_user_through_the_password_change_gate() {
    let app = TestApp::spawn().await;
    let gated = app
        .insert_user("ada", "ada@example.test", false, true)
        .await;
    let ungated = app
        .insert_user("grace", "grace@example.test", true, false)
        .await;

    for user in [&gated, &ungated] {
        let token = app.token_for(user);
        let response = app.server.get(ME).authorization_bearer(&token).await;

        response.assert_status(StatusCode::OK);
        let body = response.json::<Value>();
        assert_eq!(body["id"], json!(user.id.to_string()));
        assert_eq!(body["username"], json!(user.username));
        assert_eq!(body["email"], json!(user.email));
        assert_eq!(body["admin"], json!(user.admin));
        assert_eq!(
            body["must_change_password"],
            json!(user.must_change_password)
        );
        assert_eq!(body["notify_email"], json!(true));
        // The two internal columns are never part of the `User` DTO.
        assert_eq!(body.get("password_hash"), None);
        assert_eq!(body.get("auth_version"), None);
    }
}

/// The prelude's `Path` wrapper through this module's routes (`SPEC.md`,
/// "REST API"; `ARCHITECTURE.md`, "Orchestrator internals", Errors).
///
/// Every failure answers `{ "status", "error" }`, extractor rejections
/// included, so a path segment that is not a UUID must not fall through to
/// axum's plain-text rejection. The caller is an administrator so that a 403
/// can never be what is being observed, and the routes are driven with the
/// method each one actually has: a 405 would assert nothing.
#[tokio::test]
async fn a_path_segment_that_is_not_a_uuid_is_400_in_the_documented_shape() {
    let app = TestApp::spawn().await;
    let admin = app
        .insert_user("ada", "ada@example.test", true, false)
        .await;
    let token = app.token_for(&admin);

    let bad = format!("{USERS}/not-a-uuid");
    let invite = format!("{USERS}/invites/not-a-uuid");
    let requests = [
        app.server.get(&bad),
        app.server.put(&bad).json(&json!({ "username": "grace" })),
        app.server.delete(&bad),
        app.server
            .post(&format!("{bad}/password"))
            .json(&json!({ "password": "not-a-real-password" })),
        app.server.delete(&invite),
        app.server.post(&format!("{invite}/resend")),
    ];

    for request in requests {
        let response = request.authorization_bearer(&token).await;

        response.assert_status(StatusCode::BAD_REQUEST);
        let body = response.json::<Value>();
        assert_eq!(body["status"], json!(400));
        assert!(
            body["error"].is_string(),
            "the rejection carried no message: {body}"
        );
    }
}

#[tokio::test]
async fn me_requires_a_token() {
    let app = TestApp::spawn().await;

    let response = app.server.get(ME).await;

    response.assert_status(StatusCode::UNAUTHORIZED);
    assert_eq!(response.json::<Value>(), unauthorized());
}

#[tokio::test]
async fn patching_me_sets_notify_email_and_persists_it() {
    let app = TestApp::spawn().await;
    let user = app
        .insert_user("ada", "ada@example.test", false, false)
        .await;
    let token = app.token_for(&user);
    assert!(user.notify_email, "the column defaults to true");

    let response = app
        .server
        .patch(ME)
        .authorization_bearer(&token)
        .json(&json!({ "notify_email": false }))
        .await;

    response.assert_status(StatusCode::OK);
    assert_eq!(response.json::<Value>()["notify_email"], json!(false));
    assert!(!stored(&app, user.id).await.notify_email);

    // And back, so the route is not merely a one-way switch.
    let response = app
        .server
        .patch(ME)
        .authorization_bearer(&token)
        .json(&json!({ "notify_email": true }))
        .await;

    response.assert_status(StatusCode::OK);
    assert!(stored(&app, user.id).await.notify_email);
}

#[tokio::test]
async fn patching_me_with_an_empty_body_answers_the_current_user() {
    let app = TestApp::spawn().await;
    let user = app
        .insert_user("ada", "ada@example.test", false, false)
        .await;
    let token = app.token_for(&user);

    let response = app
        .server
        .patch(ME)
        .authorization_bearer(&token)
        .json(&json!({}))
        .await;

    response.assert_status(StatusCode::OK);
    let body = response.json::<Value>();
    assert_eq!(body["id"], json!(user.id.to_string()));
    assert_eq!(body["notify_email"], json!(true));
}

#[tokio::test]
async fn patching_me_rejects_a_field_that_is_not_the_users_to_set() {
    let app = TestApp::spawn().await;
    let user = app
        .insert_user("ada", "ada@example.test", false, false)
        .await;
    let token = app.token_for(&user);

    for body in [
        json!({ "admin": true }),
        json!({ "username": "ada.l" }),
        json!({ "notify_email": false, "admin": true }),
    ] {
        let response = app
            .server
            .patch(ME)
            .authorization_bearer(&token)
            .json(&body)
            .await;

        response.assert_status(StatusCode::BAD_REQUEST);
        assert_eq!(response.json::<Value>()["status"], json!(400), "{body}");
    }

    // Nothing of the rejected bodies was applied.
    let row = stored(&app, user.id).await;
    assert_eq!(row.username, "ada");
    assert!(!row.admin);
    assert!(row.notify_email);
}

#[tokio::test]
async fn patching_me_is_refused_while_a_password_change_is_pending() {
    let app = TestApp::spawn().await;
    let user = app
        .insert_user("ada", "ada@example.test", false, true)
        .await;
    let token = app.token_for(&user);

    let response = app
        .server
        .patch(ME)
        .authorization_bearer(&token)
        .json(&json!({ "notify_email": false }))
        .await;

    response.assert_status(StatusCode::FORBIDDEN);
    assert_eq!(
        response.json::<Value>(),
        json!({ "status": 403, "error": "password change required" })
    );
}

#[tokio::test]
async fn listing_users_answers_every_user_by_username() {
    let app = TestApp::spawn().await;
    let admin = app
        .insert_user("zoe", "zoe@example.test", true, false)
        .await;
    let ada = app
        .insert_user("ada", "ada@example.test", false, false)
        .await;
    let grace = app
        .insert_user("grace", "grace@example.test", false, false)
        .await;
    let token = app.token_for(&admin);

    let response = app.server.get(USERS).authorization_bearer(&token).await;

    response.assert_status(StatusCode::OK);
    let body = response.json::<Vec<Value>>();
    assert_eq!(
        body.iter()
            .map(|user| user["id"].as_str().unwrap_or_default().to_string())
            .collect::<Vec<_>>(),
        [
            ada.id.to_string(),
            grace.id.to_string(),
            admin.id.to_string(),
        ],
        "the listing is ordered by username, not by creation",
    );
    assert_eq!(body[0].get("password_hash"), None);
}

#[tokio::test]
async fn listing_users_requires_an_administrator() {
    let app = TestApp::spawn().await;
    let user = app
        .insert_user("ada", "ada@example.test", false, false)
        .await;
    let token = app.token_for(&user);

    let response = app.server.get(USERS).authorization_bearer(&token).await;
    response.assert_status(StatusCode::FORBIDDEN);
    assert_eq!(response.json::<Value>(), admin_required());

    let response = app.server.get(USERS).await;
    response.assert_status(StatusCode::UNAUTHORIZED);
    assert_eq!(response.json::<Value>(), unauthorized());
}

#[tokio::test]
async fn any_authenticated_user_may_read_any_other_user() {
    let app = TestApp::spawn().await;
    let reader = app
        .insert_user("ada", "ada@example.test", false, false)
        .await;
    let other = app
        .insert_user("grace", "grace@example.test", true, false)
        .await;
    let token = app.token_for(&reader);

    let response = app
        .server
        .get(&format!("{USERS}/{}", other.id))
        .authorization_bearer(&token)
        .await;

    response.assert_status(StatusCode::OK);
    let body = response.json::<Value>();
    assert_eq!(body["id"], json!(other.id.to_string()));
    assert_eq!(body["username"], json!("grace"));
    assert_eq!(body["admin"], json!(true));
}

#[tokio::test]
async fn reading_an_unknown_user_is_404() {
    let app = TestApp::spawn().await;
    let reader = app
        .insert_user("ada", "ada@example.test", false, false)
        .await;
    let token = app.token_for(&reader);

    let response = app
        .server
        .get(&format!("{USERS}/{}", Uuid::new_v4()))
        .authorization_bearer(&token)
        .await;

    response.assert_status(StatusCode::NOT_FOUND);
    assert_eq!(
        response.json::<Value>(),
        json!({ "status": 404, "error": "not found" })
    );
}

#[tokio::test]
async fn an_administrator_renames_and_promotes_a_user() {
    let app = TestApp::spawn().await;
    let admin = app
        .insert_user("zoe", "zoe@example.test", true, false)
        .await;
    let target = app
        .insert_user("ada", "ada@example.test", false, false)
        .await;
    let token = app.token_for(&admin);

    let response = app
        .server
        .put(&format!("{USERS}/{}", target.id))
        .authorization_bearer(&token)
        .json(&json!({ "username": "ada.l", "admin": true }))
        .await;

    response.assert_status(StatusCode::OK);
    let body = response.json::<Value>();
    assert_eq!(body["username"], json!("ada.l"));
    assert_eq!(body["admin"], json!(true));

    let row = stored(&app, target.id).await;
    assert_eq!(row.username, "ada.l");
    assert!(row.admin);
    // A role change is not a credential change (ADR 0025).
    assert_eq!(row.auth_version, target.auth_version);
    // And the fields the route does not own are untouched.
    assert_eq!(row.email, target.email);
    assert_eq!(row.notify_email, target.notify_email);
}

#[tokio::test]
async fn writing_the_same_values_back_is_an_accepted_no_op() {
    let app = TestApp::spawn().await;
    let admin = app
        .insert_user("zoe", "zoe@example.test", true, false)
        .await;
    let target = app
        .insert_user("ada", "ada@example.test", false, false)
        .await;
    let token = app.token_for(&admin);

    let response = app
        .server
        .put(&format!("{USERS}/{}", target.id))
        .authorization_bearer(&token)
        .json(&json!({ "username": "ada", "admin": false }))
        .await;

    response.assert_status(StatusCode::OK);
    assert_eq!(response.json::<Value>()["username"], json!("ada"));
    assert_eq!(stored(&app, target.id).await.username, "ada");
}

#[tokio::test]
async fn renaming_a_user_to_a_taken_username_is_409() {
    let app = TestApp::spawn().await;
    let admin = app
        .insert_user("zoe", "zoe@example.test", true, false)
        .await;
    let target = app
        .insert_user("ada", "ada@example.test", false, false)
        .await;
    app.insert_user("grace", "grace@example.test", false, false)
        .await;
    let token = app.token_for(&admin);

    let response = app
        .server
        .put(&format!("{USERS}/{}", target.id))
        .authorization_bearer(&token)
        .json(&json!({ "username": "grace", "admin": false }))
        .await;

    response.assert_status(StatusCode::CONFLICT);
    assert_eq!(response.json::<Value>(), conflict("username already taken"));
    assert_eq!(stored(&app, target.id).await.username, "ada");
}

#[tokio::test]
async fn an_invalid_username_is_400() {
    let app = TestApp::spawn().await;
    let admin = app
        .insert_user("zoe", "zoe@example.test", true, false)
        .await;
    let target = app
        .insert_user("ada", "ada@example.test", false, false)
        .await;
    let token = app.token_for(&admin);

    let response = app
        .server
        .put(&format!("{USERS}/{}", target.id))
        .authorization_bearer(&token)
        .json(&json!({ "username": "ad", "admin": false }))
        .await;

    response.assert_status(StatusCode::BAD_REQUEST);
    assert_eq!(
        response.json::<Value>(),
        json!({ "status": 400, "error": "username must be 3-32 characters" })
    );
    assert_eq!(stored(&app, target.id).await.username, "ada");
}

#[tokio::test]
async fn a_body_that_omits_a_required_field_is_400() {
    let app = TestApp::spawn().await;
    let admin = app
        .insert_user("zoe", "zoe@example.test", true, false)
        .await;
    let target = app
        .insert_user("ada", "ada@example.test", false, false)
        .await;
    let token = app.token_for(&admin);

    for body in [
        json!({ "username": "ada.l" }),
        json!({ "admin": true }),
        json!({ "username": "ada.l", "admin": true, "email": "nope@example.test" }),
    ] {
        let response = app
            .server
            .put(&format!("{USERS}/{}", target.id))
            .authorization_bearer(&token)
            .json(&body)
            .await;

        response.assert_status(StatusCode::BAD_REQUEST);
        assert_eq!(response.json::<Value>()["status"], json!(400), "{body}");
    }

    let row = stored(&app, target.id).await;
    assert_eq!(row.username, "ada");
    assert!(!row.admin);
}

#[tokio::test]
async fn demoting_the_last_administrator_is_409_and_changes_nothing() {
    let app = TestApp::spawn().await;
    let admin = app
        .insert_user("zoe", "zoe@example.test", true, false)
        .await;
    app.insert_user("ada", "ada@example.test", false, false)
        .await;
    let token = app.token_for(&admin);

    let response = app
        .server
        .put(&format!("{USERS}/{}", admin.id))
        .authorization_bearer(&token)
        .json(&json!({ "username": "zoe.renamed", "admin": false }))
        .await;

    response.assert_status(StatusCode::CONFLICT);
    assert_eq!(
        response.json::<Value>(),
        conflict("cannot demote the last administrator")
    );

    // The rename rode along in the rejected request and must have rolled back
    // with it (`SPEC.md`, "Users": "A rejected request changes no fields").
    let row = stored(&app, admin.id).await;
    assert!(row.admin);
    assert_eq!(row.username, "zoe");
    assert_eq!(row.updated_at, admin.updated_at);
}

#[tokio::test]
async fn self_demotion_succeeds_while_another_administrator_remains() {
    let app = TestApp::spawn().await;
    let admin = app
        .insert_user("zoe", "zoe@example.test", true, false)
        .await;
    app.insert_user("grace", "grace@example.test", true, false)
        .await;
    let token = app.token_for(&admin);

    let response = app
        .server
        .put(&format!("{USERS}/{}", admin.id))
        .authorization_bearer(&token)
        .json(&json!({ "username": "zoe", "admin": false }))
        .await;

    response.assert_status(StatusCode::OK);
    assert_eq!(response.json::<Value>()["admin"], json!(false));

    let row = stored(&app, admin.id).await;
    assert!(!row.admin);
    // No credential was revoked: the same token still authenticates, and the
    // version it carries still matches (ADR 0025).
    assert_eq!(row.auth_version, admin.auth_version);

    let response = app.server.get(ME).authorization_bearer(&token).await;
    response.assert_status(StatusCode::OK);
    assert_eq!(response.json::<Value>()["admin"], json!(false));

    // What the demotion did take effect on, on the very next request.
    let response = app.server.get(USERS).authorization_bearer(&token).await;
    response.assert_status(StatusCode::FORBIDDEN);
    assert_eq!(response.json::<Value>(), admin_required());
}

#[tokio::test]
async fn putting_an_unknown_user_is_404() {
    let app = TestApp::spawn().await;
    let admin = app
        .insert_user("zoe", "zoe@example.test", true, false)
        .await;
    let token = app.token_for(&admin);

    let response = app
        .server
        .put(&format!("{USERS}/{}", Uuid::new_v4()))
        .authorization_bearer(&token)
        .json(&json!({ "username": "ghost", "admin": false }))
        .await;

    response.assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn deleting_a_user_removes_the_row_and_their_refresh_tokens() {
    let app = TestApp::spawn().await;
    let admin = app
        .insert_user("zoe", "zoe@example.test", true, false)
        .await;
    let target = app
        .insert_user("ada", "ada@example.test", false, false)
        .await;
    let target_token = app.token_for(&target);

    // An obviously fake stand-in for the SHA-256 hex of a refresh token (rule
    // 3); nothing verifies against it, the row only has to exist.
    let mut tx = app.pool.begin().await.expect("a transaction begins");
    RefreshTokenRepository::new(&app.pool)
        .insert(
            &mut tx,
            target.id,
            &"ad".repeat(32),
            Utc::now() + REFRESH_TOKEN_TTL,
        )
        .await
        .expect("the refresh token inserts");
    tx.commit().await.expect("the transaction commits");

    let response = app
        .server
        .delete(&format!("{USERS}/{}", target.id))
        .authorization_bearer(app.token_for(&admin))
        .await;

    response.assert_status(StatusCode::NO_CONTENT);
    assert!(
        UserRepository::new(&app.pool)
            .find(target.id)
            .await
            .unwrap()
            .is_none()
    );

    // `refresh_tokens.user_id` cascades (`docs/data-model.md`).
    let remaining: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM refresh_tokens WHERE user_id = $1")
            .bind(target.id)
            .fetch_one(&app.pool)
            .await
            .expect("the count reads back");
    assert_eq!(remaining, 0);

    // Their access token was not revoked and does not have to be: the row is
    // gone, so the extractor cannot load it (ADR 0025).
    let response = app.server.get(ME).authorization_bearer(&target_token).await;
    response.assert_status(StatusCode::UNAUTHORIZED);
    assert_eq!(response.json::<Value>(), unauthorized());
}

#[tokio::test]
async fn deleting_yourself_is_409() {
    let app = TestApp::spawn().await;
    let admin = app
        .insert_user("zoe", "zoe@example.test", true, false)
        .await;
    app.insert_user("grace", "grace@example.test", true, false)
        .await;
    let token = app.token_for(&admin);

    let response = app
        .server
        .delete(&format!("{USERS}/{}", admin.id))
        .authorization_bearer(&token)
        .await;

    // Refused even though a second administrator would remain.
    response.assert_status(StatusCode::CONFLICT);
    assert_eq!(response.json::<Value>(), conflict("cannot delete yourself"));
    assert!(
        UserRepository::new(&app.pool)
            .find(admin.id)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn deleting_the_last_administrator_is_409() {
    // The check cannot be reached by a single request on its own: an
    // administrator asking to delete *another* administrator is itself the
    // second one, and asking to delete themselves is refused as self-deletion
    // before the transaction opens. It is reached when membership changes
    // while the request is already in flight — exactly what the advisory lock
    // exists for — so this test creates that window deterministically by
    // holding the lock the route is about to take.
    let app = TestApp::spawn().await;
    let caller = app
        .insert_user("zoe", "zoe@example.test", true, false)
        .await;
    // Gated on a password change, to show the gate is irrelevant to the check.
    let target = app
        .insert_user("grace", "grace@example.test", true, true)
        .await;
    let token = app.token_for(&caller);

    // Held before the request starts, so the route blocks on it after `BEGIN`
    // and before it counts anything.
    let mut holder = app.pool.begin().await.expect("a transaction begins");
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(ADMIN_MEMBERSHIP_LOCK_KEY)
        .execute(&mut *holder)
        .await
        .expect("the lock is granted");

    let request = app
        .server
        .delete(&format!("{USERS}/{}", target.id))
        .authorization_bearer(&token);

    let demote_the_caller = async {
        // Long enough for the request to authenticate — which reads `caller`
        // while they are still an administrator — and to reach the lock.
        tokio::time::sleep(Duration::from_millis(500)).await;

        sqlx::query("UPDATE users SET admin = FALSE WHERE id = $1")
            .bind(caller.id)
            .execute(&mut *holder)
            .await
            .expect("the caller is demoted");
        holder.commit().await.expect("the holder commits");
    };

    let (response, ()) = tokio::join!(request, demote_the_caller);

    // `grace` is now the only administrator, and the count the route reads
    // under the lock says so.
    response.assert_status(StatusCode::CONFLICT);
    assert_eq!(
        response.json::<Value>(),
        conflict("cannot delete the last administrator")
    );

    let row = stored(&app, target.id).await;
    assert!(row.admin);
    assert!(row.must_change_password);
}

#[tokio::test]
async fn deleting_an_unknown_user_is_404() {
    let app = TestApp::spawn().await;
    let admin = app
        .insert_user("zoe", "zoe@example.test", true, false)
        .await;
    let token = app.token_for(&admin);

    let response = app
        .server
        .delete(&format!("{USERS}/{}", Uuid::new_v4()))
        .authorization_bearer(&token)
        .await;

    response.assert_status(StatusCode::NOT_FOUND);
    assert_eq!(
        response.json::<Value>(),
        json!({ "status": 404, "error": "not found" })
    );
}

#[tokio::test]
async fn the_administrator_mutations_refuse_everybody_else() {
    let app = TestApp::spawn().await;
    let admin = app
        .insert_user("zoe", "zoe@example.test", true, false)
        .await;
    let user = app
        .insert_user("ada", "ada@example.test", false, false)
        .await;
    let token = app.token_for(&user);
    let path = format!("{USERS}/{}", admin.id);

    let response = app
        .server
        .put(&path)
        .authorization_bearer(&token)
        .json(&json!({ "username": "zoe.renamed", "admin": false }))
        .await;
    response.assert_status(StatusCode::FORBIDDEN);
    assert_eq!(response.json::<Value>(), admin_required());

    let response = app.server.delete(&path).authorization_bearer(&token).await;
    response.assert_status(StatusCode::FORBIDDEN);
    assert_eq!(response.json::<Value>(), admin_required());

    // And without a token at all.
    let response = app
        .server
        .put(&path)
        .json(&json!({ "username": "zoe.renamed", "admin": false }))
        .await;
    response.assert_status(StatusCode::UNAUTHORIZED);

    let response = app.server.delete(&path).await;
    response.assert_status(StatusCode::UNAUTHORIZED);

    // Nothing was renamed, demoted or deleted.
    let row = stored(&app, admin.id).await;
    assert_eq!(row.username, "zoe");
    assert!(row.admin);
}
