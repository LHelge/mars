//! `/api/secrets` through the real router (`SPEC.md`, "Secrets
//! (`/api/secrets`)").
//!
//! The rules themselves — ownership, scopes, renaming, the audit limits — are
//! asserted against the service in `tests/secrets_service.rs`. What is
//! asserted here is the adapter: the status of every success and of every
//! documented failure, the exact shape of `SecretMeta` and of one audit row,
//! the JWT requirement and the password-change gate on all six endpoints, and
//! the one guarantee the whole module exists to keep — **no response ever
//! contains `value`** — checked against the raw response text rather than
//! against a parsed field, so a value smuggled into an unexpected key would
//! still fail it.
//!
//! Every value here is an obviously fake credential and no test prints one
//! (`CLAUDE.md`, rule 3).
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use axum::http::StatusCode;
use chrono::DateTime;
use common::{AuthenticatedUser, TestApp};
use mars_orchestrator::models::NewProject;
use mars_orchestrator::prelude::*;
use mars_orchestrator::repositories::ProjectRepository;
use serde_json::{Value, json};
use uuid::Uuid;

/// The collection.
const SECRETS: &str = "/api/secrets";

/// Not a real credential: the value every test stores (rule 3).
const FAKE_VALUE: &str = "fake-value-not-a-credential";

/// Not a real credential either: the replacement `PUT` writes (rule 3).
const OTHER_FAKE_VALUE: &str = "another-fake-value-not-a-credential";

/// Not a real remote: the fixture the project tests use (rule 3).
const TEST_REMOTE: &str = "https://git.example.com/fake/repo.git";

/// The ten fields `SPEC.md`, "Secrets" gives `SecretMeta`, and no others.
const META_FIELDS: [&str; 10] = [
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

/// A committed project for the `project` scope to point at.
async fn seed_project(pool: &PgPool, project_name: &str) -> Uuid {
    let project = NewProject::new(project_name, TEST_REMOTE).expect("the test project is valid");
    let mut tx = pool.begin().await.expect("a transaction begins");
    let inserted = ProjectRepository::new(pool)
        .insert(&mut tx, &project)
        .await
        .expect("the project inserts");
    tx.commit().await.expect("the transaction commits");

    inserted.id
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

/// Assert `meta` is a `SecretMeta` as documented: the ten fields, no more, and
/// timestamps that parse as RFC 3339.
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

// ---- authentication ----

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

// ---- create ----

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
async fn creating_a_project_secret_stores_the_scope_id() {
    let app = TestApp::spawn().await;
    let ada = user(&app, "ada").await;
    let project = seed_project(&app.pool, "mars").await;

    let meta = seed_secret(&app, &ada, "project", Some(project), "DEPLOY_TOKEN").await;

    assert_meta_shape(&meta);
    assert_eq!(meta["scope"], json!("project"));
    assert_eq!(meta["scope_id"], json!(project.to_string()));
}

#[tokio::test]
async fn creating_rejects_a_bad_name_an_empty_value_and_an_impossible_scope() {
    let app = TestApp::spawn().await;
    let ada = user(&app, "ada").await;

    let bodies = vec![
        // `[A-Z][A-Z0-9_]*` at 1–128 characters (`docs/data-model.md`).
        create_body("global", None, "lowercase"),
        create_body("global", None, &format!("A{}", "B".repeat(128))),
        json!({ "scope": "global", "name": "DEPLOY_TOKEN", "value": "" }),
        // A target for a scope that takes none, and none for a scope that
        // needs one.
        create_body("global", Some(Uuid::new_v4()), "DEPLOY_TOKEN"),
        json!({ "scope": "project", "name": "DEPLOY_TOKEN", "value": FAKE_VALUE }),
        // A project that does not exist: 400, because `scope_id` is a field of
        // the body rather than the target of the request.
        create_body("project", Some(Uuid::new_v4()), "DEPLOY_TOKEN"),
        // Not one of the three scopes.
        create_body("team", None, "DEPLOY_TOKEN"),
    ];

    for body in bodies {
        let response = app.post_as(&ada, SECRETS).json(&body).await;

        let status = response.status_code();
        let text = response.text();
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body} answered {text}");

        let error = response.json::<Value>();
        assert_eq!(error["status"], json!(400), "{body}: {error}");
        assert!(error["error"].is_string(), "{body}: {error}");
    }

    // A `user` scope pointing at nobody is the same 400 — but only for a
    // caller who was allowed to aim at another user in the first place: for
    // anybody else the ownership rule answers first, and that 403 is
    // `creating_in_another_users_scope_is_forbidden_but_an_administrator_may`.
    let root = admin(&app, "root").await;
    let response = app
        .post_as(&root, SECRETS)
        .json(&create_body("user", Some(Uuid::new_v4()), "DEPLOY_TOKEN"))
        .await;

    response.assert_status(StatusCode::BAD_REQUEST);
    assert_eq!(response.json::<Value>()["status"], json!(400));
}

#[tokio::test]
async fn creating_in_another_users_scope_is_forbidden_but_an_administrator_may() {
    let app = TestApp::spawn().await;
    let ada = user(&app, "ada").await;
    let bob = user(&app, "bob").await;
    let root = admin(&app, "root").await;

    let response = app
        .post_as(&bob, SECRETS)
        .json(&create_body("user", Some(ada.user.id), "DEPLOY_TOKEN"))
        .await;
    response.assert_status(StatusCode::FORBIDDEN);

    let response = app
        .post_as(&root, SECRETS)
        .json(&create_body("user", Some(ada.user.id), "DEPLOY_TOKEN"))
        .await;
    response.assert_status(StatusCode::CREATED);

    let meta = response.json::<Value>();
    assert_eq!(meta["scope"], json!("user"));
    assert_eq!(meta["scope_id"], json!(ada.user.id.to_string()));
    // The administrator created it; the secret is still Ada's.
    assert_eq!(meta["created_by"], json!(root.user.id.to_string()));
}

#[tokio::test]
async fn creating_a_duplicate_name_in_one_scope_is_409() {
    let app = TestApp::spawn().await;
    let ada = user(&app, "ada").await;
    seed_secret(&app, &ada, "global", None, "DEPLOY_TOKEN").await;

    let response = app
        .post_as(&ada, SECRETS)
        .json(&create_body("global", None, "DEPLOY_TOKEN"))
        .await;

    response.assert_status(StatusCode::CONFLICT);
    assert_eq!(response.json::<Value>()["status"], json!(409));
}

// ---- list ----

#[tokio::test]
async fn listing_answers_the_secrets_the_caller_may_see() {
    let app = TestApp::spawn().await;
    let ada = user(&app, "ada").await;
    let bob = user(&app, "bob").await;

    seed_secret(&app, &ada, "global", None, "GLOBAL_TOKEN").await;
    seed_secret(&app, &ada, "user", None, "ADA_TOKEN").await;
    seed_secret(&app, &bob, "user", None, "BOB_TOKEN").await;

    let response = app.get_as(&ada, SECRETS).await;

    response.assert_status(StatusCode::OK);
    let listing = response.json::<Vec<Value>>();
    for meta in &listing {
        assert_meta_shape(meta);
    }

    let mut names: Vec<&str> = listing
        .iter()
        .map(|meta| meta["name"].as_str().expect("a name"))
        .collect();
    names.sort_unstable();
    assert_eq!(names, vec!["ADA_TOKEN", "GLOBAL_TOKEN"]);
}

#[tokio::test]
async fn listing_a_scope_filters_and_refuses_the_impossible_combinations() {
    let app = TestApp::spawn().await;
    let ada = user(&app, "ada").await;
    let project = seed_project(&app.pool, "mars").await;

    seed_secret(&app, &ada, "global", None, "GLOBAL_TOKEN").await;
    seed_secret(&app, &ada, "user", None, "ADA_TOKEN").await;
    seed_secret(&app, &ada, "project", Some(project), "PROJECT_TOKEN").await;

    for (query, expected) in [
        ("?scope=global", "GLOBAL_TOKEN"),
        ("?scope=user", "ADA_TOKEN"),
        (
            &format!("?scope=user&scope_id={}", ada.user.id),
            "ADA_TOKEN",
        ),
        (
            &format!("?scope=project&scope_id={project}"),
            "PROJECT_TOKEN",
        ),
    ] {
        let response = app.get_as(&ada, &format!("{SECRETS}{query}")).await;

        response.assert_status(StatusCode::OK);
        let listing = response.json::<Vec<Value>>();
        assert_eq!(listing.len(), 1, "{query}: {listing:?}");
        assert_eq!(listing[0]["name"], json!(expected), "{query}");
    }

    for query in [
        // Not one of the three.
        "?scope=team",
        // `project` needs a target and `global` takes none.
        "?scope=project",
        &format!("?scope=global&scope_id={project}"),
    ] {
        let response = app.get_as(&ada, &format!("{SECRETS}{query}")).await;

        response.assert_status(StatusCode::BAD_REQUEST);
        let error = response.json::<Value>();
        assert_eq!(error["status"], json!(400), "{query}: {error}");
        assert!(error["error"].is_string(), "{query}: {error}");
    }
}

#[tokio::test]
async fn listing_another_users_scope_is_forbidden_but_an_administrator_may() {
    let app = TestApp::spawn().await;
    let ada = user(&app, "ada").await;
    let bob = user(&app, "bob").await;
    let root = admin(&app, "root").await;
    seed_secret(&app, &ada, "user", None, "ADA_TOKEN").await;

    let path = format!("{SECRETS}?scope=user&scope_id={}", ada.user.id);

    let response = app.get_as(&bob, &path).await;
    response.assert_status(StatusCode::FORBIDDEN);
    assert_eq!(response.json::<Value>()["status"], json!(403));

    let response = app.get_as(&root, &path).await;
    response.assert_status(StatusCode::OK);
    let listing = response.json::<Vec<Value>>();
    assert_eq!(listing.len(), 1);
    assert_eq!(listing[0]["name"], json!("ADA_TOKEN"));
}

// ---- replace ----

#[tokio::test]
async fn replacing_a_value_answers_the_metadata_and_keeps_the_name() {
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
async fn replacing_refuses_an_empty_value_an_unknown_id_and_a_foreign_secret() {
    let app = TestApp::spawn().await;
    let ada = user(&app, "ada").await;
    let bob = user(&app, "bob").await;
    let meta = seed_secret(&app, &ada, "user", None, "DEPLOY_TOKEN").await;
    let id = id_of(&meta);

    let response = app
        .put_as(&ada, &format!("{SECRETS}/{id}"))
        .json(&json!({ "value": "" }))
        .await;
    response.assert_status(StatusCode::BAD_REQUEST);

    let response = app
        .put_as(&ada, &format!("{SECRETS}/{}", Uuid::new_v4()))
        .json(&json!({ "value": OTHER_FAKE_VALUE }))
        .await;
    response.assert_status(StatusCode::NOT_FOUND);

    let response = app
        .put_as(&bob, &format!("{SECRETS}/{id}"))
        .json(&json!({ "value": OTHER_FAKE_VALUE }))
        .await;
    response.assert_status(StatusCode::FORBIDDEN);
}

// ---- patch ----

#[tokio::test]
async fn patching_renames_reflags_and_leaves_an_empty_body_unchanged() {
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
    assert_eq!(unchanged["name"], json!("RENAMED_TOKEN"));
    assert_eq!(unchanged["orchestrator_only"], json!(true));
    assert_eq!(unchanged["updated_at"], patched["updated_at"]);
}

#[tokio::test]
async fn patching_refuses_a_bad_name_a_taken_name_an_unknown_id_and_a_foreign_secret() {
    let app = TestApp::spawn().await;
    let ada = user(&app, "ada").await;
    let bob = user(&app, "bob").await;
    let meta = seed_secret(&app, &ada, "user", None, "DEPLOY_TOKEN").await;
    seed_secret(&app, &ada, "user", None, "OTHER_TOKEN").await;
    let id = id_of(&meta);

    let response = app
        .patch_as(&ada, &format!("{SECRETS}/{id}"))
        .json(&json!({ "name": "lowercase" }))
        .await;
    response.assert_status(StatusCode::BAD_REQUEST);

    // Renaming onto a name this scope already holds.
    let response = app
        .patch_as(&ada, &format!("{SECRETS}/{id}"))
        .json(&json!({ "name": "OTHER_TOKEN" }))
        .await;
    response.assert_status(StatusCode::CONFLICT);

    let response = app
        .patch_as(&ada, &format!("{SECRETS}/{}", Uuid::new_v4()))
        .json(&json!({ "name": "RENAMED_TOKEN" }))
        .await;
    response.assert_status(StatusCode::NOT_FOUND);

    let response = app
        .patch_as(&bob, &format!("{SECRETS}/{id}"))
        .json(&json!({ "name": "RENAMED_TOKEN" }))
        .await;
    response.assert_status(StatusCode::FORBIDDEN);
}

// ---- delete ----

#[tokio::test]
async fn deleting_answers_204_and_then_404() {
    let app = TestApp::spawn().await;
    let ada = user(&app, "ada").await;
    let meta = seed_secret(&app, &ada, "user", None, "DEPLOY_TOKEN").await;
    let id = id_of(&meta);

    let response = app.delete_as(&ada, &format!("{SECRETS}/{id}")).await;
    response.assert_status(StatusCode::NO_CONTENT);
    assert!(response.text().is_empty(), "204 carries no body");

    let response = app.delete_as(&ada, &format!("{SECRETS}/{id}")).await;
    response.assert_status(StatusCode::NOT_FOUND);

    let response = app.get_as(&ada, SECRETS).await;
    response.assert_status(StatusCode::OK);
    assert!(response.json::<Vec<Value>>().is_empty());
}

#[tokio::test]
async fn deleting_a_foreign_secret_is_forbidden_and_leaves_it_standing() {
    let app = TestApp::spawn().await;
    let ada = user(&app, "ada").await;
    let bob = user(&app, "bob").await;
    let meta = seed_secret(&app, &ada, "user", None, "DEPLOY_TOKEN").await;
    let id = id_of(&meta);

    let response = app.delete_as(&bob, &format!("{SECRETS}/{id}")).await;
    response.assert_status(StatusCode::FORBIDDEN);

    let response = app.get_as(&ada, &format!("{SECRETS}?scope=user")).await;
    response.assert_status(StatusCode::OK);
    assert_eq!(response.json::<Vec<Value>>().len(), 1);
}

// ---- uses ----

#[tokio::test]
async fn uses_are_newest_first_and_honour_the_limit() {
    let app = TestApp::spawn().await;
    let ada = user(&app, "ada").await;
    let meta = seed_secret(&app, &ada, "user", None, "DEPLOY_TOKEN").await;
    let id = id_of(&meta);
    assert_eq!(meta["last_used_at"], Value::Null);

    seed_uses(&app.pool, id, ada.user.id, 5).await;

    let response = app.get_as(&ada, &format!("{SECRETS}/{id}/uses")).await;
    response.assert_status(StatusCode::OK);
    let uses = response.json::<Vec<Value>>();
    assert_eq!(uses.len(), 5);

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

    // Newest first (`SPEC.md`, "Secrets").
    let at: Vec<&str> = uses
        .iter()
        .map(|use_row| use_row["at"].as_str().expect("an RFC 3339 timestamp"))
        .collect();
    for pair in at.windows(2) {
        let newer = DateTime::parse_from_rfc3339(pair[0]).expect("RFC 3339");
        let older = DateTime::parse_from_rfc3339(pair[1]).expect("RFC 3339");
        assert!(newer > older, "{pair:?} is not newest first");
    }

    let response = app
        .get_as(&ada, &format!("{SECRETS}/{id}/uses?limit=2"))
        .await;
    response.assert_status(StatusCode::OK);
    assert_eq!(response.json::<Vec<Value>>().len(), 2);

    // A use makes `last_used_at` appear.
    let response = app.get_as(&ada, &format!("{SECRETS}?scope=user")).await;
    response.assert_status(StatusCode::OK);
    let listing = response.json::<Vec<Value>>();
    assert!(listing[0]["last_used_at"].is_string());
}

#[tokio::test]
async fn uses_refuse_a_bad_limit_an_unknown_id_and_a_foreign_secret() {
    let app = TestApp::spawn().await;
    let ada = user(&app, "ada").await;
    let bob = user(&app, "bob").await;
    let root = admin(&app, "root").await;
    let meta = seed_secret(&app, &ada, "user", None, "DEPLOY_TOKEN").await;
    let id = id_of(&meta);
    seed_uses(&app.pool, id, ada.user.id, 1).await;

    for query in ["?limit=lots", "?limit=0"] {
        let response = app
            .get_as(&ada, &format!("{SECRETS}/{id}/uses{query}"))
            .await;

        response.assert_status(StatusCode::BAD_REQUEST);
        let error = response.json::<Value>();
        assert_eq!(error["status"], json!(400), "{query}: {error}");
        assert!(error["error"].is_string(), "{query}: {error}");
    }

    let response = app
        .get_as(&ada, &format!("{SECRETS}/{}/uses", Uuid::new_v4()))
        .await;
    response.assert_status(StatusCode::NOT_FOUND);

    let response = app.get_as(&bob, &format!("{SECRETS}/{id}/uses")).await;
    response.assert_status(StatusCode::FORBIDDEN);

    // The owner's administrator may read the same audit.
    let response = app.get_as(&root, &format!("{SECRETS}/{id}/uses")).await;
    response.assert_status(StatusCode::OK);
    assert_eq!(response.json::<Vec<Value>>().len(), 1);
}

// ---- values, and the shape of a rejection ----

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

        response.assert_status(StatusCode::BAD_REQUEST);
        let error = response.json::<Value>();
        assert_eq!(error["status"], json!(400), "{path}: {error}");
        assert!(error["error"].is_string(), "{path}: {error}");
    }

    let response = app.delete_as(&ada, &format!("{SECRETS}/not-a-uuid")).await;
    response.assert_status(StatusCode::BAD_REQUEST);
    assert_eq!(response.json::<Value>()["status"], json!(400));
}
