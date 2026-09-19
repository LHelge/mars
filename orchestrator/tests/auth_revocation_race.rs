//! Credential issuance racing revocation (`SPEC.md`, "Authentication",
//! bullet 5; `docs/data-model.md`, "Users and authentication"; ADR 0025).
//!
//! The contract these tests exist for is a *negative* one, and it is the only
//! one in the authentication epic that a single-threaded test cannot reach:
//!
//! > Login, refresh, reset-link issuance and password mutations serialize on
//! > the user row and revalidate credentials after locking, so a concurrent
//! > refresh or login cannot escape revocation with an old credential.
//!
//! > Thus a refresh either commits before a password change and is revoked by
//! > it, or observes revocation and fails.
//!
//! Two outcomes are therefore legitimate and one is not. A refresh that
//! commits first may well answer 200 — and the pair it hands out must be dead
//! by the time the password change commits. A refresh that arrives second must
//! answer 401. What must never exist is a live credential minted under the old
//! `auth_version`, and that is what every assertion below is aimed at: the
//! status code is never the whole assertion, the state of the credential
//! afterwards is.
//!
//! Each scenario appears in a deterministic form — the user row held under
//! `SELECT ... FOR UPDATE` while the request is in flight, so the request
//! provably reaches its lock after the change committed — and the refresh race
//! additionally in a repeated form that leaves the interleaving to the runtime.
//!
//! Every password here is obviously fake (`CLAUDE.md`, rule 3).
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use axum::http::StatusCode;
use axum::http::header::SET_COOKIE;
use axum_test::TestResponse;
use common::TestApp;
use common::app::TokenPair;
use common::races::{
    IN_FLIGHT, RACE_ITERATIONS, RACE_TIMEOUT, hold_user_lock, unrevoked_refresh_tokens,
};
use mars_orchestrator::auth::REFRESH_COOKIE;
use mars_orchestrator::models::User;
use mars_orchestrator::models::user::hash_password;
use mars_orchestrator::prelude::*;
use mars_orchestrator::repositories::UserRepository;
use serde_json::{Value, json};
use tokio::time::{sleep, timeout};
use uuid::Uuid;

const LOGIN: &str = "/api/auth/login";

/// A route behind `CurrentUser`: a 200 means the access token still
/// authenticates, a 401 means it does not.
const ME: &str = "/api/users/me";

/// The user every test here races against.
const USERNAME: &str = "ada";

/// Obviously fake, and inside the documented 10–128 range.
const PASSWORD: &str = "correct-horse-battery-staple";

/// The replacement, equally fake.
const NEW_PASSWORD: &str = "a-completely-different-phrase";

/// `POST /users/{id}/password` for `id`.
fn password_route(id: Uuid) -> String {
    format!("/api/users/{id}/password")
}

/// The documented 401 body for a failed refresh (`SPEC.md`,
/// "Authentication").
fn unauthorized() -> Value {
    json!({ "status": 401, "error": "authentication required" })
}

/// What a failed login answers, for an unknown name and a wrong password
/// alike.
fn invalid_credentials() -> Value {
    json!({ "status": 401, "error": "invalid username or password" })
}

/// The `Set-Cookie` a rejected refresh sends, as `tests/auth_refresh.rs`
/// pins it.
const CLEARING_SET_COOKIE: &str = "refresh_token=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0";

/// Assert that `response` is the documented 401 *and* clears the cookie.
fn assert_rejected_and_cleared(response: &TestResponse) {
    response.assert_status(StatusCode::UNAUTHORIZED);
    assert_eq!(response.json::<Value>(), unauthorized());

    let header = response
        .headers()
        .get(SET_COOKIE)
        .expect("a rejected refresh clears the cookie")
        .to_str()
        .expect("an ASCII header");
    assert_eq!(header, CLEARING_SET_COOKIE);
}

/// The row as the database has it now, whatever a response claimed.
async fn stored(app: &TestApp, id: Uuid) -> User {
    UserRepository::new(&app.pool)
        .find(id)
        .await
        .expect("the lookup succeeds")
        .expect("the user is still there")
}

/// Apply a password change straight through the repository inside `tx`, the
/// way an administrator's change and a reset link do: no replacement token, so
/// nothing is left unrevoked (`SPEC.md`, "Authentication").
///
/// Hashing the new password here rather than passing a stand-in keeps the row
/// in the state a real change leaves it in, so the login that revalidates
/// against it is doing real work.
async fn change_password_under_lock(
    app: &TestApp,
    tx: &mut sqlx::PgConnection,
    id: Uuid,
    password: &str,
) {
    let hash = hash_password(password).expect("the new password hashes");
    UserRepository::new(&app.pool)
        .apply_password_change(tx, id, &hash, None)
        .await
        .expect("the password change applies");
}

/// A refresh that reaches its lock *after* a password change committed sees
/// the revocation and fails, rather than rotating the token the change had
/// already revoked.
#[tokio::test]
async fn a_refresh_that_waits_for_a_password_change_is_rejected_and_cleared() {
    let app = TestApp::spawn().await;
    let user = app
        .insert_user_with_password(USERNAME, "ada@example.test", PASSWORD, false, false)
        .await;
    let (_, cookie) = app.login(USERNAME, PASSWORD).await;

    // Taken before the request starts, so `rotate` blocks in `lock_user` —
    // after it has located the token row unlocked, and before it reads
    // `revoked_at` for the decision.
    let mut holder = hold_user_lock(&app.pool, user.id).await;

    let refreshing = app.refresh(&cookie);
    let changing = async {
        sleep(IN_FLIGHT).await;
        change_password_under_lock(&app, &mut holder, user.id, NEW_PASSWORD).await;
        holder.commit().await.expect("the holder commits");
    };

    let (response, ()) = timeout(RACE_TIMEOUT, async { tokio::join!(refreshing, changing) })
        .await
        .expect("the refresh and the password change did not deadlock");

    // The authoritative read under the lock found the token revoked.
    assert_rejected_and_cleared(&response);

    // Nothing was rotated into existence on the way out: an administrator-style
    // change issues no replacement, so the user holds no usable token at all.
    assert_eq!(unrevoked_refresh_tokens(&app.pool, user.id).await, 0);
    assert_eq!(stored(&app, user.id).await.auth_version, 1);
}

/// The mirrored order: the refresh commits first, so its 200 is legitimate —
/// and the password change that follows revokes both halves of the pair it
/// handed out.
#[tokio::test]
async fn a_refresh_that_commits_first_is_revoked_by_the_change() {
    let app = TestApp::spawn().await;
    let user = app
        .insert_user_with_password(USERNAME, "ada@example.test", PASSWORD, false, false)
        .await;
    let (_, cookie) = app.login(USERNAME, PASSWORD).await;

    // Nothing holds the row, so this one runs to completion and mints a pair
    // under the current `auth_version`.
    let rotated = app.refresh(&cookie).await;
    rotated.assert_status_ok();
    let rotated_cookie = rotated.cookie(REFRESH_COOKIE).value().to_string();
    let rotated_token = rotated.json::<TokenPair>().access_token;

    // Then the self-service change, through the route, using the pair the
    // refresh just issued: the browser that refreshed is the one changing its
    // password.
    let changed = app
        .server
        .post(&password_route(user.id))
        .authorization_bearer(&rotated_token)
        .json(&json!({ "current_password": PASSWORD, "password": NEW_PASSWORD }))
        .await;
    changed.assert_status_ok();
    let replacement = changed.cookie(REFRESH_COOKIE).value().to_string();

    // Neither half of the refresh's pair survives, which is the whole point:
    // committing first buys a credential, not an exemption.
    app.server
        .get(ME)
        .authorization_bearer(&rotated_token)
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    assert_rejected_and_cleared(&app.refresh(&rotated_cookie).await);

    // Exactly one token is left: the replacement the change inserted inside
    // its own transaction.
    assert_eq!(unrevoked_refresh_tokens(&app.pool, user.id).await, 1);
    app.refresh(&replacement).await.assert_status_ok();
}

/// The same race without a lock, repeated: whichever order the runtime picks,
/// a token the refresh handed out is never usable once the change has
/// committed.
///
/// The 200 is not the assertion. A refresh that commits first is entitled to
/// one; what is asserted is that the access token inside it carries an
/// `auth_version` the row has already moved past, that the route rejects it,
/// and that the administrator's change left the user with nothing unrevoked.
#[tokio::test]
async fn a_refresh_racing_an_administrator_password_change_never_survives_it() {
    let app = TestApp::spawn().await;
    let user = app
        .insert_user_with_password(USERNAME, "ada@example.test", PASSWORD, false, false)
        .await;
    let admin = app
        .create_admin("root", "root@example.test", PASSWORD)
        .await;

    let mut password = PASSWORD.to_string();

    for iteration in 0..RACE_ITERATIONS {
        // A fresh cookie each round, under whatever password the previous
        // round left behind.
        let (_, cookie) = app.login(USERNAME, &password).await;
        let next = format!("race-password-{iteration:02}-fake");

        let refreshing = app.refresh(&cookie);
        let changing = app
            .post_as(&admin, &password_route(user.id))
            .json(&json!({ "password": next }));

        let (refreshed, changed) =
            timeout(RACE_TIMEOUT, async { tokio::join!(refreshing, changing) })
                .await
                .expect("the refresh and the password change did not deadlock");

        // The administrator's change is never the one that loses: it takes the
        // lock and writes, whatever the refresh is doing.
        changed.assert_status(StatusCode::NO_CONTENT);

        let row = stored(&app, user.id).await;
        assert_eq!(row.auth_version, iteration as i64 + 1);

        if refreshed.status_code() == StatusCode::OK {
            let access_token = refreshed.json::<TokenPair>().access_token;
            let claims =
                Claims::decode(&access_token, &app.state.config).expect("the token verifies");

            // It was minted before the change, and the row says so.
            assert!(
                claims.auth_version < row.auth_version,
                "a refresh answered 200 with the current auth_version {} \
                 after the change committed",
                claims.auth_version
            );

            // And the version check is what the router actually enforces.
            app.server
                .get(ME)
                .authorization_bearer(&access_token)
                .await
                .assert_status(StatusCode::UNAUTHORIZED);
        } else {
            assert_rejected_and_cleared(&refreshed);
        }

        // Either way: the refresh's replacement, if it inserted one, was
        // revoked by the change, and the change issued none of its own.
        assert_eq!(unrevoked_refresh_tokens(&app.pool, user.id).await, 0);

        password = next;
    }
}

/// A login whose password verification passed before the row lock re-verifies
/// under the lock, and answers 401 when the hash changed underneath it.
///
/// The verification is deliberately outside the lock — Argon2 is expensive —
/// so this is the case that rule exists for: "an earlier password check must be
/// revalidated against the locked row before issuing credentials"
/// (`docs/data-model.md`).
#[tokio::test]
async fn a_login_that_waits_for_a_password_change_is_rejected() {
    let app = TestApp::spawn().await;
    let user = app
        .insert_user_with_password(USERNAME, "ada@example.test", PASSWORD, false, false)
        .await;

    let mut holder = hold_user_lock(&app.pool, user.id).await;

    // The password is the right one when this request starts, and the wrong
    // one by the time it reaches the locked row.
    let logging_in = app
        .server
        .post(LOGIN)
        .json(&json!({ "username": USERNAME, "password": PASSWORD }));
    let changing = async {
        sleep(IN_FLIGHT).await;
        change_password_under_lock(&app, &mut holder, user.id, NEW_PASSWORD).await;
        holder.commit().await.expect("the holder commits");
    };

    let (response, ()) = timeout(RACE_TIMEOUT, async { tokio::join!(logging_in, changing) })
        .await
        .expect("the login and the password change did not deadlock");

    response.assert_status(StatusCode::UNAUTHORIZED);
    assert_eq!(response.json::<Value>(), invalid_credentials());
    assert_eq!(response.maybe_cookie(REFRESH_COOKIE), None);

    // No credential escaped: the login inserted nothing, and the change
    // revoked the nothing that was there.
    assert_eq!(unrevoked_refresh_tokens(&app.pool, user.id).await, 0);

    // The new password is the live one, so the rejection was about the change
    // rather than about a broken row.
    app.reset_limiters();
    app.login(USERNAME, NEW_PASSWORD).await;
}
