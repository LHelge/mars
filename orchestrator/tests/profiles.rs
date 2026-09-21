//! `/api/projects/{pid}/profiles` through the real router (`SPEC.md`, "Agent
//! profiles (`/api/projects/{pid}/profiles`)").
//!
//! The five endpoints of the agent-profiles table: the list, the create, the
//! read, the full-replacement edit and the delete. What the rules *are* is
//! asserted against the models in `src/models/agent_profile.rs` and against a
//! real database in `tests/repositories_profiles.rs`; what is asserted here is
//! the adapter — the paths, the JWT requirement and the password-change gate on
//! each of them, the status and body of every success and of every documented
//! failure, and that the mutations write what they claim to and nothing else.
//!
//! The project these tests hang their profiles on is created through
//! `POST /api/projects` with an unreachable remote, so its background clone job
//! fails almost at once. Nothing here waits for it or asserts on the project's
//! status: a profile is configuration, and every one of these endpoints works
//! whatever state the clone is in.
//!
//! Every credential-shaped value is an obviously fake stand-in (rule 3).
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use axum::http::StatusCode;
use axum_test::TestResponse;
use common::{AuthenticatedUser, TestApp};
use serde_json::{Value, json};
use uuid::Uuid;

/// Not a real remote: `.invalid` can never resolve (rule 3).
const TEST_REMOTE: &str = "https://example.invalid/org/repo.git";

/// The image `SESSION_IMAGE_DEFAULT` carries in a test app, which is the image
/// a profile that names none is stored with.
const TEST_IMAGE: &str = "mars-session-stub:test";

/// The eighteen fields of `Profile` (`SPEC.md`, "Agent profiles"), sorted.
const PROFILE_FIELDS: [&str; 18] = [
    "backend",
    "created_at",
    "id",
    "idle_timeout_secs",
    "image",
    "is_default",
    "kind",
    "mcp_tools",
    "model",
    "name",
    "partial_messages",
    "permission_mode",
    "project_id",
    "runtime",
    "secrets",
    "serves_states",
    "system_prompt",
    "updated_at",
];

// ---- helpers ----
//
// The project-side helpers are the ones `tests/projects.rs` uses, copied
// rather than shared: a later task consolidates the common ones into
// `tests/common/`.

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

/// A project to hang profiles on, created through its own endpoint so it is
/// seeded exactly as a real one is: the seven default states and the `default`
/// profile.
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

/// `/api/projects/{pid}/profiles`.
fn profiles_path(pid: Uuid) -> String {
    format!("/api/projects/{pid}/profiles")
}

/// `/api/projects/{pid}/profiles/{id}`.
fn profile_path(pid: Uuid, id: Uuid) -> String {
    format!("/api/projects/{pid}/profiles/{id}")
}

/// `POST /api/projects/{pid}/profiles` with `body`, as `user`.
async fn post(app: &TestApp, user: &AuthenticatedUser, pid: Uuid, body: &Value) -> TestResponse {
    app.post_as(user, &profiles_path(pid)).json(body).await
}

/// `POST /api/projects/{pid}/profiles` for a profile that must be created,
/// asserting 201.
async fn created(app: &TestApp, user: &AuthenticatedUser, pid: Uuid, body: &Value) -> Value {
    let response = post(app, user, pid, body).await;

    response.assert_status(StatusCode::CREATED);
    response.json::<Value>()
}

/// `PUT /api/projects/{pid}/profiles/{id}` with `body`, as `user`.
async fn put(
    app: &TestApp,
    user: &AuthenticatedUser,
    pid: Uuid,
    id: Uuid,
    body: &Value,
) -> TestResponse {
    app.put_as(user, &profile_path(pid, id)).json(body).await
}

/// `GET /api/projects/{pid}/profiles`, asserting 200.
async fn list(app: &TestApp, user: &AuthenticatedUser, pid: Uuid) -> Vec<Value> {
    let response = app.get_as(user, &profiles_path(pid)).await;

    response.assert_status_ok();
    response
        .json::<Value>()
        .as_array()
        .expect("the listing is an array")
        .clone()
}

/// The id of a profile the API answered with.
fn id_of(profile: &Value) -> Uuid {
    profile["id"]
        .as_str()
        .expect("a profile carries an id")
        .parse()
        .expect("the id is a uuid")
}

/// The names of the profiles in a listing, in the order they came back.
fn names(profiles: &[Value]) -> Vec<&str> {
    profiles
        .iter()
        .map(|profile| profile["name"].as_str().expect("a profile carries a name"))
        .collect()
}

/// Assert the documented `{ status, error }` body of a failure.
fn assert_error(response: &TestResponse, status: StatusCode, message: &str) {
    response.assert_status(status);
    response.assert_json(&json!({ "status": status.as_u16(), "error": message }));
}

/// How many `profile_states` rows this profile has.
async fn served_row_count(app: &TestApp, id: Uuid) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM profile_states WHERE profile_id = $1")
        .bind(id)
        .fetch_one(&app.pool)
        .await
        .expect("the count runs")
}

/// How many `task_events` rows this project has.
///
/// A profile is project configuration and no endpoint here touches the board,
/// so this is zero after every mutation (`SPEC.md`, "TaskEvent").
async fn task_event_count(app: &TestApp, pid: Uuid) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM task_events WHERE project_id = $1")
        .bind(pid)
        .fetch_one(&app.pool)
        .await
        .expect("the count runs")
}

/// Seed a session row on `profile_id` directly.
///
/// `SessionRepository` and the launch path belong to another epic; this test
/// only needs the row that makes the profile undeletable. The token hash is an
/// obviously fake constant, never a credential (rule 3).
async fn seed_session(app: &TestApp, pid: Uuid, profile_id: Uuid) {
    let id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO sessions (id, project_id, profile_id, kind, base_ref, branch, mcp_token_hash)
        VALUES ($1, $2, $3, 'conversational'::profile_kind, 'main', $4, $5)
        "#,
    )
    .bind(id)
    .bind(pid)
    .bind(profile_id)
    .bind(format!("session/{id}"))
    .bind(format!("fake-token-hash-{id}"))
    .execute(&app.pool)
    .await
    .expect("the session seeds");
}

// ---- authentication ----

#[tokio::test]
async fn every_endpoint_requires_a_token() {
    let app = TestApp::spawn().await;
    let pid = Uuid::new_v4();
    let id = Uuid::new_v4();
    let body = json!({ "name": "planner" });

    let responses = [
        app.server.get(&profiles_path(pid)).await,
        app.server.post(&profiles_path(pid)).json(&body).await,
        app.server.get(&profile_path(pid, id)).await,
        app.server.put(&profile_path(pid, id)).json(&body).await,
        app.server.delete(&profile_path(pid, id)).await,
    ];

    for response in responses {
        response.assert_status(StatusCode::UNAUTHORIZED);
        response.assert_json(&unauthorized());
    }
}

#[tokio::test]
async fn every_endpoint_is_refused_while_a_password_change_is_pending() {
    let app = TestApp::spawn().await;
    let gated = app.create_gated_user("gated", "gated@example.test").await;
    let pid = Uuid::new_v4();
    let id = Uuid::new_v4();
    let body = json!({ "name": "planner" });

    let responses = [
        app.get_as(&gated, &profiles_path(pid)).await,
        app.post_as(&gated, &profiles_path(pid)).json(&body).await,
        app.get_as(&gated, &profile_path(pid, id)).await,
        app.put_as(&gated, &profile_path(pid, id)).json(&body).await,
        app.delete_as(&gated, &profile_path(pid, id)).await,
    ];

    for response in responses {
        response.assert_status(StatusCode::FORBIDDEN);
        response.assert_json(&password_change_required());
    }
}

// ---- list ----

#[tokio::test]
async fn listing_shows_the_seeded_default_profile_in_the_documented_shape() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;

    let profiles = list(&app, &user, pid).await;

    assert_eq!(names(&profiles), ["default"]);
    let default = &profiles[0];
    assert_eq!(default["project_id"], json!(pid.to_string()));
    assert_eq!(default["kind"], json!("conversational"));
    assert_eq!(default["backend"], json!("claude"));
    assert_eq!(default["model"], Value::Null);
    assert_eq!(default["system_prompt"], Value::Null);
    assert_eq!(default["permission_mode"], json!("bypass"));
    assert_eq!(default["image"], json!(TEST_IMAGE));
    assert_eq!(default["runtime"], Value::Null);
    assert_eq!(default["mcp_tools"], json!([]));
    assert_eq!(default["secrets"], json!([]));
    assert_eq!(default["serves_states"], json!(["ready"]));
    assert_eq!(default["partial_messages"], json!(true));
    assert_eq!(default["idle_timeout_secs"], json!(1800));
    assert_eq!(default["is_default"], json!(true));
    assert!(default["created_at"].is_string());
    assert!(default["updated_at"].is_string());

    // Exactly the documented eighteen fields, and nothing the row might add.
    let mut keys: Vec<&str> = default
        .as_object()
        .expect("a profile is an object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, PROFILE_FIELDS);
}

#[tokio::test]
async fn listing_an_unknown_project_is_not_found() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;

    let response = app.get_as(&user, &profiles_path(Uuid::new_v4())).await;

    assert_error(&response, StatusCode::NOT_FOUND, "not found");
}

#[tokio::test]
async fn the_listing_is_oldest_first() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;

    // Deliberately in reverse alphabetical order, so name order and creation
    // order disagree.
    created(&app, &user, pid, &json!({ "name": "reviewer" })).await;
    created(&app, &user, pid, &json!({ "name": "planner" })).await;
    created(&app, &user, pid, &json!({ "name": "archivist" })).await;

    assert_eq!(
        names(&list(&app, &user, pid).await),
        ["default", "reviewer", "planner", "archivist"]
    );
}

// ---- create ----

#[tokio::test]
async fn creating_a_planner_stores_it_with_the_states_it_serves() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;

    let planner = created(
        &app,
        &user,
        pid,
        &json!({
            "name": "planner",
            "serves_states": ["backlog"],
            "system_prompt": "Break the request down into tasks.",
        }),
    )
    .await;

    assert_eq!(planner["name"], json!("planner"));
    assert_eq!(planner["kind"], json!("conversational"));
    assert_eq!(planner["serves_states"], json!(["backlog"]));
    assert_eq!(
        planner["system_prompt"],
        json!("Break the request down into tasks.")
    );
    // Conversational, and nothing was said, so streaming is on.
    assert_eq!(planner["partial_messages"], json!(true));
    // The seeded profile keeps the flag.
    assert_eq!(planner["is_default"], json!(false));
    assert_eq!(planner["image"], json!(TEST_IMAGE));
    assert_eq!(planner["idle_timeout_secs"], json!(1800));

    // The link rows are in the same transaction as the row itself.
    assert_eq!(served_row_count(&app, id_of(&planner)).await, 1);
    // And the board was never touched.
    assert_eq!(task_event_count(&app, pid).await, 0);

    assert_eq!(names(&list(&app, &user, pid).await), ["default", "planner"]);
}

#[tokio::test]
async fn an_ephemeral_profile_defaults_to_no_partial_messages() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;

    let implementer = created(
        &app,
        &user,
        pid,
        &json!({ "name": "implementer", "kind": "ephemeral" }),
    )
    .await;

    assert_eq!(implementer["kind"], json!("ephemeral"));
    assert_eq!(implementer["partial_messages"], json!(false));
    // No `serves_states` either, so it takes the documented default.
    assert_eq!(implementer["serves_states"], json!(["ready"]));

    // An explicit value wins over the kind's default.
    let streaming = created(
        &app,
        &user,
        pid,
        &json!({
            "name": "streaming-implementer",
            "kind": "ephemeral",
            "partial_messages": true,
        }),
    )
    .await;

    assert_eq!(streaming["partial_messages"], json!(true));
}

#[tokio::test]
async fn every_invalid_field_is_a_bad_request() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;

    for (body, message) in [
        (
            json!({ "name": "planner", "permission_mode": "plan" }),
            "permission mode must be bypass",
        ),
        (
            json!({ "name": "planner", "mcp_tools": ["push", "delete_repo"] }),
            "unknown MCP tool \"delete_repo\"",
        ),
        (
            json!({ "name": "planner", "secrets": ["bad-name"] }),
            "secret names must be 1-128 characters matching [A-Z][A-Z0-9_]*",
        ),
        (
            json!({ "name": "planner", "idle_timeout_secs": 0 }),
            "idle timeout must be at least 1 second",
        ),
        (
            json!({ "name": "  " }),
            "profile name must be 1-64 characters",
        ),
    ] {
        let response = post(&app, &user, pid, &body).await;

        assert_error(&response, StatusCode::BAD_REQUEST, message);
    }

    // Nothing was stored by any of them.
    assert_eq!(names(&list(&app, &user, pid).await), ["default"]);
}

#[tokio::test]
async fn an_agent_credential_name_in_secrets_is_a_bad_request_on_create_and_replace() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;

    // A profile's `secrets` are the extra things its job needs; the backend's
    // credential is injected without being declared (ADR 0036; `SPEC.md`,
    // "Agent profiles"). Both of the Claude backend's names are refused, and
    // the message names the offending entry.
    for name in ["ANTHROPIC_API_KEY", "CLAUDE_CODE_OAUTH_TOKEN"] {
        let response = post(
            &app,
            &user,
            pid,
            &json!({ "name": "planner", "secrets": ["NPM_TOKEN", name] }),
        )
        .await;

        assert_error(
            &response,
            StatusCode::BAD_REQUEST,
            &format!("{name} is an agent credential and is injected automatically"),
        );
    }

    // Nothing was stored by either of them, and an ordinary list still is.
    assert_eq!(names(&list(&app, &user, pid).await), ["default"]);
    let planner = created(
        &app,
        &user,
        pid,
        &json!({ "name": "planner", "secrets": ["NPM_TOKEN", "DEPLOY_TOKEN"] }),
    )
    .await;
    assert_eq!(planner["secrets"], json!(["NPM_TOKEN", "DEPLOY_TOKEN"]));

    // The replacement runs the same rule, and the profile keeps what it had.
    let replaced = put(
        &app,
        &user,
        pid,
        id_of(&planner),
        &json!({ "name": "planner", "secrets": ["CLAUDE_CODE_OAUTH_TOKEN"] }),
    )
    .await;

    assert_error(
        &replaced,
        StatusCode::BAD_REQUEST,
        "CLAUDE_CODE_OAUTH_TOKEN is an agent credential and is injected automatically",
    );
    let stored = list(&app, &user, pid).await;
    let stored = stored
        .iter()
        .find(|profile| profile["name"] == json!("planner"))
        .expect("the planner is still there");
    assert_eq!(stored["secrets"], json!(["NPM_TOKEN", "DEPLOY_TOKEN"]));
}

#[tokio::test]
async fn a_served_state_that_is_not_a_queue_state_lists_the_ones_that_are() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;

    // A state of the project, but a `human` one: agents never claim from it.
    let human = post(
        &app,
        &user,
        pid,
        &json!({ "name": "escalator", "serves_states": ["needs_human"] }),
    )
    .await;
    human.assert_status(StatusCode::BAD_REQUEST);
    let message = human.json::<Value>()["error"]
        .as_str()
        .expect("the error carries a message")
        .to_string();
    assert!(message.contains("needs_human"), "{message}");
    assert!(
        message.contains("backlog, ready, review, merge"),
        "{message}"
    );

    // And a name that is no state of the project at all is the same mistake.
    let unknown = post(
        &app,
        &user,
        pid,
        &json!({ "name": "confused", "serves_states": ["nope"] }),
    )
    .await;
    unknown.assert_status(StatusCode::BAD_REQUEST);
    assert!(
        unknown.json::<Value>()["error"]
            .as_str()
            .expect("the error carries a message")
            .contains("backlog, ready, review, merge")
    );

    assert_eq!(names(&list(&app, &user, pid).await), ["default"]);
}

#[tokio::test]
async fn a_duplicate_name_is_a_conflict_on_create_and_on_update() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;

    created(&app, &user, pid, &json!({ "name": "planner" })).await;
    let taken = post(&app, &user, pid, &json!({ "name": "planner" })).await;
    assert_error(&taken, StatusCode::CONFLICT, "profile name already taken");

    let reviewer = created(&app, &user, pid, &json!({ "name": "reviewer" })).await;
    let renamed = put(
        &app,
        &user,
        pid,
        id_of(&reviewer),
        &json!({ "name": "planner" }),
    )
    .await;
    assert_error(&renamed, StatusCode::CONFLICT, "profile name already taken");

    // Renaming a profile to the name it already has is a no-op, not a
    // conflict.
    let unchanged = put(
        &app,
        &user,
        pid,
        id_of(&reviewer),
        &json!({ "name": "reviewer" }),
    )
    .await;
    unchanged.assert_status_ok();
}

#[tokio::test]
async fn creating_a_profile_in_an_unknown_project_is_not_found() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;

    let response = post(
        &app,
        &user,
        Uuid::new_v4(),
        &json!({ "name": "planner", "serves_states": ["backlog"] }),
    )
    .await;

    assert_error(&response, StatusCode::NOT_FOUND, "not found");
}

// ---- read, and the scope of every id ----

#[tokio::test]
async fn a_profile_is_read_back_by_id() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;
    let planner = created(
        &app,
        &user,
        pid,
        &json!({ "name": "planner", "serves_states": ["backlog", "review"] }),
    )
    .await;

    let response = app.get_as(&user, &profile_path(pid, id_of(&planner))).await;

    response.assert_status_ok();
    let read = response.json::<Value>();
    assert_eq!(read, planner);
    // Board order, not the order the caller listed them in.
    assert_eq!(read["serves_states"], json!(["backlog", "review"]));
}

#[tokio::test]
async fn an_unknown_profile_is_not_found_on_every_endpoint_that_takes_one() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;
    let unknown = Uuid::new_v4();

    let responses = [
        app.get_as(&user, &profile_path(pid, unknown)).await,
        put(&app, &user, pid, unknown, &json!({ "name": "planner" })).await,
        app.delete_as(&user, &profile_path(pid, unknown)).await,
    ];

    for response in responses {
        assert_error(&response, StatusCode::NOT_FOUND, "not found");
    }
}

#[tokio::test]
async fn a_profile_of_another_project_is_not_found_never_forbidden() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let mine = project(&app, &user, "mars").await;
    let theirs = project(&app, &user, "phobos").await;
    let planner = created(&app, &user, mine, &json!({ "name": "planner" })).await;
    let id = id_of(&planner);

    let responses = [
        app.get_as(&user, &profile_path(theirs, id)).await,
        put(&app, &user, theirs, id, &json!({ "name": "planner" })).await,
        app.delete_as(&user, &profile_path(theirs, id)).await,
        // The foreign profile is decided before the state names are resolved,
        // so an unknown state name does not turn this into a 400.
        put(
            &app,
            &user,
            theirs,
            id,
            &json!({ "name": "planner", "serves_states": ["nope"] }),
        )
        .await,
    ];

    for response in responses {
        assert_error(&response, StatusCode::NOT_FOUND, "not found");
    }

    // And the profile is untouched in the project it does belong to.
    assert_eq!(
        names(&list(&app, &user, mine).await),
        ["default", "planner"]
    );
}

// ---- update ----

#[tokio::test]
async fn an_update_is_a_full_replacement() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;
    let planner = created(
        &app,
        &user,
        pid,
        &json!({
            "name": "planner",
            "kind": "conversational",
            "model": "fake-model-id",
            "runtime": "runsc",
            "mcp_tools": ["ready", "claim"],
            "secrets": ["NPM_TOKEN"],
            "serves_states": ["backlog", "review"],
            "idle_timeout_secs": 60,
            "image": "mars-session-other:test",
        }),
    )
    .await;
    let id = id_of(&planner);
    assert_eq!(served_row_count(&app, id).await, 2);

    // Everything but the name omitted: every one of those fields takes its
    // documented default again.
    let response = put(&app, &user, pid, id, &json!({ "name": "planner" })).await;

    response.assert_status_ok();
    let replaced = response.json::<Value>();
    assert_eq!(replaced["id"], planner["id"]);
    assert_eq!(replaced["model"], Value::Null);
    assert_eq!(replaced["runtime"], Value::Null);
    assert_eq!(replaced["mcp_tools"], json!([]));
    assert_eq!(replaced["secrets"], json!([]));
    assert_eq!(replaced["serves_states"], json!(["ready"]));
    assert_eq!(replaced["idle_timeout_secs"], json!(1800));
    assert_eq!(replaced["image"], json!(TEST_IMAGE));
    assert_eq!(replaced["partial_messages"], json!(true));
    // `is_default` is the exception: omitted leaves the flag alone.
    assert_eq!(replaced["is_default"], json!(false));
    assert_eq!(served_row_count(&app, id).await, 1);
    assert_eq!(task_event_count(&app, pid).await, 0);
}

#[tokio::test]
async fn changing_the_kind_moves_partial_messages_with_it() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;
    let profile = created(&app, &user, pid, &json!({ "name": "runner" })).await;
    let id = id_of(&profile);
    assert_eq!(profile["partial_messages"], json!(true));

    // Conversational to ephemeral with nothing said: the kind's default.
    let flipped = put(
        &app,
        &user,
        pid,
        id,
        &json!({ "name": "runner", "kind": "ephemeral" }),
    )
    .await;
    flipped.assert_status_ok();
    assert_eq!(flipped.json::<Value>()["partial_messages"], json!(false));

    // With an explicit value, the value wins.
    let explicit = put(
        &app,
        &user,
        pid,
        id,
        &json!({ "name": "runner", "kind": "ephemeral", "partial_messages": true }),
    )
    .await;
    explicit.assert_status_ok();
    assert_eq!(explicit.json::<Value>()["partial_messages"], json!(true));
}

#[tokio::test]
async fn the_default_flag_transfers_and_cannot_be_cleared() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;
    let seeded = list(&app, &user, pid).await;
    let default_id = id_of(&seeded[0]);
    let planner = created(&app, &user, pid, &json!({ "name": "planner" })).await;
    let planner_id = id_of(&planner);

    let promoted = put(
        &app,
        &user,
        pid,
        planner_id,
        &json!({ "name": "planner", "is_default": true }),
    )
    .await;

    promoted.assert_status_ok();
    assert_eq!(promoted.json::<Value>()["is_default"], json!(true));

    // Exactly one default, and it moved.
    let profiles = list(&app, &user, pid).await;
    let defaults: Vec<&str> = profiles
        .iter()
        .filter(|profile| profile["is_default"] == json!(true))
        .map(|profile| profile["name"].as_str().expect("a name"))
        .collect();
    assert_eq!(defaults, ["planner"]);

    // The old default may now be edited without the flag coming back.
    let demoted = put(
        &app,
        &user,
        pid,
        default_id,
        &json!({ "name": "default", "is_default": false }),
    )
    .await;
    demoted.assert_status_ok();
    assert_eq!(demoted.json::<Value>()["is_default"], json!(false));

    // Clearing the flag on the profile that now holds it is refused: a project
    // always keeps a default.
    let cleared = put(
        &app,
        &user,
        pid,
        planner_id,
        &json!({ "name": "planner", "is_default": false }),
    )
    .await;
    assert_error(
        &cleared,
        StatusCode::CONFLICT,
        "project must keep a default profile",
    );

    // And the refusal rolled back: the flag is still there.
    let response = app.get_as(&user, &profile_path(pid, planner_id)).await;
    response.assert_status_ok();
    assert_eq!(response.json::<Value>()["is_default"], json!(true));
}

#[tokio::test]
async fn an_update_that_sends_the_whole_profile_back_keeps_the_default() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;
    let seeded = list(&app, &user, pid).await;
    let default = seeded[0].clone();
    let id = id_of(&default);

    // A client that round-trips the `Profile` it read, `is_default: true` and
    // all, is not asking for anything special.
    let response = put(&app, &user, pid, id, &default).await;

    response.assert_status_ok();
    let updated = response.json::<Value>();
    assert_eq!(updated["is_default"], json!(true));
    assert_eq!(updated["serves_states"], json!(["ready"]));
    assert_eq!(updated["name"], json!("default"));

    // And so is a `PUT` that simply omits the flag.
    let omitted = put(&app, &user, pid, id, &json!({ "name": "default" })).await;
    omitted.assert_status_ok();
    assert_eq!(omitted.json::<Value>()["is_default"], json!(true));
}

// ---- delete ----

#[tokio::test]
async fn deleting_a_profile_removes_it_and_the_states_it_served() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;
    let planner = created(
        &app,
        &user,
        pid,
        &json!({ "name": "planner", "serves_states": ["backlog"] }),
    )
    .await;
    let id = id_of(&planner);

    let response = app.delete_as(&user, &profile_path(pid, id)).await;

    response.assert_status(StatusCode::NO_CONTENT);
    assert_eq!(served_row_count(&app, id).await, 0);
    assert_eq!(names(&list(&app, &user, pid).await), ["default"]);
    assert_eq!(task_event_count(&app, pid).await, 0);

    // Deleting it again is a 404, not a second 204.
    let again = app.delete_as(&user, &profile_path(pid, id)).await;
    assert_error(&again, StatusCode::NOT_FOUND, "not found");
}

#[tokio::test]
async fn the_default_profile_and_a_profile_with_sessions_cannot_be_deleted() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;
    let default_id = id_of(&list(&app, &user, pid).await[0]);

    let default = app.delete_as(&user, &profile_path(pid, default_id)).await;
    assert_error(
        &default,
        StatusCode::CONFLICT,
        "the default profile cannot be deleted",
    );

    // A profile that has ever run a session keeps the transcript alive.
    let planner = created(&app, &user, pid, &json!({ "name": "planner" })).await;
    let planner_id = id_of(&planner);
    seed_session(&app, pid, planner_id).await;

    let used = app.delete_as(&user, &profile_path(pid, planner_id)).await;
    assert_error(&used, StatusCode::CONFLICT, "profile has sessions");

    // Neither refusal removed anything.
    assert_eq!(names(&list(&app, &user, pid).await), ["default", "planner"]);
}

#[tokio::test]
async fn deleting_from_an_unknown_project_is_not_found() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;

    let response = app
        .delete_as(&user, &profile_path(Uuid::new_v4(), Uuid::new_v4()))
        .await;

    assert_error(&response, StatusCode::NOT_FOUND, "not found");
}
