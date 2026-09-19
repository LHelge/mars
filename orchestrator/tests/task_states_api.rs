//! `/api/projects/{pid}/task-states` through the real router (`SPEC.md`,
//! "Task states" and "TaskEvent"; `docs/data-model.md`, `task_states`;
//! `ARCHITECTURE.md`, "Task tracker" → "State is a queue, defined per
//! project").
//!
//! The board's columns are the one piece of tracker configuration a user edits
//! directly, and almost everything that can go wrong with an edit is a refusal
//! rather than an error: a state's kind never changes, a project keeps at
//! least one queue state, exactly one human state and at least one terminal
//! state, and a column holding tasks is not removed out from under them.
//!
//! What is asserted here is the endpoint's contract — the statuses, the
//! `TaskState` body, the ordering and re-packing of positions, the exact
//! refusal messages, and the single `states_changed` event each successful
//! change appends with `task_id` null and the full list after the change,
//! against none at all on the rejected ones.
//!
//! Every credential-shaped value is an obviously fake stand-in (rule 3).
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use axum::http::StatusCode;
use axum_test::TestResponse;
use common::{AuthenticatedUser, TestApp};
use mars_orchestrator::projects::{NewProjectRequest, create_project};
use mars_orchestrator::repositories::TaskRepository;
use serde_json::{Value, json};
use uuid::Uuid;

/// Not a real remote: `.invalid` can never resolve (rule 3).
const TEST_REMOTE: &str = "https://example.invalid/org/repo.git";

/// The seeded set, in board order (`docs/data-model.md`, `task_states`).
const DEFAULTS: [&str; 7] = [
    "backlog",
    "ready",
    "review",
    "merge",
    "needs_human",
    "done",
    "cancelled",
];

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

/// Assert the documented error body (`SPEC.md`, "REST API").
fn assert_error(response: &TestResponse, status: StatusCode, message: &str) {
    response.assert_status(status);
    response.assert_json(&json!({ "status": status.as_u16(), "error": message }));
}

/// A project with the seeded default states, created by `user`.
async fn project(app: &TestApp, user: &AuthenticatedUser) -> Uuid {
    create_project(
        &app.state,
        NewProjectRequest {
            name: "mars".to_string(),
            remote_url: TEST_REMOTE.to_string(),
            default_branch: None,
            credential: None,
        },
        user.user.id,
    )
    .await
    .expect("the project is created")
    .id
}

/// `/api/projects/{pid}/task-states`.
fn states_path(pid: Uuid) -> String {
    format!("/api/projects/{pid}/task-states")
}

/// `/api/projects/{pid}/task-states/{name}`.
fn state_path(pid: Uuid, name: &str) -> String {
    format!("/api/projects/{pid}/task-states/{name}")
}

/// The project's states as the endpoint answers them.
async fn list(app: &TestApp, user: &AuthenticatedUser, pid: Uuid) -> Vec<Value> {
    let response = app.get_as(user, &states_path(pid)).await;
    response.assert_status(StatusCode::OK);
    response.json::<Vec<Value>>()
}

/// The names of the project's states, in the order the endpoint answers.
async fn names(app: &TestApp, user: &AuthenticatedUser, pid: Uuid) -> Vec<String> {
    list(app, user, pid)
        .await
        .iter()
        .map(|state| state["name"].as_str().expect("a name").to_string())
        .collect()
}

/// The `(name, position)` pairs, for asserting that positions stay packed.
async fn positions(app: &TestApp, user: &AuthenticatedUser, pid: Uuid) -> Vec<(String, i64)> {
    list(app, user, pid)
        .await
        .iter()
        .map(|state| {
            (
                state["name"].as_str().expect("a name").to_string(),
                state["position"].as_i64().expect("a position"),
            )
        })
        .collect()
}

/// The project's event stream from the beginning, as `(kind, task_id,
/// payload)`.
async fn events(app: &TestApp, pid: Uuid) -> Vec<(String, Option<Uuid>, Value)> {
    TaskRepository::new(&app.pool)
        .list_task_events_after(pid, 0, 100)
        .await
        .expect("the events read")
        .into_iter()
        .map(|row| (row.kind, row.task_id, row.payload))
        .collect()
}

/// The `states_changed` events of the project's stream.
async fn state_events(app: &TestApp, pid: Uuid) -> Vec<(Option<Uuid>, Value)> {
    events(app, pid)
        .await
        .into_iter()
        .filter(|(kind, _, _)| kind == "states_changed")
        .map(|(_, task_id, payload)| (task_id, payload))
        .collect()
}

/// The state names an event payload carries, in order.
fn payload_names(payload: &Value) -> Vec<String> {
    payload["states"]
        .as_array()
        .expect("a state list")
        .iter()
        .map(|state| state["name"].as_str().expect("a name").to_string())
        .collect()
}

/// Create a task in `state` and answer with it.
async fn task_in(app: &TestApp, user: &AuthenticatedUser, pid: Uuid, state: &str) -> Value {
    let response = app
        .post_as(user, &format!("/api/projects/{pid}/tasks"))
        .json(&json!({ "title": "Ship the routes", "state": state }))
        .await;
    response.assert_status(StatusCode::CREATED);
    response.json::<Value>()
}

// ---- list ----

#[tokio::test]
async fn the_list_is_the_seven_default_states_in_board_order() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user).await;

    let states = list(&app, &user, pid).await;

    assert_eq!(states.len(), 7);
    for (index, (state, name)) in states.iter().zip(DEFAULTS).enumerate() {
        assert_eq!(state["name"], json!(name));
        assert_eq!(state["position"], json!(index as i64));
        assert_eq!(state["project_id"], json!(pid));
        assert!(state["id"].is_string());
        assert!(state["created_at"].is_string());
    }
    assert_eq!(states[1]["kind"], json!("queue"));
    assert_eq!(states[4]["kind"], json!("human"));
    assert_eq!(states[5]["kind"], json!("terminal"));

    // A read takes no part in any mutation and announces nothing.
    assert!(state_events(&app, pid).await.is_empty());
}

#[tokio::test]
async fn the_list_of_an_unknown_project_is_404() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;

    let response = app.get_as(&user, &states_path(Uuid::new_v4())).await;

    assert_error(&response, StatusCode::NOT_FOUND, "not found");
}

#[tokio::test]
async fn every_route_requires_a_token() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user).await;

    let listed = app.server.get(&states_path(pid)).await;
    assert_error(&listed, StatusCode::UNAUTHORIZED, "authentication required");

    let created = app
        .server
        .post(&states_path(pid))
        .json(&json!({ "name": "qa", "kind": "queue" }))
        .await;
    assert_error(
        &created,
        StatusCode::UNAUTHORIZED,
        "authentication required",
    );

    let updated = app
        .server
        .put(&state_path(pid, "review"))
        .json(&json!({ "name": "qa" }))
        .await;
    assert_error(
        &updated,
        StatusCode::UNAUTHORIZED,
        "authentication required",
    );

    let removed = app.server.delete(&state_path(pid, "done")).await;
    assert_error(
        &removed,
        StatusCode::UNAUTHORIZED,
        "authentication required",
    );

    assert_eq!(names(&app, &user, pid).await, DEFAULTS.to_vec());
    assert!(state_events(&app, pid).await.is_empty());
}

// ---- create ----

#[tokio::test]
async fn a_state_without_a_position_appends_and_announces_the_new_list() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user).await;

    let response = app
        .post_as(&user, &states_path(pid))
        .json(&json!({ "name": "qa", "kind": "queue" }))
        .await;

    response.assert_status(StatusCode::CREATED);
    let created = response.json::<Value>();
    assert_eq!(created["name"], json!("qa"));
    assert_eq!(created["kind"], json!("queue"));
    assert_eq!(created["position"], json!(7));
    assert_eq!(created["project_id"], json!(pid));

    assert_eq!(
        names(&app, &user, pid).await,
        vec![
            "backlog",
            "ready",
            "review",
            "merge",
            "needs_human",
            "done",
            "cancelled",
            "qa",
        ]
    );

    let announced = state_events(&app, pid).await;
    assert_eq!(announced.len(), 1);
    let (task_id, payload) = &announced[0];
    assert_eq!(*task_id, None);
    assert_eq!(
        payload["actor"],
        json!({ "kind": "user", "user_id": user.user.id })
    );
    assert_eq!(
        payload_names(payload),
        vec![
            "backlog",
            "ready",
            "review",
            "merge",
            "needs_human",
            "done",
            "cancelled",
            "qa",
        ]
    );
}

#[tokio::test]
async fn an_explicit_position_shifts_the_states_at_and_after_it() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user).await;

    let response = app
        .post_as(&user, &states_path(pid))
        .json(&json!({ "name": "triage", "kind": "queue", "position": 1 }))
        .await;

    response.assert_status(StatusCode::CREATED);
    assert_eq!(response.json::<Value>()["position"], json!(1));

    assert_eq!(
        positions(&app, &user, pid).await,
        vec![
            ("backlog".to_string(), 0),
            ("triage".to_string(), 1),
            ("ready".to_string(), 2),
            ("review".to_string(), 3),
            ("merge".to_string(), 4),
            ("needs_human".to_string(), 5),
            ("done".to_string(), 6),
            ("cancelled".to_string(), 7),
        ]
    );
}

#[tokio::test]
async fn a_position_beyond_the_end_appends() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user).await;

    let response = app
        .post_as(&user, &states_path(pid))
        .json(&json!({ "name": "qa", "kind": "queue", "position": 99 }))
        .await;

    response.assert_status(StatusCode::CREATED);
    assert_eq!(response.json::<Value>()["position"], json!(7));
}

#[tokio::test]
async fn an_invalid_name_is_400_and_changes_nothing() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user).await;

    let message = "state name must be 1-32 characters of lowercase letters, digits, \
                   '_' or '-', starting with a letter or digit";

    for name in [
        "Ready".to_string(),
        "s".repeat(33),
        String::new(),
        "needs human".to_string(),
    ] {
        let response = app
            .post_as(&user, &states_path(pid))
            .json(&json!({ "name": name, "kind": "queue" }))
            .await;

        assert_error(&response, StatusCode::BAD_REQUEST, message);
    }

    assert_eq!(names(&app, &user, pid).await, DEFAULTS.to_vec());
    assert!(state_events(&app, pid).await.is_empty());
}

#[tokio::test]
async fn an_unknown_kind_is_400() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user).await;

    for kind in [json!("Queue"), json!("waiting"), json!(3), json!(null)] {
        let response = app
            .post_as(&user, &states_path(pid))
            .json(&json!({ "name": "qa", "kind": kind }))
            .await;

        assert_error(&response, StatusCode::BAD_REQUEST, "invalid state kind");
    }

    assert_eq!(names(&app, &user, pid).await, DEFAULTS.to_vec());
    assert!(state_events(&app, pid).await.is_empty());
}

#[tokio::test]
async fn a_negative_position_is_400() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user).await;

    let response = app
        .post_as(&user, &states_path(pid))
        .json(&json!({ "name": "qa", "kind": "queue", "position": -1 }))
        .await;

    assert_error(
        &response,
        StatusCode::BAD_REQUEST,
        "position must not be negative",
    );
    assert!(state_events(&app, pid).await.is_empty());
}

#[tokio::test]
async fn a_name_that_is_taken_is_409() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user).await;

    let response = app
        .post_as(&user, &states_path(pid))
        .json(&json!({ "name": "review", "kind": "queue" }))
        .await;

    assert_error(&response, StatusCode::CONFLICT, "state name already taken");
    assert_eq!(names(&app, &user, pid).await, DEFAULTS.to_vec());
    assert!(state_events(&app, pid).await.is_empty());
}

#[tokio::test]
async fn a_second_human_state_is_409() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user).await;

    let response = app
        .post_as(&user, &states_path(pid))
        .json(&json!({ "name": "escalated", "kind": "human" }))
        .await;

    assert_error(
        &response,
        StatusCode::CONFLICT,
        "project already has a human state",
    );
    assert_eq!(names(&app, &user, pid).await, DEFAULTS.to_vec());
    assert!(state_events(&app, pid).await.is_empty());
}

#[tokio::test]
async fn a_create_on_an_unknown_project_is_404() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;

    let response = app
        .post_as(&user, &states_path(Uuid::new_v4()))
        .json(&json!({ "name": "qa", "kind": "queue" }))
        .await;

    assert_error(&response, StatusCode::NOT_FOUND, "not found");
}

// ---- update ----

#[tokio::test]
async fn a_rename_renames_the_state_everywhere_and_announces_once() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user).await;
    let task = task_in(&app, &user, pid, "review").await;

    let response = app
        .put_as(&user, &state_path(pid, "review"))
        .json(&json!({ "name": "qa" }))
        .await;

    response.assert_status(StatusCode::OK);
    let renamed = response.json::<Value>();
    assert_eq!(renamed["name"], json!("qa"));
    assert_eq!(renamed["kind"], json!("queue"));
    assert_eq!(renamed["position"], json!(2));

    assert_eq!(
        names(&app, &user, pid).await,
        vec![
            "backlog",
            "ready",
            "qa",
            "merge",
            "needs_human",
            "done",
            "cancelled",
        ]
    );

    // The link is by id, so the task moved with the column.
    let id = task["id"].as_str().expect("a task id");
    let detail = app
        .get_as(&user, &format!("/api/projects/{pid}/tasks/{id}"))
        .await;
    detail.assert_status(StatusCode::OK);
    assert_eq!(detail.json::<Value>()["state"], json!("qa"));

    let announced = state_events(&app, pid).await;
    assert_eq!(announced.len(), 1);
    assert_eq!(announced[0].0, None);
    assert!(payload_names(&announced[0].1).contains(&"qa".to_string()));
}

#[tokio::test]
async fn a_reorder_repacks_every_position() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user).await;

    let response = app
        .put_as(&user, &state_path(pid, "cancelled"))
        .json(&json!({ "position": 0 }))
        .await;

    response.assert_status(StatusCode::OK);
    assert_eq!(response.json::<Value>()["position"], json!(0));

    assert_eq!(
        positions(&app, &user, pid).await,
        vec![
            ("cancelled".to_string(), 0),
            ("backlog".to_string(), 1),
            ("ready".to_string(), 2),
            ("review".to_string(), 3),
            ("merge".to_string(), 4),
            ("needs_human".to_string(), 5),
            ("done".to_string(), 6),
        ]
    );

    let announced = state_events(&app, pid).await;
    assert_eq!(announced.len(), 1);
    assert_eq!(payload_names(&announced[0].1)[0], "cancelled");
}

#[tokio::test]
async fn a_rename_and_a_move_in_one_request_announce_once() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user).await;

    let response = app
        .put_as(&user, &state_path(pid, "review"))
        .json(&json!({ "name": "qa", "position": 0 }))
        .await;

    response.assert_status(StatusCode::OK);
    let updated = response.json::<Value>();
    assert_eq!(updated["name"], json!("qa"));
    assert_eq!(updated["position"], json!(0));

    assert_eq!(
        names(&app, &user, pid).await,
        vec![
            "qa",
            "backlog",
            "ready",
            "merge",
            "needs_human",
            "done",
            "cancelled",
        ]
    );
    assert_eq!(state_events(&app, pid).await.len(), 1);
}

#[tokio::test]
async fn an_update_that_changes_nothing_is_200_without_an_event() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user).await;

    for body in [
        json!({}),
        json!({ "name": "review" }),
        json!({ "position": 2 }),
        json!({ "name": "review", "position": 2 }),
    ] {
        let response = app
            .put_as(&user, &state_path(pid, "review"))
            .json(&body)
            .await;

        response.assert_status(StatusCode::OK);
        let unchanged = response.json::<Value>();
        assert_eq!(unchanged["name"], json!("review"));
        assert_eq!(unchanged["position"], json!(2));
    }

    assert_eq!(names(&app, &user, pid).await, DEFAULTS.to_vec());
    assert!(state_events(&app, pid).await.is_empty());
}

#[tokio::test]
async fn moving_the_last_state_past_the_end_is_a_no_op() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user).await;

    let response = app
        .put_as(&user, &state_path(pid, "cancelled"))
        .json(&json!({ "position": 99 }))
        .await;

    response.assert_status(StatusCode::OK);
    assert_eq!(response.json::<Value>()["position"], json!(6));
    assert!(state_events(&app, pid).await.is_empty());
}

#[tokio::test]
async fn a_body_carrying_kind_is_400_whatever_the_value() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user).await;

    for body in [
        json!({ "kind": "queue" }),
        json!({ "kind": "terminal" }),
        json!({ "name": "qa", "kind": null }),
    ] {
        let response = app
            .put_as(&user, &state_path(pid, "review"))
            .json(&body)
            .await;

        assert_error(&response, StatusCode::BAD_REQUEST, "kind is immutable");
    }

    assert_eq!(names(&app, &user, pid).await, DEFAULTS.to_vec());
    assert!(state_events(&app, pid).await.is_empty());
}

#[tokio::test]
async fn a_rename_to_a_taken_name_is_409() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user).await;

    let response = app
        .put_as(&user, &state_path(pid, "review"))
        .json(&json!({ "name": "merge" }))
        .await;

    assert_error(&response, StatusCode::CONFLICT, "state name already taken");
    assert_eq!(names(&app, &user, pid).await, DEFAULTS.to_vec());
    assert!(state_events(&app, pid).await.is_empty());
}

#[tokio::test]
async fn a_rename_to_an_invalid_name_is_400() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user).await;

    let response = app
        .put_as(&user, &state_path(pid, "review"))
        .json(&json!({ "name": "QA" }))
        .await;

    response.assert_status(StatusCode::BAD_REQUEST);
    assert_eq!(names(&app, &user, pid).await, DEFAULTS.to_vec());
    assert!(state_events(&app, pid).await.is_empty());
}

#[tokio::test]
async fn the_path_is_a_name_so_a_uuid_is_404() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user).await;
    let id = list(&app, &user, pid).await[2]["id"]
        .as_str()
        .expect("a state id")
        .to_string();

    let response = app
        .put_as(&user, &state_path(pid, &id))
        .json(&json!({ "name": "qa" }))
        .await;

    assert_error(&response, StatusCode::NOT_FOUND, "not found");

    let removed = app.delete_as(&user, &state_path(pid, &id)).await;
    assert_error(&removed, StatusCode::NOT_FOUND, "not found");
}

// ---- delete ----

#[tokio::test]
async fn deleting_a_spare_terminal_state_is_204_and_announces_the_rest() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user).await;

    let response = app.delete_as(&user, &state_path(pid, "done")).await;

    response.assert_status(StatusCode::NO_CONTENT);
    assert_eq!(
        positions(&app, &user, pid).await,
        vec![
            ("backlog".to_string(), 0),
            ("ready".to_string(), 1),
            ("review".to_string(), 2),
            ("merge".to_string(), 3),
            ("needs_human".to_string(), 4),
            ("cancelled".to_string(), 5),
        ]
    );

    let announced = state_events(&app, pid).await;
    assert_eq!(announced.len(), 1);
    assert_eq!(announced[0].0, None);
    assert_eq!(
        payload_names(&announced[0].1),
        vec![
            "backlog",
            "ready",
            "review",
            "merge",
            "needs_human",
            "cancelled"
        ]
    );
}

#[tokio::test]
async fn the_human_state_cannot_be_deleted() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user).await;

    let response = app.delete_as(&user, &state_path(pid, "needs_human")).await;

    assert_error(
        &response,
        StatusCode::CONFLICT,
        "cannot delete the human state",
    );
    assert_eq!(names(&app, &user, pid).await, DEFAULTS.to_vec());
    assert!(state_events(&app, pid).await.is_empty());
}

#[tokio::test]
async fn the_last_terminal_state_cannot_be_deleted() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user).await;

    app.delete_as(&user, &state_path(pid, "done"))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    let response = app.delete_as(&user, &state_path(pid, "cancelled")).await;

    assert_error(
        &response,
        StatusCode::CONFLICT,
        "cannot delete the last terminal state",
    );
    assert!(
        names(&app, &user, pid)
            .await
            .contains(&"cancelled".to_string())
    );
    // The successful delete above, and nothing for the refusal.
    assert_eq!(state_events(&app, pid).await.len(), 1);
}

#[tokio::test]
async fn the_last_queue_state_cannot_be_deleted() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user).await;

    for name in ["ready", "review", "merge"] {
        app.delete_as(&user, &state_path(pid, name))
            .await
            .assert_status(StatusCode::NO_CONTENT);
    }

    let response = app.delete_as(&user, &state_path(pid, "backlog")).await;

    assert_error(
        &response,
        StatusCode::CONFLICT,
        "cannot delete the last queue state",
    );
    assert_eq!(
        names(&app, &user, pid).await,
        vec!["backlog", "needs_human", "done", "cancelled"]
    );
    assert_eq!(state_events(&app, pid).await.len(), 3);
}

#[tokio::test]
async fn a_state_holding_a_task_cannot_be_deleted() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user).await;
    task_in(&app, &user, pid, "review").await;

    let response = app.delete_as(&user, &state_path(pid, "review")).await;

    assert_error(&response, StatusCode::CONFLICT, "state is in use by tasks");
    assert_eq!(names(&app, &user, pid).await, DEFAULTS.to_vec());
    assert!(state_events(&app, pid).await.is_empty());
}

#[tokio::test]
async fn deleting_an_unknown_state_is_404() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user).await;

    let response = app.delete_as(&user, &state_path(pid, "nowhere")).await;

    assert_error(&response, StatusCode::NOT_FOUND, "not found");
    assert!(state_events(&app, pid).await.is_empty());
}

#[tokio::test]
async fn a_delete_on_an_unknown_project_is_404() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;

    let response = app
        .delete_as(&user, &state_path(Uuid::new_v4(), "done"))
        .await;

    assert_error(&response, StatusCode::NOT_FOUND, "not found");
}

#[tokio::test]
async fn any_authenticated_user_may_edit_the_board() {
    let app = TestApp::spawn().await;
    let owner = signed_in(&app, "ada").await;
    let other = signed_in(&app, "bob").await;
    let pid = project(&app, &owner).await;

    let response = app
        .post_as(&other, &states_path(pid))
        .json(&json!({ "name": "qa", "kind": "queue" }))
        .await;

    response.assert_status(StatusCode::CREATED);

    let announced = state_events(&app, pid).await;
    assert_eq!(
        announced[0].1["actor"],
        json!({ "kind": "user", "user_id": other.user.id })
    );
}
