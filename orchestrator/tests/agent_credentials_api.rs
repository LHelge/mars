//! `GET /api/projects/{pid}/agent-credentials` through the real router
//! (`SPEC.md`, "Secrets", `AgentCredentialStatus`; `ARCHITECTURE.md`,
//! "Secrets", Agent credentials; ADR 0036).
//!
//! The preflight that answers "which credential would a launch by me use",
//! before anything is launched. It runs the selection a launch runs
//! (`secrets::select_credential`, reached through
//! `secrets::preview_credential`), so what is asserted here is not the
//! precedence rule itself — that is `tests/secrets_resolve.rs`, over the same
//! function — but that this endpoint gives the same answer for the *caller*
//! and gives it without touching a value:
//!
//! - one entry per backend, in enum order, credential or null;
//! - `global`, then the project's, then the caller's own, most specific first,
//!   with the winning row's own name whichever of the backend's names it is;
//! - another user's user-scoped credential is invisible, to an administrator
//!   too, because the question is "if *I* launch now" and there is no
//!   `?user_id=`;
//! - an `orchestrator_only` credential row — only one older than the write
//!   rule that forbids it — answers null, exactly as a launch would inject
//!   nothing;
//! - nothing is decrypted, no `secret_uses` row is written and no value
//!   reaches the response, asserted against the raw response text and through
//!   `GET /secrets/{id}/uses`;
//! - 401 without a token, 403 under the password-change gate, 404 for an
//!   unknown project.
//!
//! Every value stored here is an obviously fake credential and no test prints
//! one (`CLAUDE.md`, rule 3).
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use axum::http::StatusCode;
use common::{AuthenticatedUser, TestApp};
use mars_orchestrator::models::{NewSecret, ScopeRef, SecretName};
use mars_orchestrator::repositories::SecretRepository;
use mars_orchestrator::secrets::{SealedSecret, SecretIdentity};
use serde_json::{Value, json};
use uuid::Uuid;

/// Not a real remote: `.invalid` can never resolve (rule 3).
const TEST_REMOTE: &str = "https://example.invalid/org/repo.git";

/// Not a real credential: the value every row here holds (rule 3).
const FAKE_VALUE: &str = "fake-value-not-a-credential";

/// The Claude adapter's two credential names, most preferred first
/// (`agent::credential_names`).
const OAUTH_TOKEN: &str = "CLAUDE_CODE_OAUTH_TOKEN";
const API_KEY: &str = "ANTHROPIC_API_KEY";

// ---- helpers ----

/// An obviously fake password of the length `POST /api/test/users` requires
/// (rule 3).
fn password(name: &str) -> String {
    format!("fake-password-{name}")
}

/// A signed-in ordinary user to make requests as.
async fn signed_in(app: &TestApp, name: &str) -> AuthenticatedUser {
    app.create_user(name, &format!("{name}@example.test"), &password(name))
        .await
}

/// The documented 401 body (`SPEC.md`, "Authentication").
fn unauthorized() -> Value {
    json!({ "status": 401, "error": "authentication required" })
}

/// The documented body of the password-change gate (`SPEC.md`,
/// "Authentication").
fn password_change_required() -> Value {
    json!({ "status": 403, "error": "password change required" })
}

/// A project of this test's own, through the documented create endpoint.
///
/// No `credential`, so the project has no `GIT_CREDENTIAL` row of its own and
/// the only secrets in a test are the ones it seeds. The clone fails in the
/// background and nothing here waits for it: a `cloning` project answers this
/// endpoint normally, because a credential does not depend on the mirror.
async fn project(app: &TestApp, user: &AuthenticatedUser, name: &str) -> Uuid {
    let response = app
        .post_as(user, "/api/projects")
        .json(&json!({ "name": name, "remote_url": TEST_REMOTE }))
        .await;
    response.assert_status(StatusCode::CREATED);

    response.json::<Value>()["id"]
        .as_str()
        .expect("a project carries an id")
        .parse()
        .expect("the id is a uuid")
}

/// `/api/projects/{pid}/agent-credentials`.
fn path(pid: Uuid) -> String {
    format!("/api/projects/{pid}/agent-credentials")
}

/// Create a secret as `caller` through `POST /api/secrets` and answer its id.
///
/// The arrangement every test but the `orchestrator_only` one uses: a
/// credential is an ordinary secret created through the documented endpoint
/// (`SPEC.md`, "Secrets").
async fn seed_secret(
    app: &TestApp,
    caller: &AuthenticatedUser,
    scope: &str,
    scope_id: Option<Uuid>,
    name: &str,
) -> Uuid {
    let response = app
        .post_as(caller, "/api/secrets")
        .json(&json!({
            "scope": scope,
            "scope_id": scope_id.map(|id| id.to_string()),
            "name": name,
            "value": FAKE_VALUE,
        }))
        .await;
    response.assert_status(StatusCode::CREATED);

    response.json::<Value>()["id"]
        .as_str()
        .expect("the metadata carries an id")
        .parse()
        .expect("the id is a uuid")
}

/// Commit an `orchestrator_only` credential row straight through the
/// repository.
///
/// The row the endpoint has to cope with is one *older than* the rule that a
/// credential is never `orchestrator_only`, which the service refuses to
/// create (`ARCHITECTURE.md`, "Secrets", Agent credentials). There is no
/// interface that writes one, so this test arranges it at the table, sealed by
/// the real keyring under the harness's fixed test key.
async fn seed_orchestrator_only_credential(app: &TestApp, scope: ScopeRef, name: &str) -> Uuid {
    let secret_name = SecretName::parse(name).expect("the test credential name is valid");
    let sealed = SealedSecret::seal(
        &app.state.keyring,
        SecretIdentity::new(&scope, &secret_name),
        FAKE_VALUE.as_bytes(),
    )
    .expect("the value seals");

    let mut new = NewSecret::new(sealed);
    new.orchestrator_only = true;

    let mut tx = app.pool.begin().await.expect("a transaction begins");
    let inserted = SecretRepository::new(&app.pool)
        .insert(&mut tx, &new)
        .await
        .expect("the secret inserts");
    tx.commit().await.expect("the transaction commits");

    inserted.id
}

/// The one `claude` entry of an answer, asserted to be the whole answer.
///
/// `SPEC.md` gives one entry per backend in enum order; `agent::BACKENDS` has
/// exactly one member in v1, so a longer list here is a backend added without
/// this endpoint being revisited.
#[track_caller]
fn claude_entry(body: &Value) -> Value {
    let entries = body.as_array().expect("the answer is an array");
    assert_eq!(entries.len(), 1, "one entry per backend: {body}");

    let entry = entries[0].as_object().expect("an entry is an object");
    let mut keys: Vec<&str> = entry.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["backend", "credential"], "unexpected fields: {body}");
    assert_eq!(entry["backend"], json!("claude"));

    entry["credential"].clone()
}

/// The answer this caller gets for this project, with the shape asserted and
/// no value anywhere in the raw text.
async fn credential_of(app: &TestApp, caller: &AuthenticatedUser, pid: Uuid) -> Value {
    let response = app.get_as(caller, &path(pid)).await;
    response.assert_status_ok();

    // The whole point of the endpoint: a value could only appear here by a
    // defect, so the raw text is checked rather than a parsed field, which
    // would miss one smuggled into an unexpected key (rule 3).
    assert!(
        !response.text().contains(FAKE_VALUE),
        "a value reached the response"
    );

    claude_entry(&response.json::<Value>())
}

/// The documented `{ secret_id, name, scope }`, as a whole value.
fn credential(secret_id: Uuid, name: &str, scope: &str) -> Value {
    json!({ "secret_id": secret_id, "name": name, "scope": scope })
}

/// The recorded uses of `secret_id`, read the way a client reads them.
async fn uses(app: &TestApp, caller: &AuthenticatedUser, secret_id: Uuid) -> Vec<Value> {
    let response = app
        .get_as(caller, &format!("/api/secrets/{secret_id}/uses"))
        .await;
    response.assert_status_ok();

    response.json::<Vec<Value>>()
}

// ---- the selection ----

#[tokio::test]
async fn a_project_with_no_credential_anywhere_answers_one_null_entry_per_backend() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "no-credential").await;

    assert_eq!(credential_of(&app, &user, pid).await, Value::Null);
}

#[tokio::test]
async fn a_global_credential_is_what_a_launch_would_use() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "global-only").await;

    let id = seed_secret(&app, &user, "global", None, OAUTH_TOKEN).await;

    assert_eq!(
        credential_of(&app, &user, pid).await,
        credential(id, OAUTH_TOKEN, "global")
    );
}

#[tokio::test]
async fn the_projects_credential_beats_the_global_one() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "project-beats-global").await;

    seed_secret(&app, &user, "global", None, OAUTH_TOKEN).await;
    let project_id = seed_secret(&app, &user, "project", Some(pid), OAUTH_TOKEN).await;

    assert_eq!(
        credential_of(&app, &user, pid).await,
        credential(project_id, OAUTH_TOKEN, "project")
    );
}

#[tokio::test]
async fn the_callers_own_credential_beats_the_projects_and_another_users_is_invisible() {
    let app = TestApp::spawn().await;
    let ada = signed_in(&app, "ada").await;
    let bob = signed_in(&app, "bob").await;
    let pid = project(&app, &ada, "user-beats-project").await;

    let project_id = seed_secret(&app, &ada, "project", Some(pid), OAUTH_TOKEN).await;
    let ada_id = seed_secret(&app, &ada, "user", Some(ada.user.id), OAUTH_TOKEN).await;
    let bob_id = seed_secret(&app, &bob, "user", Some(bob.user.id), OAUTH_TOKEN).await;

    // Each of them sees their own row and neither sees the other's: the user
    // scope of the answer is the caller's, and the project's row is what is
    // left when a caller has none.
    assert_eq!(
        credential_of(&app, &ada, pid).await,
        credential(ada_id, OAUTH_TOKEN, "user")
    );
    assert_eq!(
        credential_of(&app, &bob, pid).await,
        credential(bob_id, OAUTH_TOKEN, "user")
    );
    assert_ne!(ada_id, bob_id);
    assert_ne!(ada_id, project_id);
}

#[tokio::test]
async fn an_administrator_gets_their_own_answer_and_not_everyones() {
    let app = TestApp::spawn().await;
    let ada = signed_in(&app, "ada").await;
    let root = app
        .create_admin("root", "root@example.test", &password("root"))
        .await;
    let pid = project(&app, &ada, "admin-sees-their-own").await;

    let project_id = seed_secret(&app, &ada, "project", Some(pid), OAUTH_TOKEN).await;
    seed_secret(&app, &ada, "user", Some(ada.user.id), OAUTH_TOKEN).await;

    // The administrator may *read* Ada's user-scoped secret through
    // `GET /secrets`, but a launch of theirs would not be given it: there is
    // no `?user_id=` and the user scope is the caller's own.
    assert_eq!(
        credential_of(&app, &root, pid).await,
        credential(project_id, OAUTH_TOKEN, "project")
    );
}

#[tokio::test]
async fn the_most_specific_scope_wins_whichever_of_the_names_it_carries() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "different-names").await;

    // One credential per scope, two different names: the answer is the winning
    // scope's name, not the backend's preferred one.
    seed_secret(&app, &user, "global", None, OAUTH_TOKEN).await;
    let project_id = seed_secret(&app, &user, "project", Some(pid), API_KEY).await;

    assert_eq!(
        credential_of(&app, &user, pid).await,
        credential(project_id, API_KEY, "project")
    );
}

#[tokio::test]
async fn an_orchestrator_only_credential_at_the_winning_scope_answers_null() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "withheld-credential").await;

    // The global row is injectable and is *not* a fallback: the project's row
    // already overrode it, and a launch would inject neither.
    seed_secret(&app, &user, "global", None, OAUTH_TOKEN).await;
    seed_orchestrator_only_credential(&app, ScopeRef::project(pid), API_KEY).await;

    assert_eq!(credential_of(&app, &user, pid).await, Value::Null);
}

// ---- what it does not do ----

#[tokio::test]
async fn the_preflight_records_no_use_of_the_credential_it_names() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "no-uses").await;

    let id = seed_secret(&app, &user, "global", None, OAUTH_TOKEN).await;
    assert!(uses(&app, &user, id).await.is_empty(), "arranged with uses");

    for _ in 0..3 {
        assert_eq!(
            credential_of(&app, &user, pid).await,
            credential(id, OAUTH_TOKEN, "global")
        );
    }

    assert!(
        uses(&app, &user, id).await.is_empty(),
        "the preflight wrote a secret_uses row"
    );

    // `last_used_at` is derived from those rows, so it has not moved either.
    let response = app.get_as(&user, "/api/secrets").await;
    response.assert_status_ok();
    let listed = response.json::<Vec<Value>>();
    let meta = listed
        .iter()
        .find(|meta| meta["id"] == json!(id))
        .expect("the credential is listed");
    assert_eq!(meta["last_used_at"], Value::Null);
}

// ---- the gates ----

#[tokio::test]
async fn the_endpoint_requires_a_token() {
    let app = TestApp::spawn().await;
    let response = app.server.get(&path(Uuid::new_v4())).await;

    response.assert_status(StatusCode::UNAUTHORIZED);
    response.assert_json(&unauthorized());
}

#[tokio::test]
async fn the_endpoint_is_refused_while_a_password_change_is_pending() {
    let app = TestApp::spawn().await;
    let gated = app.create_gated_user("gated", "gated@example.test").await;
    let response = app.get_as(&gated, &path(Uuid::new_v4())).await;

    response.assert_status(StatusCode::FORBIDDEN);
    response.assert_json(&password_change_required());
}

#[tokio::test]
async fn an_unknown_project_is_404_rather_than_the_global_answer() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;

    // A global credential exists, so a 200 here would be an answer about a
    // project that does not exist.
    seed_secret(&app, &user, "global", None, OAUTH_TOKEN).await;

    let response = app.get_as(&user, &path(Uuid::new_v4())).await;
    response.assert_status(StatusCode::NOT_FOUND);
}
