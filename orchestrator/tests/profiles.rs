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
use common::projects::{id_of, password_change_required, project, signed_in, unauthorized};
use common::{AuthenticatedUser, TestApp};
use serde_json::{Value, json};
use uuid::Uuid;

/// The image `SESSION_IMAGE_DEFAULT` carries in a test app, which is the image
/// a profile that names none is stored with.
const TEST_IMAGE: &str = "mars-session-stub:test";

/// The twenty-four fields of `Profile` (`SPEC.md`, "Agent profiles"), sorted.
const PROFILE_FIELDS: [&str; 24] = [
    "auto_launch",
    "backend",
    "created_at",
    "id",
    "idle_timeout_secs",
    "image",
    "is_default",
    "kind",
    "last_scheduled_at",
    "max_concurrent",
    "mcp_tools",
    "model",
    "name",
    "next_scheduled_at",
    "partial_messages",
    "permission_mode",
    "project_id",
    "runtime",
    "schedule_cron",
    "schedule_prompt",
    "secrets",
    "serves_states",
    "system_prompt",
    "updated_at",
];

// ---- helpers ----
//
// Signing in, the documented refusals and the project the profiles hang on
// are `tests/common/projects.rs`'s, shared with the other project route
// suites.

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

/// The names of the profiles in a listing, in the order they came back.
fn names(profiles: &[Value]) -> Vec<&str> {
    profiles
        .iter()
        .map(|profile| profile["name"].as_str().expect("a profile carries a name"))
        .collect()
}

/// The four profiles `POST /api/projects` seeds, in listing order (`SPEC.md`,
/// "Role profile templates"). This suite's own profiles are named so that
/// they never collide with these.
const SEEDED: [&str; 4] = ["claude", "planner", "implementer", "reviewer"];

/// The seeded profile that carries `is_default`.
const SEEDED_DEFAULT: &str = "claude";

/// `SEEDED` followed by `extra`: the listing is oldest first, and anything
/// this suite creates is newer than the ones the project was seeded with.
fn seeded_and(extra: &[&'static str]) -> Vec<&'static str> {
    SEEDED
        .iter()
        .copied()
        .chain(extra.iter().copied())
        .collect()
}

/// The project's default profile as the listing shows it.
fn default_profile(profiles: &[Value]) -> &Value {
    profiles
        .iter()
        .find(|profile| profile["is_default"] == json!(true))
        .expect("a project always has a default profile")
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
    let body = json!({ "name": "scout" });

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
    let body = json!({ "name": "scout" });

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
async fn listing_shows_the_four_seeded_profiles_in_the_documented_shape() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;

    let profiles = list(&app, &user, pid).await;

    // The seeded templates, oldest first: the default, then the roles in the
    // order a task travels through them (`SPEC.md`, "Role profile
    // templates").
    assert_eq!(names(&profiles), SEEDED);

    let default = default_profile(&profiles);
    assert_eq!(default["name"], json!(SEEDED_DEFAULT));
    assert_eq!(default["project_id"], json!(pid.to_string()));
    assert_eq!(default["kind"], json!("conversational"));
    assert_eq!(default["backend"], json!("claude"));
    assert_eq!(default["model"], Value::Null);
    assert_eq!(default["permission_mode"], json!("bypass"));
    assert_eq!(default["image"], json!(TEST_IMAGE));
    assert_eq!(default["runtime"], Value::Null);
    assert_eq!(default["mcp_tools"], json!([]));
    assert_eq!(default["secrets"], json!([]));
    // The default serves no queue, so it never sees a role's work (ADR 0051).
    assert_eq!(default["serves_states"], json!([]));
    assert_eq!(default["auto_launch"], json!(false));
    assert_eq!(default["partial_messages"], json!(true));
    assert_eq!(default["idle_timeout_secs"], json!(1800));
    assert_eq!(default["is_default"], json!(true));
    assert!(default["created_at"].is_string());
    assert!(default["updated_at"].is_string());

    // Every seeded profile carries its role prompt; the texts themselves are
    // `tests/profile_templates.rs`.
    for profile in &profiles {
        let prompt = profile["system_prompt"]
            .as_str()
            .expect("a seeded profile carries its role prompt");
        assert!(!prompt.trim().is_empty());
    }

    // Exactly the documented twenty fields, and nothing the row might add.
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
    created(&app, &user, pid, &json!({ "name": "scribe" })).await;
    created(&app, &user, pid, &json!({ "name": "scout" })).await;
    created(&app, &user, pid, &json!({ "name": "archivist" })).await;

    assert_eq!(
        names(&list(&app, &user, pid).await),
        seeded_and(&["scribe", "scout", "archivist"])
    );
}

// ---- create ----

#[tokio::test]
async fn creating_a_profile_stores_it_with_the_states_it_serves() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;

    let scout = created(
        &app,
        &user,
        pid,
        &json!({
            "name": "scout",
            "serves_states": ["backlog"],
            "system_prompt": "Break the request down into tasks.",
        }),
    )
    .await;

    assert_eq!(scout["name"], json!("scout"));
    assert_eq!(scout["kind"], json!("conversational"));
    assert_eq!(scout["serves_states"], json!(["backlog"]));
    assert_eq!(
        scout["system_prompt"],
        json!("Break the request down into tasks.")
    );
    // Conversational, and nothing was said, so streaming is on.
    assert_eq!(scout["partial_messages"], json!(true));
    // The seeded profile keeps the flag.
    assert_eq!(scout["is_default"], json!(false));
    assert_eq!(scout["image"], json!(TEST_IMAGE));
    assert_eq!(scout["idle_timeout_secs"], json!(1800));

    // The link rows are in the same transaction as the row itself.
    assert_eq!(served_row_count(&app, id_of(&scout)).await, 1);
    // And the board was never touched.
    assert_eq!(task_event_count(&app, pid).await, 0);

    assert_eq!(names(&list(&app, &user, pid).await), seeded_and(&["scout"]));
}

#[tokio::test]
async fn an_ephemeral_profile_defaults_to_no_partial_messages() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;

    let one_shot = created(
        &app,
        &user,
        pid,
        &json!({ "name": "one-shot", "kind": "ephemeral" }),
    )
    .await;

    assert_eq!(one_shot["kind"], json!("ephemeral"));
    assert_eq!(one_shot["partial_messages"], json!(false));
    // No `serves_states` either, so it takes the documented default.
    assert_eq!(one_shot["serves_states"], json!(["ready"]));

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
            json!({ "name": "scout", "permission_mode": "plan" }),
            "permission mode must be bypass",
        ),
        (
            json!({ "name": "scout", "mcp_tools": ["push", "delete_repo"] }),
            "unknown MCP tool \"delete_repo\"",
        ),
        (
            json!({ "name": "scout", "secrets": ["bad-name"] }),
            "secret names must be 1-128 characters matching [A-Z][A-Z0-9_]*",
        ),
        (
            json!({ "name": "scout", "idle_timeout_secs": 0 }),
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
    assert_eq!(names(&list(&app, &user, pid).await), SEEDED);
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
            &json!({ "name": "scout", "secrets": ["NPM_TOKEN", name] }),
        )
        .await;

        assert_error(
            &response,
            StatusCode::BAD_REQUEST,
            &format!("{name} is an agent credential and is injected automatically"),
        );
    }

    // Nothing was stored by either of them, and an ordinary list still is.
    assert_eq!(names(&list(&app, &user, pid).await), SEEDED);
    let scout = created(
        &app,
        &user,
        pid,
        &json!({ "name": "scout", "secrets": ["NPM_TOKEN", "DEPLOY_TOKEN"] }),
    )
    .await;
    assert_eq!(scout["secrets"], json!(["NPM_TOKEN", "DEPLOY_TOKEN"]));

    // The replacement runs the same rule, and the profile keeps what it had.
    let replaced = put(
        &app,
        &user,
        pid,
        id_of(&scout),
        &json!({ "name": "scout", "secrets": ["CLAUDE_CODE_OAUTH_TOKEN"] }),
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
        .find(|profile| profile["name"] == json!("scout"))
        .expect("the scout is still there");
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

    assert_eq!(names(&list(&app, &user, pid).await), SEEDED);
}

#[tokio::test]
async fn a_duplicate_name_is_a_conflict_on_create_and_on_update() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;

    created(&app, &user, pid, &json!({ "name": "scout" })).await;
    let taken = post(&app, &user, pid, &json!({ "name": "scout" })).await;
    assert_error(&taken, StatusCode::CONFLICT, "profile name already taken");

    let scribe = created(&app, &user, pid, &json!({ "name": "scribe" })).await;
    let renamed = put(
        &app,
        &user,
        pid,
        id_of(&scribe),
        &json!({ "name": "scout" }),
    )
    .await;
    assert_error(&renamed, StatusCode::CONFLICT, "profile name already taken");

    // Renaming a profile to the name it already has is a no-op, not a
    // conflict.
    let unchanged = put(
        &app,
        &user,
        pid,
        id_of(&scribe),
        &json!({ "name": "scribe" }),
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
        &json!({ "name": "scout", "serves_states": ["backlog"] }),
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
    let scout = created(
        &app,
        &user,
        pid,
        &json!({ "name": "scout", "serves_states": ["backlog", "review"] }),
    )
    .await;

    let response = app.get_as(&user, &profile_path(pid, id_of(&scout))).await;

    response.assert_status_ok();
    let read = response.json::<Value>();
    assert_eq!(read, scout);
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
        put(&app, &user, pid, unknown, &json!({ "name": "scout" })).await,
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
    let scout = created(&app, &user, mine, &json!({ "name": "scout" })).await;
    let id = id_of(&scout);

    let responses = [
        app.get_as(&user, &profile_path(theirs, id)).await,
        put(&app, &user, theirs, id, &json!({ "name": "scout" })).await,
        app.delete_as(&user, &profile_path(theirs, id)).await,
        // The foreign profile is decided before the state names are resolved,
        // so an unknown state name does not turn this into a 400.
        put(
            &app,
            &user,
            theirs,
            id,
            &json!({ "name": "scout", "serves_states": ["nope"] }),
        )
        .await,
    ];

    for response in responses {
        assert_error(&response, StatusCode::NOT_FOUND, "not found");
    }

    // And the profile is untouched in the project it does belong to.
    assert_eq!(
        names(&list(&app, &user, mine).await),
        seeded_and(&["scout"])
    );
}

// ---- update ----

#[tokio::test]
async fn an_update_is_a_full_replacement() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;
    let scout = created(
        &app,
        &user,
        pid,
        &json!({
            "name": "scout",
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
    let id = id_of(&scout);
    assert_eq!(served_row_count(&app, id).await, 2);

    // Everything but the name omitted: every one of those fields takes its
    // documented default again.
    let response = put(&app, &user, pid, id, &json!({ "name": "scout" })).await;

    response.assert_status_ok();
    let replaced = response.json::<Value>();
    assert_eq!(replaced["id"], scout["id"]);
    assert_eq!(replaced["model"], Value::Null);
    assert_eq!(replaced["runtime"], Value::Null);
    assert_eq!(replaced["mcp_tools"], json!([]));
    assert_eq!(replaced["secrets"], json!([]));
    assert_eq!(replaced["serves_states"], json!(["ready"]));
    assert_eq!(replaced["idle_timeout_secs"], json!(1800));
    assert_eq!(replaced["image"], json!(TEST_IMAGE));
    assert_eq!(replaced["partial_messages"], json!(true));
    // A `PUT` is a replacement here too: omitted means off and 1, not "keep"
    // (`SPEC.md`, "Agent profiles").
    assert_eq!(replaced["auto_launch"], json!(false));
    assert_eq!(replaced["max_concurrent"], json!(1));
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
    let default_id = id_of(default_profile(&seeded));
    let scout = created(&app, &user, pid, &json!({ "name": "scout" })).await;
    let scout_id = id_of(&scout);

    let promoted = put(
        &app,
        &user,
        pid,
        scout_id,
        &json!({ "name": "scout", "is_default": true }),
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
    assert_eq!(defaults, ["scout"]);

    // The old default may now be edited without the flag coming back.
    let demoted = put(
        &app,
        &user,
        pid,
        default_id,
        &json!({ "name": SEEDED_DEFAULT, "is_default": false }),
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
        scout_id,
        &json!({ "name": "scout", "is_default": false }),
    )
    .await;
    assert_error(
        &cleared,
        StatusCode::CONFLICT,
        "project must keep a default profile",
    );

    // And the refusal rolled back: the flag is still there.
    let response = app.get_as(&user, &profile_path(pid, scout_id)).await;
    response.assert_status_ok();
    assert_eq!(response.json::<Value>()["is_default"], json!(true));
}

#[tokio::test]
async fn an_update_that_sends_the_whole_profile_back_keeps_the_default() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;
    let seeded = list(&app, &user, pid).await;
    let default = default_profile(&seeded).clone();
    let id = id_of(&default);

    // A client that round-trips the `Profile` it read, `is_default: true` and
    // all, is not asking for anything special.
    let response = put(&app, &user, pid, id, &default).await;

    response.assert_status_ok();
    let updated = response.json::<Value>();
    assert_eq!(updated["is_default"], json!(true));
    // The seeded default serves nothing, and an empty list sent back is
    // kept rather than read as "omitted".
    assert_eq!(updated["serves_states"], json!([]));
    assert_eq!(updated["name"], json!(SEEDED_DEFAULT));

    // And so is a `PUT` that simply omits the flag.
    let omitted = put(&app, &user, pid, id, &json!({ "name": SEEDED_DEFAULT })).await;
    omitted.assert_status_ok();
    assert_eq!(omitted.json::<Value>()["is_default"], json!(true));
}

// ---- delete ----

#[tokio::test]
async fn deleting_a_profile_removes_it_and_the_states_it_served() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;
    let scout = created(
        &app,
        &user,
        pid,
        &json!({ "name": "scout", "serves_states": ["backlog"] }),
    )
    .await;
    let id = id_of(&scout);

    let response = app.delete_as(&user, &profile_path(pid, id)).await;

    response.assert_status(StatusCode::NO_CONTENT);
    assert_eq!(served_row_count(&app, id).await, 0);
    assert_eq!(names(&list(&app, &user, pid).await), SEEDED);
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
    let default_id = id_of(default_profile(&list(&app, &user, pid).await));

    let default = app.delete_as(&user, &profile_path(pid, default_id)).await;
    assert_error(
        &default,
        StatusCode::CONFLICT,
        "the default profile cannot be deleted",
    );

    // A profile that has ever run a session keeps the transcript alive.
    let scout = created(&app, &user, pid, &json!({ "name": "scout" })).await;
    let scout_id = id_of(&scout);
    seed_session(&app, pid, scout_id).await;

    let used = app.delete_as(&user, &profile_path(pid, scout_id)).await;
    assert_error(&used, StatusCode::CONFLICT, "profile has sessions");

    // Neither refusal removed anything.
    assert_eq!(names(&list(&app, &user, pid).await), seeded_and(&["scout"]));
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

// ---- unattended launching (`ARCHITECTURE.md`, "Unattended launches") ----

/// Not a real credential: the value every credential row below holds (rule 3).
const FAKE_CREDENTIAL: &str = "fake-value-not-a-credential";

/// The Claude adapter's preferred credential name (`agent::credential_names`).
const OAUTH_TOKEN: &str = "CLAUDE_CODE_OAUTH_TOKEN";

/// The refusal a save without a resolvable credential gets
/// (`secrets::NO_UNATTENDED_CREDENTIAL`).
const NO_CREDENTIAL: &str =
    "auto_launch requires this backend's agent credential at global or project scope";

/// The refusal `auto_launch` on a profile that talks to a person gets.
const NOT_EPHEMERAL: &str = "auto_launch requires an ephemeral profile";

/// The refusal a cap below one gets.
const CAP_TOO_LOW: &str = "max_concurrent must be at least 1";

/// Store an agent credential at `scope` through the documented endpoint.
async fn seed_credential(
    app: &TestApp,
    caller: &AuthenticatedUser,
    scope: &str,
    scope_id: Option<Uuid>,
) {
    let response = app
        .post_as(caller, "/api/secrets")
        .json(&json!({
            "scope": scope,
            "scope_id": scope_id.map(|id| id.to_string()),
            "name": OAUTH_TOKEN,
            "value": FAKE_CREDENTIAL,
        }))
        .await;
    response.assert_status(StatusCode::CREATED);
}

/// The body of an `auto_launch` profile, which must be ephemeral.
fn auto_launch_body(name: &str) -> Value {
    json!({ "name": name, "kind": "ephemeral", "auto_launch": true, "max_concurrent": 2 })
}

#[tokio::test]
async fn the_unattended_fields_default_to_off_and_one() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;

    let scout = created(&app, &user, pid, &json!({ "name": "scout" })).await;
    assert_eq!(scout["auto_launch"], json!(false));
    assert_eq!(scout["max_concurrent"], json!(1));

    // The seeded implementer and reviewer launch themselves, and are
    // ephemeral so that they may; the default and the planner talk to a
    // person (ADR 0051). They are seeded that way whether or not a
    // credential exists — the dispatcher checks at launch.
    for profile in list(&app, &user, pid).await {
        let auto_launched =
            profile["name"] == json!("implementer") || profile["name"] == json!("reviewer");
        assert_eq!(
            profile["auto_launch"],
            json!(auto_launched),
            "{}: auto_launch",
            profile["name"]
        );
        assert_eq!(
            profile["kind"],
            json!(if auto_launched {
                "ephemeral"
            } else {
                "conversational"
            }),
            "{}: kind",
            profile["name"]
        );
        assert_eq!(profile["max_concurrent"], json!(1));
    }
}

/// Saving a seeded auto-launched profile without an agent credential the
/// jobs could use is the create endpoint's ordinary 400 — the seeding does
/// not relax the rule for a later edit (ADR 0051) — and switching
/// `auto_launch` off is what saves it.
#[tokio::test]
async fn editing_a_seeded_auto_launched_profile_without_a_credential_is_refused() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;

    let implementer = list(&app, &user, pid)
        .await
        .into_iter()
        .find(|profile| profile["name"] == json!("implementer"))
        .expect("a new project has the implementer");

    let mut body = implementer.clone();
    body["model"] = json!("sonnet");
    let refused = put(&app, &user, pid, id_of(&implementer), &body).await;
    assert_error(&refused, StatusCode::BAD_REQUEST, NO_CREDENTIAL);

    body["auto_launch"] = json!(false);
    let saved = put(&app, &user, pid, id_of(&implementer), &body).await;
    saved.assert_status_ok();
    let saved = saved.json::<Value>();
    assert_eq!(saved["auto_launch"], json!(false));
    assert_eq!(saved["model"], json!("sonnet"));
}

#[tokio::test]
async fn a_project_scoped_credential_lets_an_ephemeral_profile_launch_itself() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;

    // Without a credential the jobs could use, the save is refused before
    // anything is written.
    let refused = post(&app, &user, pid, &auto_launch_body("scanner")).await;
    assert_error(&refused, StatusCode::BAD_REQUEST, NO_CREDENTIAL);
    assert_eq!(names(&list(&app, &user, pid).await), seeded_and(&[]));

    // A `user`-scope row is the caller's own and can never resolve for a
    // launch whose `created_by` is NULL, so it does not satisfy the rule.
    seed_credential(&app, &user, "user", Some(user.user.id)).await;
    let still_refused = post(&app, &user, pid, &auto_launch_body("scanner")).await;
    assert_error(&still_refused, StatusCode::BAD_REQUEST, NO_CREDENTIAL);

    seed_credential(&app, &user, "project", Some(pid)).await;
    let stored = created(&app, &user, pid, &auto_launch_body("scanner")).await;
    assert_eq!(stored["auto_launch"], json!(true));
    assert_eq!(stored["max_concurrent"], json!(2));
    assert_eq!(stored["kind"], json!("ephemeral"));

    // And the stored row answers the same on a re-read.
    let id = id_of(&stored);
    let read = app.get_as(&user, &profile_path(pid, id)).await;
    read.assert_status_ok();
    let body = read.json::<Value>();
    assert_eq!(body["auto_launch"], json!(true));
    assert_eq!(body["max_concurrent"], json!(2));
}

#[tokio::test]
async fn a_global_credential_satisfies_the_rule_for_every_project() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;

    seed_credential(&app, &user, "global", None).await;

    // On create, and on the update of a profile that was saved without it.
    let stored = created(&app, &user, pid, &auto_launch_body("scanner")).await;
    assert_eq!(stored["auto_launch"], json!(true));

    let scout = created(
        &app,
        &user,
        pid,
        &json!({ "name": "scout", "kind": "ephemeral" }),
    )
    .await;
    let response = put(&app, &user, pid, id_of(&scout), &auto_launch_body("scout")).await;
    response.assert_status_ok();
    assert_eq!(response.json::<Value>()["auto_launch"], json!(true));
}

#[tokio::test]
async fn an_update_that_turns_auto_launch_on_needs_the_credential_too() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;
    let scout = created(
        &app,
        &user,
        pid,
        &json!({ "name": "scout", "kind": "ephemeral" }),
    )
    .await;
    let id = id_of(&scout);

    let refused = put(&app, &user, pid, id, &auto_launch_body("scout")).await;
    assert_error(&refused, StatusCode::BAD_REQUEST, NO_CREDENTIAL);

    // Nothing was written: the refusal happens before the transaction opens.
    let read = app.get_as(&user, &profile_path(pid, id)).await;
    read.assert_status_ok();
    assert_eq!(read.json::<Value>()["auto_launch"], json!(false));

    seed_credential(&app, &user, "project", Some(pid)).await;
    let accepted = put(&app, &user, pid, id, &auto_launch_body("scout")).await;
    accepted.assert_status_ok();
    assert_eq!(accepted.json::<Value>()["auto_launch"], json!(true));
}

#[tokio::test]
async fn auto_launch_is_refused_on_a_conversational_profile() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;
    seed_credential(&app, &user, "project", Some(pid)).await;

    // The kind is checked by the model, before the credential lookup, so a
    // project that *has* a credential still gets this answer.
    for body in [
        json!({ "name": "scout", "auto_launch": true }),
        json!({ "name": "scout", "kind": "conversational", "auto_launch": true }),
    ] {
        let response = post(&app, &user, pid, &body).await;
        assert_error(&response, StatusCode::BAD_REQUEST, NOT_EPHEMERAL);
    }

    // The same rule on the way through `PUT`.
    let scout = created(&app, &user, pid, &json!({ "name": "scout" })).await;
    let response = put(
        &app,
        &user,
        pid,
        id_of(&scout),
        &json!({ "name": "scout", "auto_launch": true }),
    )
    .await;
    assert_error(&response, StatusCode::BAD_REQUEST, NOT_EPHEMERAL);
}

#[tokio::test]
async fn max_concurrent_is_valid_on_any_ephemeral_profile_and_never_below_one() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;

    // The scheduler reads it too, so it needs no `auto_launch` beside it.
    let scout = created(
        &app,
        &user,
        pid,
        &json!({ "name": "scout", "kind": "ephemeral", "max_concurrent": 4 }),
    )
    .await;
    assert_eq!(scout["max_concurrent"], json!(4));
    assert_eq!(scout["auto_launch"], json!(false));

    for cap in [0, -1] {
        let response = post(
            &app,
            &user,
            pid,
            &json!({ "name": "capped", "kind": "ephemeral", "max_concurrent": cap }),
        )
        .await;
        assert_error(&response, StatusCode::BAD_REQUEST, CAP_TOO_LOW);

        let updated = put(
            &app,
            &user,
            pid,
            id_of(&scout),
            &json!({ "name": "scout", "kind": "ephemeral", "max_concurrent": cap }),
        )
        .await;
        assert_error(&updated, StatusCode::BAD_REQUEST, CAP_TOO_LOW);
    }

    assert_eq!(names(&list(&app, &user, pid).await), seeded_and(&["scout"]));
}

// ---- schedules (`ARCHITECTURE.md`, "Scheduled agents"; ADR 0043) ----

/// The refusal a schedule without a credential the jobs could use gets
/// (`secrets::NO_UNATTENDED_CREDENTIAL_FOR_SCHEDULE`).
const NO_CREDENTIAL_FOR_SCHEDULE: &str =
    "schedule_cron requires this backend's agent credential at global or project scope";

/// The refusal a schedule on a profile that talks to a person gets.
const SCHEDULE_NOT_EPHEMERAL: &str = "schedule_cron requires an ephemeral profile";

/// The refusal an expression that is not five plain fields gets.
const BAD_EXPRESSION: &str = "schedule_cron must be a 5-field cron expression (minute hour \
     day-of-month month day-of-week) evaluated in UTC, with no seconds field, no year field \
     and no @-form";

/// The refusal an expression with no prompt gets.
const NO_PROMPT: &str = "schedule_prompt is required when schedule_cron is set";

/// The `next_scheduled_at` of a response, which must be there.
fn next_run(profile: &Value) -> chrono::DateTime<chrono::Utc> {
    profile["next_scheduled_at"]
        .as_str()
        .expect("a scheduled profile carries its next run")
        .parse()
        .expect("next_scheduled_at is RFC 3339")
}

/// The body of a scheduled profile, which must be ephemeral.
fn scheduled_body(name: &str, cron: &str) -> Value {
    json!({
        "name": name,
        "kind": "ephemeral",
        "schedule_cron": cron,
        "schedule_prompt": "scan the repository for tech debt and file tasks",
    })
}

#[tokio::test]
async fn a_profile_without_a_schedule_carries_four_nulls() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;

    let scout = created(&app, &user, pid, &json!({ "name": "scout" })).await;
    for field in [
        "schedule_cron",
        "schedule_prompt",
        "last_scheduled_at",
        "next_scheduled_at",
    ] {
        assert_eq!(scout[field], Value::Null, "{field} is not null");
    }

    // None of the seeded role profiles is scheduled either.
    for profile in list(&app, &user, pid).await {
        assert_eq!(
            profile["schedule_cron"],
            Value::Null,
            "{} is scheduled",
            profile["name"]
        );
    }
}

#[tokio::test]
async fn a_schedule_is_set_updated_and_cleared_through_the_profile() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;
    seed_credential(&app, &user, "project", Some(pid)).await;

    let stored = created(&app, &user, pid, &scheduled_body("scanner", "30 3 * * *")).await;
    assert_eq!(stored["schedule_cron"], json!("30 3 * * *"));
    assert_eq!(
        stored["schedule_prompt"],
        json!("scan the repository for tech debt and file tasks")
    );
    assert_eq!(stored["last_scheduled_at"], Value::Null);
    let next = next_run(&stored);
    // 03:30 UTC, and ahead of the request that asked for it.
    assert!(next > chrono::Utc::now());
    assert_eq!(
        next.time(),
        chrono::NaiveTime::from_hms_opt(3, 30, 0).unwrap()
    );

    // A changed expression is a changed next run.
    let id = id_of(&stored);
    let response = put(
        &app,
        &user,
        pid,
        id,
        &scheduled_body("scanner", "0 * * * *"),
    )
    .await;
    response.assert_status_ok();
    let updated = response.json::<Value>();
    assert_eq!(updated["schedule_cron"], json!("0 * * * *"));
    let next = next_run(&updated);
    assert_eq!(chrono::Timelike::minute(&next), 0);
    assert!(next <= chrono::Utc::now() + chrono::Duration::hours(1));

    // `PUT` replaces the whole profile, so a body with no schedule clears one.
    let response = put(
        &app,
        &user,
        pid,
        id,
        &json!({ "name": "scanner", "kind": "ephemeral" }),
    )
    .await;
    response.assert_status_ok();
    let cleared = response.json::<Value>();
    for field in [
        "schedule_cron",
        "schedule_prompt",
        "last_scheduled_at",
        "next_scheduled_at",
    ] {
        assert_eq!(cleared[field], Value::Null, "{field} survived the clear");
    }

    // And the stored row answers the same on a re-read.
    let read = app.get_as(&user, &profile_path(pid, id)).await;
    read.assert_status_ok();
    assert_eq!(read.json::<Value>()["schedule_cron"], Value::Null);
}

#[tokio::test]
async fn clearing_a_schedule_clears_the_scheduler_s_guard() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;
    seed_credential(&app, &user, "project", Some(pid)).await;

    let stored = created(&app, &user, pid, &scheduled_body("scanner", "30 3 * * *")).await;
    let id = id_of(&stored);

    // The column no endpoint writes: the scheduler's own, set here directly
    // because only the job that does not exist yet would otherwise set it.
    sqlx::query("UPDATE agent_profiles SET last_scheduled_at = now() WHERE id = $1")
        .bind(id)
        .execute(&app.pool)
        .await
        .expect("the guard is written");

    let read = app.get_as(&user, &profile_path(pid, id)).await;
    read.assert_status_ok();
    assert_ne!(read.json::<Value>()["last_scheduled_at"], Value::Null);

    // A save that only changes the expression keeps it: that tick did fire.
    let kept = put(
        &app,
        &user,
        pid,
        id,
        &scheduled_body("scanner", "0 4 * * *"),
    )
    .await;
    kept.assert_status_ok();
    assert_ne!(kept.json::<Value>()["last_scheduled_at"], Value::Null);

    // Clearing the schedule takes it with it.
    let cleared = put(
        &app,
        &user,
        pid,
        id,
        &json!({ "name": "scanner", "kind": "ephemeral" }),
    )
    .await;
    cleared.assert_status_ok();
    assert_eq!(cleared.json::<Value>()["last_scheduled_at"], Value::Null);
}

#[tokio::test]
async fn last_scheduled_at_is_read_only_over_rest() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;
    seed_credential(&app, &user, "project", Some(pid)).await;

    let mut body = scheduled_body("scanner", "30 3 * * *");
    body["last_scheduled_at"] = json!("2020-01-01T00:00:00Z");
    body["next_scheduled_at"] = json!("2020-01-01T00:00:00Z");

    // The extra keys are ignored rather than refused, as every unknown key is.
    let stored = created(&app, &user, pid, &body).await;
    assert_eq!(stored["last_scheduled_at"], Value::Null);
    assert!(next_run(&stored) > chrono::Utc::now());
}

#[tokio::test]
async fn a_schedule_needs_a_credential_the_jobs_can_use() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;

    let refused = post(&app, &user, pid, &scheduled_body("scanner", "30 3 * * *")).await;
    assert_error(
        &refused,
        StatusCode::BAD_REQUEST,
        NO_CREDENTIAL_FOR_SCHEDULE,
    );
    assert_eq!(names(&list(&app, &user, pid).await), seeded_and(&[]));

    // The caller's own row is not one a launch with no user could resolve.
    seed_credential(&app, &user, "user", Some(user.user.id)).await;
    let still_refused = post(&app, &user, pid, &scheduled_body("scanner", "30 3 * * *")).await;
    assert_error(
        &still_refused,
        StatusCode::BAD_REQUEST,
        NO_CREDENTIAL_FOR_SCHEDULE,
    );

    seed_credential(&app, &user, "global", None).await;
    let stored = created(&app, &user, pid, &scheduled_body("scanner", "30 3 * * *")).await;
    assert_eq!(stored["schedule_cron"], json!("30 3 * * *"));
}

#[tokio::test]
async fn a_schedule_is_refused_on_a_conversational_profile() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;
    seed_credential(&app, &user, "project", Some(pid)).await;

    // The kind is checked by the model, before the credential lookup, so a
    // project that *has* a credential still gets this answer.
    let mut body = scheduled_body("scanner", "30 3 * * *");
    body["kind"] = json!("conversational");
    let response = post(&app, &user, pid, &body).await;
    assert_error(&response, StatusCode::BAD_REQUEST, SCHEDULE_NOT_EPHEMERAL);

    // And the same rule on the way through `PUT`.
    let scout = created(&app, &user, pid, &json!({ "name": "scout" })).await;
    let response = put(&app, &user, pid, id_of(&scout), &body).await;
    assert_error(&response, StatusCode::BAD_REQUEST, SCHEDULE_NOT_EPHEMERAL);
}

#[tokio::test]
async fn a_bad_expression_or_a_missing_prompt_is_refused() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;
    seed_credential(&app, &user, "project", Some(pid)).await;

    for cron in ["0 30 3 * * *", "@daily", "* * * *", "30 3 * * * 2026"] {
        let response = post(&app, &user, pid, &scheduled_body("scanner", cron)).await;
        assert_error(&response, StatusCode::BAD_REQUEST, BAD_EXPRESSION);
    }

    let garbage = post(
        &app,
        &user,
        pid,
        &scheduled_body("scanner", "nope not a cron here"),
    )
    .await;
    garbage.assert_status(StatusCode::BAD_REQUEST);
    let message = garbage.json::<Value>()["error"]
        .as_str()
        .expect("an error carries a message")
        .to_string();
    assert!(
        message.starts_with("schedule_cron is not a valid cron expression"),
        "{message}"
    );

    for prompt in [Value::Null, json!(""), json!("   ")] {
        let mut body = scheduled_body("scanner", "30 3 * * *");
        body["schedule_prompt"] = prompt.clone();
        let response = post(&app, &user, pid, &body).await;
        assert_error(&response, StatusCode::BAD_REQUEST, NO_PROMPT);
    }

    // A prompt with nothing to run it is refused the other way round.
    let response = post(
        &app,
        &user,
        pid,
        &json!({ "name": "scanner", "kind": "ephemeral", "schedule_prompt": "scan" }),
    )
    .await;
    assert_error(
        &response,
        StatusCode::BAD_REQUEST,
        "schedule_prompt requires schedule_cron",
    );

    // Nothing was written by any of them.
    assert_eq!(names(&list(&app, &user, pid).await), seeded_and(&[]));
}
