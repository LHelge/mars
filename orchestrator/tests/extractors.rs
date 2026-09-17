//! The authentication extractors through the real router (`SPEC.md`,
//! "Authentication"; ADR 0025).
//!
//! Driven through one real route per extractor — `GET /api/users/{id}` for
//! [`mars_orchestrator::routes::CurrentUser`], `GET /api/users/me` for
//! [`mars_orchestrator::routes::UngatedUser`] and `GET /api/users` for
//! [`mars_orchestrator::routes::AdminUser`] — so every assertion here goes
//! through `build_api_router`, the same middleware stack every other route
//! sits in. The routes are picked for their extractor, not for what they
//! return: the gated one is always pointed at the caller's own id, so a status
//! is the extractor's answer and never the lookup's.
//!
//! What is being asserted is a *contract about the database*, not about the
//! token: the claims are only a snapshot, so a deleted user, a superseded
//! `auth_version`, a demotion and a raised `must_change_password` all take
//! effect on the next request with no revocation of anything.
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use axum::http::StatusCode;
use axum::http::header::AUTHORIZATION;
use chrono::Utc;
use common::TestApp;
use mars_orchestrator::models::{User, UserUpdate};
use mars_orchestrator::prelude::*;
use mars_orchestrator::repositories::UserRepository;
use serde_json::{Value, json};

/// The route behind [`mars_orchestrator::routes::CurrentUser`], pointed at
/// `user`'s own id.
fn gated(user: &User) -> String {
    format!("/api/users/{}", user.id)
}

/// The same route for a request that is never expected to reach the handler,
/// because there is no user to point it at. The id is a valid UUID and names
/// nothing.
const GATED_ANY: &str = "/api/users/00000000-0000-0000-0000-00000000dead";

/// The route behind [`mars_orchestrator::routes::UngatedUser`].
const UNGATED: &str = "/api/users/me";

/// The route behind [`mars_orchestrator::routes::AdminUser`].
const ADMIN: &str = "/api/users";

/// The documented 401 body (`SPEC.md`, "Authentication").
fn unauthorized() -> Value {
    json!({ "status": 401, "error": "authentication required" })
}

/// The documented 403 body for `message`.
fn forbidden(message: &str) -> Value {
    json!({ "status": 403, "error": message })
}

#[tokio::test]
async fn a_valid_token_reaches_every_extractor_its_row_allows() {
    let app = TestApp::spawn().await;
    let user = app
        .insert_user("ada", "ada@example.test", true, false)
        .await;
    let token = app.token_for(&user);

    for path in [gated(&user), UNGATED.to_string()] {
        let response = app.server.get(&path).authorization_bearer(&token).await;

        response.assert_status(StatusCode::OK);
        let body = response.json::<Value>();
        assert_eq!(body["id"], json!(user.id.to_string()), "{path}");
        assert_eq!(body["username"], json!("ada"), "{path}");
        // The row is what a handler gets, and it never carries the two
        // internal columns (`SPEC.md`, "Users").
        assert_eq!(body.get("password_hash"), None, "{path}");
        assert_eq!(body.get("auth_version"), None, "{path}");
    }

    // The administrator route answers a list rather than the caller, so it is
    // asserted on its own; the only interesting part is that the row's `admin`
    // let the request through.
    let response = app.server.get(ADMIN).authorization_bearer(&token).await;
    response.assert_status(StatusCode::OK);
    let body = response.json::<Vec<Value>>();
    assert_eq!(body.len(), 1);
    assert_eq!(body[0]["id"], json!(user.id.to_string()));
    assert_eq!(body[0].get("password_hash"), None);
    assert_eq!(body[0].get("auth_version"), None);
}

#[tokio::test]
async fn a_request_without_an_authorization_header_is_401() {
    let app = TestApp::spawn().await;

    for path in [GATED_ANY, UNGATED, ADMIN] {
        let response = app.server.get(path).await;

        response.assert_status(StatusCode::UNAUTHORIZED);
        assert_eq!(response.json::<Value>(), unauthorized(), "{path}");
    }
}

#[tokio::test]
async fn a_header_that_is_not_a_bearer_token_is_401() {
    let app = TestApp::spawn().await;
    let user = app
        .insert_user("ada", "ada@example.test", false, false)
        .await;
    let token = app.token_for(&user);

    for header in [
        format!("Basic {token}"),
        format!("Token {token}"),
        "Bearer".to_string(),
        "Bearer ".to_string(),
        String::new(),
    ] {
        let response = app
            .server
            .get(&gated(&user))
            .add_header(AUTHORIZATION, header.clone())
            .await;

        response.assert_status(StatusCode::UNAUTHORIZED);
        assert_eq!(response.json::<Value>(), unauthorized(), "{header:?}");
    }
}

/// The scheme is a case-insensitive token, so a client that sends `bearer`
/// authenticates like any other.
#[tokio::test]
async fn the_bearer_scheme_is_matched_case_insensitively() {
    let app = TestApp::spawn().await;
    let user = app
        .insert_user("ada", "ada@example.test", false, false)
        .await;
    let token = app.token_for(&user);

    let response = app
        .server
        .get(&gated(&user))
        .add_header(AUTHORIZATION, format!("bEaReR {token}"))
        .await;

    response.assert_status(StatusCode::OK);
}

#[tokio::test]
async fn a_garbage_token_is_401() {
    let app = TestApp::spawn().await;

    for token in ["not-a-token", "a.b.c", "....."] {
        let response = app.server.get(GATED_ANY).authorization_bearer(token).await;

        response.assert_status(StatusCode::UNAUTHORIZED);
        assert_eq!(response.json::<Value>(), unauthorized(), "{token}");
    }
}

#[tokio::test]
async fn an_expired_token_is_401() {
    let app = TestApp::spawn().await;
    let user = app
        .insert_user("ada", "ada@example.test", true, false)
        .await;
    let token = app.expired_token_for(&user);

    for path in [gated(&user), UNGATED.to_string(), ADMIN.to_string()] {
        let response = app.server.get(&path).authorization_bearer(&token).await;

        response.assert_status(StatusCode::UNAUTHORIZED);
        assert_eq!(response.json::<Value>(), unauthorized(), "{path}");
    }
}

/// Deletion takes effect on the next request, with nothing revoked: the token
/// is still signed and still unexpired, and there is no user behind it.
#[tokio::test]
async fn a_token_for_a_deleted_user_is_401_on_the_next_request() {
    let app = TestApp::spawn().await;
    let user = app
        .insert_user("ada", "ada@example.test", false, false)
        .await;
    let token = app.token_for(&user);

    app.server
        .get(&gated(&user))
        .authorization_bearer(&token)
        .await
        .assert_status(StatusCode::OK);

    delete_user(&app, &user).await;

    let response = app
        .server
        .get(&gated(&user))
        .authorization_bearer(&token)
        .await;
    response.assert_status(StatusCode::UNAUTHORIZED);
    assert_eq!(response.json::<Value>(), unauthorized());
}

/// A password change increments `auth_version`, so every token minted before
/// it stops authenticating — the whole of revocation, without a blacklist.
#[tokio::test]
async fn a_token_one_auth_version_behind_the_row_is_401() {
    let app = TestApp::spawn().await;
    let user = app
        .insert_user("ada", "ada@example.test", false, false)
        .await;
    let token = app.token_for(&user);

    let changed = change_password(&app, &user).await;
    assert_eq!(changed.auth_version, user.auth_version + 1);

    for path in [gated(&user), UNGATED.to_string()] {
        let response = app.server.get(&path).authorization_bearer(&token).await;

        response.assert_status(StatusCode::UNAUTHORIZED);
        assert_eq!(response.json::<Value>(), unauthorized(), "{path}");
    }

    // The token for the new generation works, so this is the version check
    // and not the user having gone missing.
    app.server
        .get(&gated(&changed))
        .authorization_bearer(app.token_for(&changed))
        .await
        .assert_status(StatusCode::OK);
}

/// A token minted for a *later* generation is rejected just as flatly: the
/// comparison is equality, not "at least".
#[tokio::test]
async fn a_token_ahead_of_the_row_s_auth_version_is_401() {
    let app = TestApp::spawn().await;
    let user = app
        .insert_user("ada", "ada@example.test", false, false)
        .await;

    let mut claims = Claims::for_user(&user, Utc::now());
    claims.auth_version = user.auth_version + 1;

    let response = app
        .server
        .get(&gated(&user))
        .authorization_bearer(app.encode(&claims))
        .await;

    response.assert_status(StatusCode::UNAUTHORIZED);
    assert_eq!(response.json::<Value>(), unauthorized());
}

#[tokio::test]
async fn the_password_change_gate_is_403_everywhere_but_the_ungated_route() {
    let app = TestApp::spawn().await;
    let user = app
        .insert_user("ada", "ada@example.test", false, true)
        .await;
    let token = app.token_for(&user);

    let response = app
        .server
        .get(&gated(&user))
        .authorization_bearer(&token)
        .await;
    response.assert_status(StatusCode::FORBIDDEN);
    assert_eq!(
        response.json::<Value>(),
        forbidden("password change required")
    );

    // `GET /users/me` and `POST /users/{id}/password` are the documented
    // exceptions, and this is the extractor they take.
    let response = app.server.get(UNGATED).authorization_bearer(&token).await;
    response.assert_status(StatusCode::OK);
    assert_eq!(response.json::<Value>()["id"], json!(user.id.to_string()));
}

/// The gate reads the current row, so raising the flag after the token was
/// minted closes the routes on the next request.
#[tokio::test]
async fn the_gate_reads_the_row_rather_than_the_token_s_snapshot() {
    let app = TestApp::spawn().await;
    let user = app
        .insert_user("ada", "ada@example.test", false, true)
        .await;

    // A token whose snapshot says the user is free to go anywhere.
    let mut claims = Claims::for_user(&user, Utc::now());
    claims.must_change_password = false;

    let response = app
        .server
        .get(&gated(&user))
        .authorization_bearer(app.encode(&claims))
        .await;

    response.assert_status(StatusCode::FORBIDDEN);
    assert_eq!(
        response.json::<Value>(),
        forbidden("password change required")
    );
}

#[tokio::test]
async fn a_non_administrator_on_an_admin_route_is_403() {
    let app = TestApp::spawn().await;
    let user = app
        .insert_user("ada", "ada@example.test", false, false)
        .await;

    let response = app
        .server
        .get(ADMIN)
        .authorization_bearer(app.token_for(&user))
        .await;

    response.assert_status(StatusCode::FORBIDDEN);
    assert_eq!(response.json::<Value>(), forbidden("admin required"));
}

/// The `admin` claim is a display snapshot; the row decides (ADR 0025).
#[tokio::test]
async fn an_admin_claim_over_a_non_admin_row_is_403() {
    let app = TestApp::spawn().await;
    let user = app
        .insert_user("ada", "ada@example.test", false, false)
        .await;

    let mut claims = Claims::for_user(&user, Utc::now());
    claims.admin = true;

    let response = app
        .server
        .get(ADMIN)
        .authorization_bearer(app.encode(&claims))
        .await;

    response.assert_status(StatusCode::FORBIDDEN);
    assert_eq!(response.json::<Value>(), forbidden("admin required"));
}

/// Demotion takes effect on the next request and revokes nothing: the same
/// token still authenticates, it just no longer authorizes.
#[tokio::test]
async fn a_demotion_closes_the_admin_route_on_the_next_request() {
    let app = TestApp::spawn().await;
    let user = app
        .insert_user("ada", "ada@example.test", true, false)
        .await;
    let token = app.token_for(&user);

    app.server
        .get(ADMIN)
        .authorization_bearer(&token)
        .await
        .assert_status(StatusCode::OK);

    demote(&app, &user).await;

    let response = app.server.get(ADMIN).authorization_bearer(&token).await;
    response.assert_status(StatusCode::FORBIDDEN);
    assert_eq!(response.json::<Value>(), forbidden("admin required"));

    // Ordinary routes still work: a demotion is not a revocation.
    app.server
        .get(&gated(&user))
        .authorization_bearer(&token)
        .await
        .assert_status(StatusCode::OK);
}

/// Both checks failing: the gate runs first, so a gated administrator is told
/// to change their password rather than that they are not an administrator.
#[tokio::test]
async fn a_gated_administrator_is_told_about_the_password_not_the_role() {
    let app = TestApp::spawn().await;
    let user = app.insert_user("ada", "ada@example.test", true, true).await;

    let response = app
        .server
        .get(ADMIN)
        .authorization_bearer(app.token_for(&user))
        .await;

    response.assert_status(StatusCode::FORBIDDEN);
    assert_eq!(
        response.json::<Value>(),
        forbidden("password change required")
    );
}

/// Clear `admin` on `user`'s row.
///
/// Straight through the repository: the administrator-membership invariant is
/// the routes' composition, and these tests are about what the extractor reads,
/// not about how the row got that way.
async fn demote(app: &TestApp, user: &User) {
    let update = UserUpdate {
        admin: Some(false),
        ..UserUpdate::default()
    };

    let mut tx = app.pool.begin().await.expect("a transaction begins");
    UserRepository::new(&app.pool)
        .update(&mut tx, user.id, &update)
        .await
        .expect("the user updates")
        .expect("the user exists");
    tx.commit().await.expect("the transaction commits");
}

/// Run the password transaction against `user`, which increments
/// `auth_version`, and return the row it left behind.
async fn change_password(app: &TestApp, user: &User) -> User {
    /// Not a credential: an obviously fake replacement hash (rule 3).
    const NEW_FAKE_HASH: &str = "$argon2id$fake$newhash";

    let mut tx = app.pool.begin().await.expect("a transaction begins");
    let changed = UserRepository::new(&app.pool)
        .apply_password_change(&mut tx, user.id, NEW_FAKE_HASH, None)
        .await
        .expect("the password change applies");
    tx.commit().await.expect("the transaction commits");

    changed
}

async fn delete_user(app: &TestApp, user: &User) {
    let mut tx = app.pool.begin().await.expect("a transaction begins");
    assert!(
        UserRepository::new(&app.pool)
            .delete(&mut tx, user.id)
            .await
            .expect("the delete runs"),
        "the test user existed"
    );
    tx.commit().await.expect("the transaction commits");
}
