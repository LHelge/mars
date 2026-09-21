//! `/api/secrets` through the real router (`SPEC.md`, "Secrets
//! (`/api/secrets`)").
//!
//! The rules themselves — ownership, scopes, names, values, conflicts and the
//! audit limits — are asserted against the service in
//! `tests/secrets_service.rs` and are not repeated here. What this file
//! asserts is the adapter, and only what a service test cannot reach:
//!
//! - every endpoint needs a token and every endpoint applies the
//!   password-change gate and the ownership gate, which is the one thing a
//!   handler can get wrong on its own by building the [`Actor`] badly;
//! - a request body or query string the handler cannot read is a 400 in the
//!   documented shape, including a scope or a limit that is not one;
//! - each endpoint answers the documented status and the documented shape;
//! - one end-to-end create → list → delete over HTTP;
//! - and the one guarantee the whole module exists to keep — **no response
//!   ever contains `value`** — checked against the raw response text rather
//!   than against a parsed field, so a value smuggled into an unexpected key
//!   would still fail it.
//!
//! The agent-credential rules are the exception to "not repeated here": what
//! they promise is a status and an exact message a client is shown (`SPEC.md`,
//! "Secrets", Agent credentials), and `credential_for` exists only in a
//! response, so both are asserted over HTTP.
//!
//! Every value here is an obviously fake credential and no test prints one
//! (`CLAUDE.md`, rule 3).
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use std::future::IntoFuture;

use axum::http::StatusCode;
use axum_test::TestResponse;
use chrono::DateTime;
use common::{AuthenticatedUser, TestApp};
use mars_orchestrator::prelude::*;
use serde_json::{Value, json};
use uuid::Uuid;

/// The collection.
const SECRETS: &str = "/api/secrets";

/// Not a real credential: the value every test stores (rule 3).
const FAKE_VALUE: &str = "fake-value-not-a-credential";

/// Not a real credential either: the replacement `PUT` writes (rule 3).
const OTHER_FAKE_VALUE: &str = "another-fake-value-not-a-credential";

/// The eleven fields `SPEC.md`, "Secrets" gives `SecretMeta`, and no others.
///
/// `credential_for` is the derived one: it is no column, so a response that
/// left it out would be a `SecretMeta` the frontend cannot group by backend
/// (ADR 0036).
const META_FIELDS: [&str; 11] = [
    "id",
    "scope",
    "scope_id",
    "name",
    "orchestrator_only",
    "key_version",
    "created_by",
    "created_at",
    "updated_at",
    "last_used_at",
    "credential_for",
];

/// The four fields one `/uses` row carries.
const USE_FIELDS: [&str; 4] = ["session_id", "user_id", "purpose", "at"];

/// The documented 401 body (`SPEC.md`, "Authentication").
fn unauthorized() -> Value {
    json!({ "status": 401, "error": "authentication required" })
}

/// An obviously fake password of the length `POST /api/test/users` requires
/// (rule 3).
fn password(name: &str) -> String {
    format!("fake-password-{name}")
}

/// A signed-in ordinary user.
async fn user(app: &TestApp, name: &str) -> AuthenticatedUser {
    app.create_user(name, &format!("{name}@example.test"), &password(name))
        .await
}

/// A signed-in administrator.
async fn admin(app: &TestApp, name: &str) -> AuthenticatedUser {
    app.create_admin(name, &format!("{name}@example.test"), &password(name))
        .await
}

/// A create body, with the two fields most tests do not vary.
fn create_body(scope: &str, scope_id: Option<Uuid>, name: &str) -> Value {
    json!({
        "scope": scope,
        "scope_id": scope_id.map(|id| id.to_string()),
        "name": name,
        "value": FAKE_VALUE,
    })
}

/// Create a secret through the route and answer its metadata.
///
/// Asserts 201, so it is the arrangement step for the tests that are about
/// what happens to a secret that already exists; a test about creation posts
/// to [`SECRETS`] itself.
async fn seed_secret(
    app: &TestApp,
    caller: &AuthenticatedUser,
    scope: &str,
    scope_id: Option<Uuid>,
    name: &str,
) -> Value {
    let response = app
        .post_as(caller, SECRETS)
        .json(&create_body(scope, scope_id, name))
        .await;

    response.assert_status(StatusCode::CREATED);
    response.json::<Value>()
}

/// The id out of a metadata body.
fn id_of(meta: &Value) -> Uuid {
    meta["id"]
        .as_str()
        .expect("the metadata carries an id")
        .parse()
        .expect("the id is a uuid")
}

/// Assert `meta` is a `SecretMeta` as documented: the eleven fields, no more,
/// and timestamps that parse as RFC 3339.
#[track_caller]
fn assert_meta_shape(meta: &Value) {
    let object = meta.as_object().expect("the metadata is an object");

    let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
    keys.sort_unstable();
    let mut expected = META_FIELDS;
    expected.sort_unstable();
    assert_eq!(keys, expected, "unexpected SecretMeta fields");

    for field in ["created_at", "updated_at"] {
        let raw = object[field].as_str().unwrap_or_else(|| {
            panic!("{field} is a string, got {}", object[field]);
        });
        DateTime::parse_from_rfc3339(raw).unwrap_or_else(|err| {
            panic!("{field} is not RFC 3339: {err}");
        });
    }

    // Absent rather than omitted: the frontend's types mirror these fields.
    assert!(object["scope_id"].is_null() || object["scope_id"].is_string());
    assert!(object["created_by"].is_null() || object["created_by"].is_string());
    assert!(object["last_used_at"].is_null() || object["last_used_at"].is_string());
    assert!(
        object["credential_for"].is_null() || object["credential_for"].is_string(),
        "credential_for is a backend or null, got {}",
        object["credential_for"]
    );
}

/// Assert a 400 in the documented `{ status, error }` shape.
#[track_caller]
fn assert_bad_request(response: &TestResponse, about: &str) {
    assert_eq!(
        response.status_code(),
        StatusCode::BAD_REQUEST,
        "{about}: {}",
        response.text()
    );

    let error = response.json::<Value>();
    assert_eq!(error["status"], json!(400), "{about}: {error}");
    assert!(error["error"].is_string(), "{about}: {error}");
}

/// Seed `count` uses, one second apart, newest last.
///
/// The unchecked query rather than a repository call: this file introduces no
/// compile-time checked query, so `.sqlx/` never has to carry one for it
/// (`CLAUDE.md`, "Backend conventions").
async fn seed_uses(pool: &PgPool, secret_id: Uuid, user_id: Uuid, count: i32) {
    sqlx::query(
        "INSERT INTO secret_uses (secret_id, user_id, purpose, at)
         SELECT $1, $2, 'launch', NOW() - make_interval(secs => g)
         FROM generate_series(1, $3) AS g",
    )
    .bind(secret_id)
    .bind(user_id)
    .bind(count)
    .execute(pool)
    .await
    .expect("the uses insert");
}

// ---- who may call ----

#[tokio::test]
async fn every_endpoint_requires_a_token() {
    let app = TestApp::spawn().await;
    let ada = user(&app, "ada").await;
    let meta = seed_secret(&app, &ada, "global", None, "DEPLOY_TOKEN").await;
    let id = id_of(&meta);

    let responses = vec![
        app.server.get(SECRETS).await,
        app.server
            .post(SECRETS)
            .json(&create_body("global", None, "OTHER_TOKEN"))
            .await,
        app.server
            .put(&format!("{SECRETS}/{id}"))
            .json(&json!({ "value": OTHER_FAKE_VALUE }))
            .await,
        app.server
            .patch(&format!("{SECRETS}/{id}"))
            .json(&json!({ "name": "RENAMED_TOKEN" }))
            .await,
        app.server.delete(&format!("{SECRETS}/{id}")).await,
        app.server.get(&format!("{SECRETS}/{id}/uses")).await,
    ];

    for response in responses {
        response.assert_status(StatusCode::UNAUTHORIZED);
        assert_eq!(response.json::<Value>(), unauthorized());
    }
}

#[tokio::test]
async fn the_password_change_gate_applies_to_every_endpoint() {
    let app = TestApp::spawn().await;
    let ada = user(&app, "ada").await;
    let meta = seed_secret(&app, &ada, "global", None, "DEPLOY_TOKEN").await;
    let id = id_of(&meta);
    let gated = app.create_gated_user("gated", "gated@example.test").await;

    let responses = vec![
        app.get_as(&gated, SECRETS).await,
        app.post_as(&gated, SECRETS)
            .json(&create_body("global", None, "OTHER_TOKEN"))
            .await,
        app.put_as(&gated, &format!("{SECRETS}/{id}"))
            .json(&json!({ "value": OTHER_FAKE_VALUE }))
            .await,
        app.patch_as(&gated, &format!("{SECRETS}/{id}"))
            .json(&json!({ "name": "RENAMED_TOKEN" }))
            .await,
        app.delete_as(&gated, &format!("{SECRETS}/{id}")).await,
        app.get_as(&gated, &format!("{SECRETS}/{id}/uses")).await,
    ];

    for response in responses {
        response.assert_status(StatusCode::FORBIDDEN);
        assert_eq!(
            response.json::<Value>(),
            json!({ "status": 403, "error": "password change required" })
        );
    }
}

#[tokio::test]
async fn every_endpoint_applies_the_ownership_gate_and_an_administrator_passes_it() {
    // The rule is `secrets_service.rs`'s; what is asserted here is that each
    // handler builds the `Actor` from the row it loaded and passes it down —
    // so an administrator reaches the same secret every other user is refused
    // (ADR 0025; `src/routes/secrets.rs`).
    let app = TestApp::spawn().await;
    let ada = user(&app, "ada").await;
    let bob = user(&app, "bob").await;
    let root = admin(&app, "root").await;

    let meta = seed_secret(&app, &ada, "user", None, "DEPLOY_TOKEN").await;
    let id = id_of(&meta);
    let owned = format!("{SECRETS}?scope=user&scope_id={}", ada.user.id);

    let refused = vec![
        app.get_as(&bob, &owned).await,
        app.post_as(&bob, SECRETS)
            .json(&create_body("user", Some(ada.user.id), "OTHER_TOKEN"))
            .await,
        app.put_as(&bob, &format!("{SECRETS}/{id}"))
            .json(&json!({ "value": OTHER_FAKE_VALUE }))
            .await,
        app.patch_as(&bob, &format!("{SECRETS}/{id}"))
            .json(&json!({ "name": "BOBS_TOKEN" }))
            .await,
        app.delete_as(&bob, &format!("{SECRETS}/{id}")).await,
        app.get_as(&bob, &format!("{SECRETS}/{id}/uses")).await,
    ];

    for response in refused {
        response.assert_status(StatusCode::FORBIDDEN);
        assert_eq!(response.json::<Value>()["status"], json!(403));
    }

    // The same two reads as the administrator, who may.
    let response = app.get_as(&root, &owned).await;
    response.assert_status(StatusCode::OK);
    assert_eq!(response.json::<Vec<Value>>().len(), 1);

    let response = app.get_as(&root, &format!("{SECRETS}/{id}/uses")).await;
    response.assert_status(StatusCode::OK);
}

// ---- bodies and query strings the handler cannot read ----

#[tokio::test]
async fn a_body_or_query_the_handler_cannot_read_is_400() {
    let app = TestApp::spawn().await;
    let ada = user(&app, "ada").await;
    let meta = seed_secret(&app, &ada, "user", None, "DEPLOY_TOKEN").await;
    let id = id_of(&meta);

    let bodies = vec![
        // Not one of the three scopes.
        create_body("team", None, "DEPLOY_TOKEN"),
        // Missing the two fields that have no default.
        json!({ "scope": "global", "name": "DEPLOY_TOKEN" }),
        // The server's own columns are not the client's to send
        // (`deny_unknown_fields`).
        json!({
            "scope": "global",
            "name": "DEPLOY_TOKEN",
            "value": FAKE_VALUE,
            "key_version": 1,
        }),
    ];
    for body in bodies {
        let response = app.post_as(&ada, SECRETS).json(&body).await;
        assert_bad_request(&response, &format!("POST {body}"));
    }

    // A value is `PUT`'s field and a name is `PATCH`'s; neither endpoint
    // silently ignores the other's.
    let response = app
        .patch_as(&ada, &format!("{SECRETS}/{id}"))
        .json(&json!({ "value": OTHER_FAKE_VALUE }))
        .await;
    assert_bad_request(&response, "PATCH with a value");

    for query in ["?scope=team", "?scope_id=not-a-uuid"] {
        let response = app.get_as(&ada, &format!("{SECRETS}{query}")).await;
        assert_bad_request(&response, query);
    }

    let response = app
        .get_as(&ada, &format!("{SECRETS}/{id}/uses?limit=lots"))
        .await;
    assert_bad_request(&response, "?limit=lots");
}

#[tokio::test]
async fn a_malformed_uuid_in_the_path_is_400_in_the_documented_shape() {
    let app = TestApp::spawn().await;
    let ada = user(&app, "ada").await;

    for path in [
        format!("{SECRETS}/not-a-uuid"),
        format!("{SECRETS}/not-a-uuid/uses"),
    ] {
        let response = app.get_as(&ada, &path).await;
        // `GET /secrets/{id}` is not a route; the uses read is.
        if response.status_code() == StatusCode::METHOD_NOT_ALLOWED {
            continue;
        }

        assert_bad_request(&response, &path);
    }

    let response = app.delete_as(&ada, &format!("{SECRETS}/not-a-uuid")).await;
    assert_bad_request(&response, "DELETE with a malformed id");
}

// ---- the shape and status of each endpoint ----

#[tokio::test]
async fn creating_a_secret_answers_201_and_the_documented_metadata() {
    let app = TestApp::spawn().await;
    let ada = user(&app, "ada").await;

    let response = app
        .post_as(&ada, SECRETS)
        .json(&create_body("global", None, "DEPLOY_TOKEN"))
        .await;

    response.assert_status(StatusCode::CREATED);
    let meta = response.json::<Value>();
    assert_meta_shape(&meta);

    assert_eq!(meta["scope"], json!("global"));
    assert_eq!(meta["scope_id"], Value::Null);
    assert_eq!(meta["name"], json!("DEPLOY_TOKEN"));
    // Omitted on create means false (`SPEC.md`, "Secrets").
    assert_eq!(meta["orchestrator_only"], json!(false));
    assert_eq!(meta["created_by"], json!(ada.user.id.to_string()));
    assert_eq!(meta["last_used_at"], Value::Null);
    assert_eq!(
        meta["key_version"],
        json!(app.state.keyring.current_version())
    );
}

#[tokio::test]
async fn replacing_a_value_answers_200_and_the_documented_metadata() {
    let app = TestApp::spawn().await;
    let ada = user(&app, "ada").await;
    let meta = seed_secret(&app, &ada, "user", None, "DEPLOY_TOKEN").await;
    let id = id_of(&meta);

    let response = app
        .put_as(&ada, &format!("{SECRETS}/{id}"))
        .json(&json!({ "value": OTHER_FAKE_VALUE }))
        .await;

    response.assert_status(StatusCode::OK);
    let updated = response.json::<Value>();
    assert_meta_shape(&updated);
    assert_eq!(updated["id"], meta["id"]);
    assert_eq!(updated["name"], json!("DEPLOY_TOKEN"));
    assert_eq!(updated["scope_id"], json!(ada.user.id.to_string()));
}

#[tokio::test]
async fn patching_answers_200_and_the_documented_metadata_for_an_empty_body_too() {
    let app = TestApp::spawn().await;
    let ada = user(&app, "ada").await;
    let meta = seed_secret(&app, &ada, "user", None, "DEPLOY_TOKEN").await;
    let id = id_of(&meta);

    let response = app
        .patch_as(&ada, &format!("{SECRETS}/{id}"))
        .json(&json!({ "name": "RENAMED_TOKEN", "orchestrator_only": true }))
        .await;

    response.assert_status(StatusCode::OK);
    let patched = response.json::<Value>();
    assert_meta_shape(&patched);
    assert_eq!(patched["name"], json!("RENAMED_TOKEN"));
    assert_eq!(patched["orchestrator_only"], json!(true));

    // `{}` is legal and answers the current metadata unchanged.
    let response = app
        .patch_as(&ada, &format!("{SECRETS}/{id}"))
        .json(&json!({}))
        .await;

    response.assert_status(StatusCode::OK);
    let unchanged = response.json::<Value>();
    assert_meta_shape(&unchanged);
    assert_eq!(unchanged, patched);
}

#[tokio::test]
async fn a_use_is_answered_in_the_documented_four_fields() {
    let app = TestApp::spawn().await;
    let ada = user(&app, "ada").await;
    let meta = seed_secret(&app, &ada, "user", None, "DEPLOY_TOKEN").await;
    let id = id_of(&meta);
    seed_uses(&app.pool, id, ada.user.id, 3).await;

    let response = app.get_as(&ada, &format!("{SECRETS}/{id}/uses")).await;
    response.assert_status(StatusCode::OK);
    let uses = response.json::<Vec<Value>>();
    assert_eq!(uses.len(), 3);

    let mut keys: Vec<&str> = uses[0]
        .as_object()
        .expect("a use is an object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    let mut expected = USE_FIELDS;
    expected.sort_unstable();
    assert_eq!(keys, expected, "unexpected use fields");

    assert_eq!(uses[0]["purpose"], json!("launch"));
    assert_eq!(uses[0]["user_id"], json!(ada.user.id.to_string()));
    assert_eq!(uses[0]["session_id"], Value::Null);
    DateTime::parse_from_rfc3339(uses[0]["at"].as_str().expect("a timestamp"))
        .expect("at is RFC 3339");

    // `?limit=` reaches the service, which decides the range.
    let response = app
        .get_as(&ada, &format!("{SECRETS}/{id}/uses?limit=2"))
        .await;
    response.assert_status(StatusCode::OK);
    assert_eq!(response.json::<Vec<Value>>().len(), 2);
}

// ---- end to end ----

#[tokio::test]
async fn a_secret_is_created_listed_and_deleted_over_http() {
    let app = TestApp::spawn().await;
    let ada = user(&app, "ada").await;

    let created = seed_secret(&app, &ada, "user", None, "DEPLOY_TOKEN").await;
    let id = id_of(&created);

    let response = app.get_as(&ada, SECRETS).await;
    response.assert_status(StatusCode::OK);
    let listing = response.json::<Vec<Value>>();
    assert_eq!(listing.len(), 1);
    assert_meta_shape(&listing[0]);
    assert_eq!(listing[0], created);

    let response = app.delete_as(&ada, &format!("{SECRETS}/{id}")).await;
    response.assert_status(StatusCode::NO_CONTENT);
    assert!(response.text().is_empty(), "204 carries no body");

    let response = app.get_as(&ada, SECRETS).await;
    response.assert_status(StatusCode::OK);
    assert!(response.json::<Vec<Value>>().is_empty());
}

// ---- values ----

#[tokio::test]
async fn no_success_response_of_any_endpoint_carries_the_value() {
    let app = TestApp::spawn().await;
    let ada = user(&app, "ada").await;

    // Every success of every endpoint, in one pass: create, list, uses,
    // replace, patch and delete.
    let created = app
        .post_as(&ada, SECRETS)
        .json(&create_body("user", None, "DEPLOY_TOKEN"))
        .await;
    created.assert_status(StatusCode::CREATED);
    let id = id_of(&created.json::<Value>());
    seed_uses(&app.pool, id, ada.user.id, 1).await;

    let responses = vec![
        ("create", created),
        ("list", app.get_as(&ada, SECRETS).await),
        (
            "uses",
            app.get_as(&ada, &format!("{SECRETS}/{id}/uses")).await,
        ),
        (
            "replace",
            app.put_as(&ada, &format!("{SECRETS}/{id}"))
                .json(&json!({ "value": OTHER_FAKE_VALUE }))
                .await,
        ),
        (
            "patch",
            app.patch_as(&ada, &format!("{SECRETS}/{id}"))
                .json(&json!({ "name": "RENAMED_TOKEN" }))
                .await,
        ),
        (
            "delete",
            app.delete_as(&ada, &format!("{SECRETS}/{id}")).await,
        ),
    ];

    for (endpoint, response) in responses {
        assert!(
            response.status_code().is_success(),
            "{endpoint}: {}",
            response.status_code()
        );

        let text = response.text();
        assert!(
            !text.contains(FAKE_VALUE) && !text.contains(OTHER_FAKE_VALUE),
            "{endpoint} leaked a stored value"
        );
        assert!(
            !text.contains("\"value\""),
            "{endpoint} carries a value key: {text}"
        );
    }
}

// ---- agent credentials ----
//
// The three write rules of `SPEC.md`, "Secrets", Agent credentials, over HTTP,
// because their whole point is the status and the message a client is given
// (ADR 0036). Every value here is an obviously fake credential (rule 3).

/// The Claude backend's two credential names, in the adapter's order.
const OAUTH_TOKEN: &str = "CLAUDE_CODE_OAUTH_TOKEN";
const API_KEY: &str = "ANTHROPIC_API_KEY";

/// Assert a response is the documented `{ status, error }` with this status,
/// and answer the message.
#[track_caller]
fn error_message(response: &TestResponse, status: StatusCode, about: &str) -> String {
    assert_eq!(
        response.status_code(),
        status,
        "{about}: {}",
        response.text()
    );

    let body = response.json::<Value>();
    assert_eq!(body["status"], json!(status.as_u16()), "{about}: {body}");

    body["error"]
        .as_str()
        .unwrap_or_else(|| panic!("{about}: no error message in {body}"))
        .to_string()
}

#[tokio::test]
async fn a_credential_reports_the_backend_it_authenticates() {
    let app = TestApp::spawn().await;
    let ada = user(&app, "ada").await;

    let credential = seed_secret(&app, &ada, "user", None, OAUTH_TOKEN).await;
    assert_meta_shape(&credential);
    assert_eq!(credential["credential_for"], json!("claude"));

    let ordinary = seed_secret(&app, &ada, "global", None, "DEPLOY_TOKEN").await;
    assert_eq!(ordinary["credential_for"], Value::Null);

    // And on every other response that carries a `SecretMeta`.
    let id = id_of(&credential);
    let listing = app.get_as(&ada, SECRETS).await.json::<Vec<Value>>();
    for meta in &listing {
        assert_meta_shape(meta);
    }
    let renamed = app
        .patch_as(&ada, &format!("{SECRETS}/{id}"))
        .json(&json!({ "name": API_KEY }))
        .await;
    renamed.assert_status(StatusCode::OK);
    assert_eq!(renamed.json::<Value>()["credential_for"], json!("claude"));
}

#[tokio::test]
async fn a_second_credential_at_one_scope_is_refused_and_names_the_first() {
    let app = TestApp::spawn().await;
    let ada = user(&app, "ada").await;

    seed_secret(&app, &ada, "user", None, OAUTH_TOKEN).await;

    let response = app
        .post_as(&ada, SECRETS)
        .json(&create_body("user", None, API_KEY))
        .await;
    let message = error_message(&response, StatusCode::CONFLICT, "the other credential name");
    assert_eq!(
        message,
        format!(
            "this scope already has an agent credential ({OAUTH_TOKEN}); replace or delete it first"
        ),
    );

    // The same name at the same scope is the plain "already exists" 409 the
    // endpoint has always answered.
    let response = app
        .post_as(&ada, SECRETS)
        .json(&create_body("user", None, OAUTH_TOKEN))
        .await;
    let message = error_message(&response, StatusCode::CONFLICT, "the same credential name");
    assert_eq!(message, "secret already exists");

    // A different scope is the whole resolution model and is not a conflict.
    let response = app
        .post_as(&ada, SECRETS)
        .json(&create_body("global", None, API_KEY))
        .await;
    response.assert_status(StatusCode::CREATED);
}

#[tokio::test]
async fn a_rename_into_an_occupied_scope_is_refused_and_the_row_is_untouched() {
    let app = TestApp::spawn().await;
    let ada = user(&app, "ada").await;

    seed_secret(&app, &ada, "user", None, OAUTH_TOKEN).await;
    let ordinary = seed_secret(&app, &ada, "user", None, "DEPLOY_TOKEN").await;
    let id = id_of(&ordinary);

    let response = app
        .patch_as(&ada, &format!("{SECRETS}/{id}"))
        .json(&json!({ "name": API_KEY }))
        .await;
    let message = error_message(&response, StatusCode::CONFLICT, "renamed into the slot");
    assert!(message.contains(OAUTH_TOKEN), "{message}");

    // Nothing was written, so the secret is still the one it was.
    let listing = app.get_as(&ada, SECRETS).await.json::<Vec<Value>>();
    let mut names: Vec<&str> = listing
        .iter()
        .map(|meta| meta["name"].as_str().expect("a name"))
        .collect();
    names.sort_unstable();
    assert_eq!(names, vec![OAUTH_TOKEN, "DEPLOY_TOKEN"]);

    // Renaming a credential to the other name at its own scope is allowed:
    // the row it would collide with is itself.
    let credential = listing
        .iter()
        .find(|meta| meta["name"] == json!(OAUTH_TOKEN))
        .expect("the credential is listed");
    let response = app
        .patch_as(&ada, &format!("{SECRETS}/{}", id_of(credential)))
        .json(&json!({ "name": API_KEY }))
        .await;
    response.assert_status(StatusCode::OK);
    assert_eq!(response.json::<Value>()["name"], json!(API_KEY));
}

#[tokio::test]
async fn a_credential_cannot_be_orchestrator_only_on_create_or_on_patch() {
    let app = TestApp::spawn().await;
    let ada = user(&app, "ada").await;

    const REFUSED: &str = "an agent credential cannot be orchestrator-only";

    // On create.
    let response = app
        .post_as(&ada, SECRETS)
        .json(&json!({
            "scope": "user",
            "name": API_KEY,
            "value": FAKE_VALUE,
            "orchestrator_only": true,
        }))
        .await;
    assert_eq!(
        error_message(&response, StatusCode::BAD_REQUEST, "on create"),
        REFUSED
    );

    // On patch, by flagging a credential.
    let credential = seed_secret(&app, &ada, "user", None, API_KEY).await;
    let response = app
        .patch_as(&ada, &format!("{SECRETS}/{}", id_of(&credential)))
        .json(&json!({ "orchestrator_only": true }))
        .await;
    assert_eq!(
        error_message(&response, StatusCode::BAD_REQUEST, "flagging a credential"),
        REFUSED
    );

    // And on patch, by renaming an orchestrator-only secret into one.
    let hidden = app
        .post_as(&ada, SECRETS)
        .json(&json!({
            "scope": "global",
            "name": "DEPLOY_TOKEN",
            "value": FAKE_VALUE,
            "orchestrator_only": true,
        }))
        .await;
    hidden.assert_status(StatusCode::CREATED);
    let response = app
        .patch_as(
            &ada,
            &format!("{SECRETS}/{}", id_of(&hidden.json::<Value>())),
        )
        .json(&json!({ "name": OAUTH_TOKEN }))
        .await;
    assert_eq!(
        error_message(&response, StatusCode::BAD_REQUEST, "renamed into one"),
        REFUSED
    );
}

#[tokio::test]
async fn two_simultaneous_credentials_at_one_scope_leave_exactly_one_row() {
    let app = TestApp::spawn().await;
    let ada = user(&app, "ada").await;

    // The check and the insert are one transaction, but two of them can still
    // interleave; `secrets_claude_credential_idx` is what settles it, and the
    // loser is the same 409 rather than a 500 (`docs/data-model.md`).
    let (first, second) = tokio::join!(
        app.post_as(&ada, SECRETS)
            .json(&create_body("user", None, OAUTH_TOKEN))
            .into_future(),
        app.post_as(&ada, SECRETS)
            .json(&create_body("user", None, API_KEY))
            .into_future(),
    );

    let mut statuses = [first.status_code(), second.status_code()];
    statuses.sort_unstable_by_key(StatusCode::as_u16);
    assert_eq!(
        statuses,
        [StatusCode::CREATED, StatusCode::CONFLICT],
        "one create wins and the other is a conflict: {} / {}",
        first.text(),
        second.text()
    );

    let loser = if first.status_code() == StatusCode::CONFLICT {
        &first
    } else {
        &second
    };
    assert!(
        loser.json::<Value>()["error"]
            .as_str()
            .expect("an error message")
            .contains("already has an agent credential"),
        "{}",
        loser.text()
    );

    let listing = app.get_as(&ada, SECRETS).await.json::<Vec<Value>>();
    assert_eq!(listing.len(), 1, "{listing:?}");
    assert_eq!(listing[0]["credential_for"], json!("claude"));
}
