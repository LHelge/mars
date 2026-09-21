//! `/api/projects/{pid}/tasks`, `/api/projects/{pid}/tasks/{id}` and the
//! dashboard's `/api/tasks` through the real router (`SPEC.md`, "Tasks";
//! `SPEC.md`, "Frontend" → "Dashboard").
//!
//! The read side of the tracker and the creation that fills it. What the
//! *rules* are is asserted against the models and against a real database in
//! `tests/tracker_tasks.rs` and `tests/tracker_states.rs`;
//! what is asserted here is everything the endpoints add on top:
//!
//! - the documented status and body of each success and each refusal, and the
//!   JWT requirement on all four paths;
//! - that a creation writes the events it owes, in the order `SPEC.md` gives
//!   them: `created`, then one `dependency_added` per edge, then any `blocked`
//!   flip;
//! - that a refused creation leaves no task and no events behind, because the
//!   whole thing is one mutation (ADR 0021);
//! - that `{id}` and `?parent=` accept a UUID or a per-project number alike.
//!
//! **A creation cannot close a dependency cycle**, so there is no 409 scenario
//! here: the new task has no incoming `blocks` edges at the moment its own are
//! inserted, so nothing can reach back to it. The check runs all the same —
//! it is the same `check_no_cycle` the dependency endpoint uses — and what is
//! asserted instead is the rollback a *different* failing dependency causes.
//!
//! Projects are created through `projects::create_project` rather than through
//! `POST /api/projects`: what these scenarios need from a project is its seven
//! default states and its task-number counter, not a clone, and no task
//! endpoint touches git or the engine.
//!
//! Every credential-shaped value is an obviously fake stand-in (rule 3).
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use std::time::Duration;

use axum::http::StatusCode;
use axum_test::TestResponse;
use chrono::Utc;
use common::{AuthenticatedUser, TestApp};
use mars_orchestrator::events::TaskActor;
use mars_orchestrator::models::{
    HandoffCaller, NewSession, NewTaskComment, NewTaskHandoff, ProfileKind, ReviewDecision,
    ReviewStatus, TaskRef,
};
use mars_orchestrator::prelude::*;
use mars_orchestrator::projects::{NewProjectRequest, create_project};
use mars_orchestrator::repositories::{ProjectRepository, SessionRepository, TaskRepository};
use mars_orchestrator::tracker::{ReviewCarry, TrackerMutation, claim_for_launch};
use serde_json::{Value, json};
use tokio::time::sleep;
use uuid::Uuid;

/// Not a real remote: `.invalid` can never resolve (rule 3).
const TEST_REMOTE: &str = "https://example.invalid/org/repo.git";

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

/// Assert the documented error body (`SPEC.md`, "REST API").
fn assert_error(response: &TestResponse, status: StatusCode, message: &str) {
    response.assert_status(status);
    response.assert_json(&json!({ "status": status.as_u16(), "error": message }));
}

/// A project with the seeded default states, created by `user`.
async fn project(app: &TestApp, user: &AuthenticatedUser, name: &str) -> Uuid {
    create_project(
        &app.state,
        NewProjectRequest {
            name: name.to_string(),
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

/// `/api/projects/{pid}/tasks`.
fn tasks_path(pid: Uuid) -> String {
    format!("/api/projects/{pid}/tasks")
}

/// `/api/projects/{pid}/tasks/{id}`, with `id` in either accepted form.
fn task_path(pid: Uuid, id: &str) -> String {
    format!("/api/projects/{pid}/tasks/{id}")
}

/// `POST` a task and expect it to be created.
async fn create_task(app: &TestApp, user: &AuthenticatedUser, pid: Uuid, body: Value) -> Value {
    let response = app.post_as(user, &tasks_path(pid)).json(&body).await;
    response.assert_status(StatusCode::CREATED);
    response.json::<Value>()
}

/// The project's event stream from the beginning, as `(kind, task_id)` pairs
/// with their payloads.
async fn events(app: &TestApp, pid: Uuid) -> Vec<(String, Value)> {
    TaskRepository::new(&app.pool)
        .list_task_events_after(pid, 0, 100)
        .await
        .expect("the events read")
        .into_iter()
        .map(|row| (row.kind, row.payload))
        .collect()
}

/// How many tasks the project has, straight from the table.
async fn task_count(pool: &PgPool, pid: Uuid) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM tasks WHERE project_id = $1")
        .bind(pid)
        .fetch_one(pool)
        .await
        .expect("the count runs")
}

/// The `number` of each task in a list response, in the order it came back.
fn numbers(list: &Value) -> Vec<i64> {
    list.as_array()
        .expect("a list")
        .iter()
        .map(|task| task["number"].as_i64().expect("a number"))
        .collect()
}

// ---- create ----

#[tokio::test]
async fn a_created_task_takes_the_default_state_priority_and_number() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "creator").await;
    let pid = project(&app, &user, "mars").await;

    let task = create_task(&app, &user, pid, json!({ "title": "Wire the tracker" })).await;

    assert_eq!(task["number"], json!(1));
    assert_eq!(task["title"], json!("Wire the tracker"));
    assert_eq!(task["description"], json!(""));
    assert_eq!(task["state"], json!("backlog"));
    assert_eq!(task["priority"], json!(2));
    assert_eq!(task["blocked"], json!(false));
    assert_eq!(task["labels"], json!([]));
    assert_eq!(task["parent_id"], json!(null));
    assert_eq!(task["assignee_user_id"], json!(null));
    assert_eq!(task["depends_on"], json!([]));
    assert_eq!(task["closed_at"], json!(null));

    let stream = events(&app, pid).await;
    assert_eq!(stream.len(), 1, "creation emits exactly one event");
    assert_eq!(stream[0].0, "created");
    assert_eq!(
        stream[0].1["actor"],
        json!({ "kind": "user", "user_id": user.user.id })
    );
    assert_eq!(stream[0].1["task"]["id"], task["id"]);

    // The counter advances and is never reused.
    let second = create_task(&app, &user, pid, json!({ "title": "And the board" })).await;
    assert_eq!(second["number"], json!(2));
}

#[tokio::test]
async fn a_creation_takes_the_state_description_priority_and_labels_it_is_given() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "creator").await;
    let pid = project(&app, &user, "mars").await;

    let task = create_task(
        &app,
        &user,
        pid,
        json!({
            "title": "Wire the tracker",
            "description": "The read side first.",
            "state": "ready",
            "priority": 0,
            "labels": ["backend", "backend", "tracker"],
        }),
    )
    .await;

    assert_eq!(task["state"], json!("ready"));
    assert_eq!(task["description"], json!("The read side first."));
    assert_eq!(task["priority"], json!(0));
    // Deduplicated in first-occurrence order by the model.
    assert_eq!(task["labels"], json!(["backend", "tracker"]));
}

#[tokio::test]
async fn a_task_created_in_a_terminal_state_is_closed_on_the_spot() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "creator").await;
    let pid = project(&app, &user, "mars").await;

    let task = create_task(
        &app,
        &user,
        pid,
        json!({ "title": "Already done", "state": "done" }),
    )
    .await;

    assert_eq!(task["state"], json!("done"));
    assert!(
        task["closed_at"].is_string(),
        "a terminal state closes the task"
    );
}

#[tokio::test]
async fn a_task_created_in_the_human_state_emits_created_and_nothing_else() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "creator").await;
    let pid = project(&app, &user, "mars").await;

    let task = create_task(
        &app,
        &user,
        pid,
        json!({ "title": "Ask a person", "state": "needs_human" }),
    )
    .await;

    assert_eq!(task["state"], json!("needs_human"));
    assert_eq!(task["needs_human_reason"], json!(null));

    let kinds: Vec<String> = events(&app, pid).await.into_iter().map(|e| e.0).collect();
    assert_eq!(kinds, vec!["created"], "creation is `created` only");
}

#[tokio::test]
async fn an_unknown_state_is_refused_with_the_projects_valid_names() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "creator").await;
    let pid = project(&app, &user, "mars").await;

    let response = app
        .post_as(&user, &tasks_path(pid))
        .json(&json!({ "title": "Wire the tracker", "state": "in-progress" }))
        .await;

    assert_error(
        &response,
        StatusCode::BAD_REQUEST,
        "unknown state \"in-progress\"; valid states are: backlog, ready, review, merge, needs_human, done, cancelled",
    );
    assert_eq!(task_count(&app.pool, pid).await, 0);
}

#[tokio::test]
async fn a_priority_outside_the_documented_range_is_refused() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "creator").await;
    let pid = project(&app, &user, "mars").await;

    let response = app
        .post_as(&user, &tasks_path(pid))
        .json(&json!({ "title": "Wire the tracker", "priority": 4 }))
        .await;

    assert_error(
        &response,
        StatusCode::BAD_REQUEST,
        "priority must be between 0 (critical) and 3 (low)",
    );
    assert_eq!(task_count(&app.pool, pid).await, 0);
}

#[tokio::test]
async fn a_label_that_is_not_one_is_refused_with_the_models_message() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "creator").await;
    let pid = project(&app, &user, "mars").await;

    let response = app
        .post_as(&user, &tasks_path(pid))
        .json(&json!({ "title": "Wire the tracker", "labels": ["Back End"] }))
        .await;

    assert_error(
        &response,
        StatusCode::BAD_REQUEST,
        "each label must be 1-32 characters of lowercase letters, digits, '_' or '-', starting with a letter or digit",
    );
}

#[tokio::test]
async fn an_empty_title_is_refused() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "creator").await;
    let pid = project(&app, &user, "mars").await;

    let response = app
        .post_as(&user, &tasks_path(pid))
        .json(&json!({ "title": "   " }))
        .await;

    assert_error(
        &response,
        StatusCode::BAD_REQUEST,
        "title must be 1-200 characters",
    );
}

#[tokio::test]
async fn an_assignee_in_the_body_is_refused_because_it_is_a_put_field() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "creator").await;
    let pid = project(&app, &user, "mars").await;

    let response = app
        .post_as(&user, &tasks_path(pid))
        .json(&json!({ "title": "Wire the tracker", "assignee_user_id": user.user.id }))
        .await;

    response.assert_status(StatusCode::BAD_REQUEST);
    assert_eq!(task_count(&app.pool, pid).await, 0);
}

#[tokio::test]
async fn a_dependency_on_an_open_task_blocks_the_new_one_and_emits_three_events() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "creator").await;
    let pid = project(&app, &user, "mars").await;

    let prerequisite = create_task(&app, &user, pid, json!({ "title": "First" })).await;
    let task = create_task(
        &app,
        &user,
        pid,
        json!({ "title": "Second", "depends_on": [prerequisite["id"]] }),
    )
    .await;

    assert_eq!(task["blocked"], json!(true));
    assert_eq!(
        task["depends_on"],
        json!([{ "task_id": prerequisite["id"], "kind": "blocks" }])
    );

    let stream = events(&app, pid).await;
    let kinds: Vec<&str> = stream.iter().map(|event| event.0.as_str()).collect();
    assert_eq!(
        kinds,
        vec!["created", "created", "dependency_added", "blocked"]
    );

    // `created` and `dependency_added` both describe the task once its edges
    // exist; only the `blocked` event shows the flag turned on.
    assert_eq!(stream[1].1["task"]["depends_on"], task["depends_on"]);
    assert_eq!(stream[1].1["task"]["blocked"], json!(false));
    assert_eq!(stream[2].1["task"]["depends_on"], task["depends_on"]);
    assert_eq!(stream[3].1["task"]["blocked"], json!(true));
}

#[tokio::test]
async fn a_dependency_on_a_closed_task_leaves_the_new_one_unblocked() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "creator").await;
    let pid = project(&app, &user, "mars").await;

    let prerequisite = create_task(
        &app,
        &user,
        pid,
        json!({ "title": "First", "state": "done" }),
    )
    .await;
    let task = create_task(
        &app,
        &user,
        pid,
        json!({ "title": "Second", "depends_on": [prerequisite["number"].to_string()] }),
    )
    .await;

    assert_eq!(task["blocked"], json!(false));
    assert_eq!(
        task["depends_on"],
        json!([{ "task_id": prerequisite["id"], "kind": "blocks" }])
    );
}

#[tokio::test]
async fn the_same_prerequisite_named_twice_becomes_one_edge() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "creator").await;
    let pid = project(&app, &user, "mars").await;

    let prerequisite = create_task(&app, &user, pid, json!({ "title": "First" })).await;
    let by_number = prerequisite["number"].to_string();
    let task = create_task(
        &app,
        &user,
        pid,
        json!({ "title": "Second", "depends_on": [prerequisite["id"], by_number] }),
    )
    .await;

    assert_eq!(
        task["depends_on"],
        json!([{ "task_id": prerequisite["id"], "kind": "blocks" }]),
        "a duplicate reference is inserted once rather than refused",
    );

    let kinds: Vec<String> = events(&app, pid).await.into_iter().map(|e| e.0).collect();
    assert_eq!(
        kinds,
        vec!["created", "created", "dependency_added", "blocked"],
        "and emits one `dependency_added`",
    );
}

#[tokio::test]
async fn a_dependency_on_a_task_of_another_project_rolls_the_whole_creation_back() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "creator").await;
    let pid = project(&app, &user, "mars").await;
    let other = project(&app, &user, "phobos").await;

    let elsewhere = create_task(&app, &user, other, json!({ "title": "Elsewhere" })).await;

    let response = app
        .post_as(&user, &tasks_path(pid))
        .json(&json!({ "title": "Second", "depends_on": [elsewhere["id"]] }))
        .await;

    assert_error(
        &response,
        StatusCode::BAD_REQUEST,
        "dependency must reference tasks of the same project",
    );
    assert_eq!(task_count(&app.pool, pid).await, 0, "no task row");
    assert!(events(&app, pid).await.is_empty(), "and no events");
}

#[tokio::test]
async fn a_dependency_reference_that_addresses_nothing_is_refused() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "creator").await;
    let pid = project(&app, &user, "mars").await;

    let response = app
        .post_as(&user, &tasks_path(pid))
        .json(&json!({ "title": "Second", "depends_on": ["4242"] }))
        .await;

    assert_error(
        &response,
        StatusCode::BAD_REQUEST,
        "dependency must reference tasks of the same project",
    );

    let malformed = app
        .post_as(&user, &tasks_path(pid))
        .json(&json!({ "title": "Second", "depends_on": ["not-a-task"] }))
        .await;

    assert_error(
        &malformed,
        StatusCode::BAD_REQUEST,
        "a task is addressed by its UUID or its per-project number",
    );
}

#[tokio::test]
async fn a_child_of_an_open_child_is_refused_by_the_one_level_rule() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "creator").await;
    let pid = project(&app, &user, "mars").await;

    let epic = create_task(&app, &user, pid, json!({ "title": "Epic" })).await;
    let child = create_task(
        &app,
        &user,
        pid,
        json!({ "title": "Child", "parent_id": epic["id"] }),
    )
    .await;

    let response = app
        .post_as(&user, &tasks_path(pid))
        .json(&json!({ "title": "Grandchild", "parent_id": child["id"] }))
        .await;

    assert_error(
        &response,
        StatusCode::BAD_REQUEST,
        "parent must be a top-level task of the same project",
    );
}

#[tokio::test]
async fn a_parent_from_another_project_is_refused() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "creator").await;
    let pid = project(&app, &user, "mars").await;
    let other = project(&app, &user, "phobos").await;

    let elsewhere = create_task(&app, &user, other, json!({ "title": "Elsewhere" })).await;

    let response = app
        .post_as(&user, &tasks_path(pid))
        .json(&json!({ "title": "Child", "parent_id": elsewhere["id"] }))
        .await;

    assert_error(
        &response,
        StatusCode::BAD_REQUEST,
        "parent must be a top-level task of the same project",
    );
    assert_eq!(task_count(&app.pool, pid).await, 0);
}

#[tokio::test]
async fn an_open_child_blocks_its_parent_when_it_is_created() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "creator").await;
    let pid = project(&app, &user, "mars").await;

    let epic = create_task(&app, &user, pid, json!({ "title": "Epic" })).await;
    create_task(
        &app,
        &user,
        pid,
        json!({ "title": "Child", "parent_id": epic["id"] }),
    )
    .await;

    let response = app
        .get_as(&user, &task_path(pid, epic["id"].as_str().expect("a uuid")))
        .await;
    response.assert_status_ok();
    assert_eq!(response.json::<Value>()["blocked"], json!(true));

    let kinds: Vec<String> = events(&app, pid).await.into_iter().map(|e| e.0).collect();
    assert_eq!(kinds, vec!["created", "created", "blocked"]);
}

#[tokio::test]
async fn creating_a_task_needs_a_token_and_a_project_that_exists() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "creator").await;
    let pid = project(&app, &user, "mars").await;

    let anonymous = app
        .server
        .post(&tasks_path(pid))
        .json(&json!({ "title": "Wire the tracker" }))
        .await;
    anonymous.assert_status(StatusCode::UNAUTHORIZED);
    anonymous.assert_json(&unauthorized());

    let unknown = app
        .post_as(&user, &tasks_path(Uuid::new_v4()))
        .json(&json!({ "title": "Wire the tracker" }))
        .await;
    assert_error(&unknown, StatusCode::NOT_FOUND, "not found");
}

// ---- project list ----

#[tokio::test]
async fn the_project_list_is_ordered_by_priority_then_number() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "creator").await;
    let pid = project(&app, &user, "mars").await;

    create_task(&app, &user, pid, json!({ "title": "One" })).await;
    create_task(&app, &user, pid, json!({ "title": "Two", "priority": 0 })).await;
    create_task(&app, &user, pid, json!({ "title": "Three" })).await;
    create_task(&app, &user, pid, json!({ "title": "Four", "priority": 0 })).await;

    let response = app.get_as(&user, &tasks_path(pid)).await;
    response.assert_status_ok();

    assert_eq!(numbers(&response.json::<Value>()), vec![2, 4, 1, 3]);
}

#[tokio::test]
async fn the_project_list_filters_by_state_label_priority_parent_and_held() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "creator").await;
    let pid = project(&app, &user, "mars").await;

    let epic = create_task(&app, &user, pid, json!({ "title": "Epic" })).await;
    create_task(
        &app,
        &user,
        pid,
        json!({ "title": "Ready and labelled", "state": "ready", "labels": ["backend"] }),
    )
    .await;
    create_task(
        &app,
        &user,
        pid,
        json!({ "title": "Child", "parent_id": epic["id"], "priority": 1 }),
    )
    .await;
    create_task(
        &app,
        &user,
        pid,
        json!({ "title": "Closed", "state": "done" }),
    )
    .await;

    let cases = [
        ("?state=ready", vec![2]),
        // A closed task is listed like any other: the board shows every column.
        ("?state=done", vec![4]),
        ("?label=backend", vec![2]),
        ("?label=nothing-carries-this", vec![]),
        ("?priority=1", vec![3]),
        // The parent by its per-project number, not only by UUID.
        ("?parent=1", vec![3]),
        ("?held=false", vec![3, 1, 2, 4]),
        ("?held=true", vec![]),
    ];

    for (query, expected) in cases {
        let response = app
            .get_as(&user, &format!("{}{query}", tasks_path(pid)))
            .await;
        response.assert_status_ok();
        assert_eq!(numbers(&response.json::<Value>()), expected, "{query}");
    }
}

#[tokio::test]
async fn the_project_list_refuses_a_filter_it_cannot_resolve() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "creator").await;
    let pid = project(&app, &user, "mars").await;

    let unknown_state = app
        .get_as(&user, &format!("{}?state=nowhere", tasks_path(pid)))
        .await;
    assert_error(
        &unknown_state,
        StatusCode::BAD_REQUEST,
        "unknown state \"nowhere\"; valid states are: backlog, ready, review, merge, needs_human, done, cancelled",
    );

    let bad_priority = app
        .get_as(&user, &format!("{}?priority=9", tasks_path(pid)))
        .await;
    assert_error(
        &bad_priority,
        StatusCode::BAD_REQUEST,
        "priority must be between 0 (critical) and 3 (low)",
    );

    let unknown_parent = app
        .get_as(&user, &format!("{}?parent=404", tasks_path(pid)))
        .await;
    assert_error(&unknown_parent, StatusCode::NOT_FOUND, "not found");
}

#[tokio::test]
async fn the_project_list_needs_a_token_and_a_project_that_exists() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "creator").await;
    let pid = project(&app, &user, "mars").await;

    let anonymous = app.server.get(&tasks_path(pid)).await;
    anonymous.assert_status(StatusCode::UNAUTHORIZED);
    anonymous.assert_json(&unauthorized());

    let unknown = app.get_as(&user, &tasks_path(Uuid::new_v4())).await;
    assert_error(&unknown, StatusCode::NOT_FOUND, "not found");
}

// ---- detail ----

#[tokio::test]
async fn the_detail_answers_to_a_number_and_to_a_uuid_alike() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "creator").await;
    let pid = project(&app, &user, "mars").await;

    let epic = create_task(&app, &user, pid, json!({ "title": "Epic" })).await;
    create_task(
        &app,
        &user,
        pid,
        json!({ "title": "Child", "parent_id": epic["id"] }),
    )
    .await;

    for reference in ["1", epic["id"].as_str().expect("a uuid")] {
        let response = app.get_as(&user, &task_path(pid, reference)).await;
        response.assert_status_ok();

        let detail = response.json::<Value>();
        assert_eq!(detail["id"], epic["id"], "{reference}");
        assert_eq!(detail["number"], json!(1));
        assert_eq!(detail["comments"], json!([]));
        assert_eq!(detail["handoffs"], json!([]));
        assert_eq!(detail["sessions"], json!([]));
        assert_eq!(detail["children"].as_array().expect("a list").len(), 1);
        assert_eq!(detail["children"][0]["number"], json!(2));
    }
}

#[tokio::test]
async fn an_unresolvable_detail_reference_is_not_found() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "creator").await;
    let pid = project(&app, &user, "mars").await;
    let other = project(&app, &user, "phobos").await;

    let elsewhere = create_task(&app, &user, other, json!({ "title": "Elsewhere" })).await;

    for reference in [
        "77".to_string(),
        Uuid::new_v4().to_string(),
        "not-a-task".to_string(),
        elsewhere["id"].as_str().expect("a uuid").to_string(),
    ] {
        let response = app.get_as(&user, &task_path(pid, &reference)).await;
        assert_error(&response, StatusCode::NOT_FOUND, "not found");
    }
}

#[tokio::test]
async fn the_detail_needs_a_token() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "creator").await;
    let pid = project(&app, &user, "mars").await;
    create_task(&app, &user, pid, json!({ "title": "Epic" })).await;

    let response = app.server.get(&task_path(pid, "1")).await;
    response.assert_status(StatusCode::UNAUTHORIZED);
    response.assert_json(&unauthorized());
}

// ---- dashboard ----

#[tokio::test]
async fn the_dashboard_lists_the_human_state_across_projects() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "creator").await;
    let first = project(&app, &user, "mars").await;
    let second = project(&app, &user, "phobos").await;

    create_task(&app, &user, first, json!({ "title": "Queued" })).await;
    let here = create_task(
        &app,
        &user,
        first,
        json!({ "title": "Waiting here", "state": "needs_human" }),
    )
    .await;
    let there = create_task(
        &app,
        &user,
        second,
        json!({ "title": "Waiting there", "state": "needs_human" }),
    )
    .await;
    create_task(
        &app,
        &user,
        second,
        json!({ "title": "Closed", "state": "cancelled" }),
    )
    .await;

    let response = app.get_as(&user, "/api/tasks?state_kind=human").await;
    response.assert_status_ok();

    let listed = response.json::<Value>();
    let ids: Vec<&str> = listed
        .as_array()
        .expect("a list")
        .iter()
        .map(|task| task["id"].as_str().expect("a uuid"))
        .collect();

    assert_eq!(ids.len(), 2, "nothing from the queue or terminal states");
    assert!(ids.contains(&here["id"].as_str().expect("a uuid")));
    assert!(ids.contains(&there["id"].as_str().expect("a uuid")));
}

#[tokio::test]
async fn the_dashboard_requires_a_state_kind_it_recognises() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "creator").await;

    let missing = app.get_as(&user, "/api/tasks").await;
    assert_error(&missing, StatusCode::BAD_REQUEST, "state_kind is required");

    let unknown = app.get_as(&user, "/api/tasks?state_kind=waiting").await;
    assert_error(
        &unknown,
        StatusCode::BAD_REQUEST,
        "unknown state kind \"waiting\"; valid kinds are: queue, human, terminal",
    );
}

#[tokio::test]
async fn the_dashboard_needs_a_token() {
    let app = TestApp::spawn().await;

    let response = app.server.get("/api/tasks?state_kind=human").await;
    response.assert_status(StatusCode::UNAUTHORIZED);
    response.assert_json(&unauthorized());
}

// ---- release ----
//
// `POST /projects/{pid}/tasks/{id}/release` (`SPEC.md`, "Tasks"). What a
// release *is* — which columns move, which stay, which event it writes — is
// asserted against the tracker in `tests/tracker_leases.rs`; what is asserted
// here is the endpoint around it: its status codes, its body and the two ways
// of addressing the task.

/// A session of this project, so that a lease has something to point at.
///
/// Through the project's default profile, the seeded `implementer` over
/// `ready` (`SPEC.md`, "Role profile templates"), found by its flag rather
/// than by position.
async fn session(app: &TestApp, pid: Uuid) -> Uuid {
    let profile = ProjectRepository::new(&app.pool)
        .find_default_profile(pid)
        .await
        .expect("the profiles read")
        .expect("a new project has its default profile");

    let new = NewSession::new(
        pid,
        profile.id,
        ProfileKind::Conversational,
        "main",
        // Not a credential: a fake stand-in for the hashed MCP token (rule 3).
        format!("fake-mcp-token-hash-{}", Uuid::new_v4()),
    );

    let mut tx = app.pool.begin().await.expect("a transaction begins");
    let inserted = SessionRepository::new(&app.pool)
        .insert(&mut tx, &new)
        .await
        .expect("the session inserts");
    tx.commit().await.expect("the transaction commits");

    inserted.id
}

/// Put this session's lease on the task, through the claim a launch makes.
async fn claim(app: &TestApp, pid: Uuid, task_id: Uuid, session_id: Uuid, user_id: Uuid) {
    let mut mutation = TrackerMutation::begin(&app.pool, pid, TaskActor::User { user_id })
        .await
        .expect("the mutation opens");

    let task = TaskRepository::new(&app.pool)
        .find_task_for_update(mutation.conn(), pid, TaskRef::Id(task_id))
        .await
        .expect("the task reads")
        .expect("the task is in this project");

    claim_for_launch(&mut mutation, &task, session_id)
        .await
        .expect("the claim succeeds");
    mutation.commit().await.expect("the mutation commits");
}

/// `/api/projects/{pid}/tasks/{id}/release`.
fn release_path(pid: Uuid, id: &str) -> String {
    format!("/api/projects/{pid}/tasks/{id}/release")
}

#[tokio::test]
async fn releasing_a_held_task_clears_the_lease_and_keeps_the_state() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "releaser").await;
    let pid = project(&app, &user, "mars").await;

    let task = create_task(
        &app,
        &user,
        pid,
        json!({ "title": "Wire the tracker", "state": "ready" }),
    )
    .await;
    let task_id: Uuid = serde_json::from_value(task["id"].clone()).expect("a task id");
    let session_id = session(&app, pid).await;
    claim(&app, pid, task_id, session_id, user.user.id).await;

    let response = app
        .post_as(&user, &release_path(pid, &task_id.to_string()))
        .await;
    response.assert_status(StatusCode::OK);

    let released = response.json::<Value>();
    assert_eq!(released["id"], task["id"]);
    assert_eq!(released["lease_holder_session_id"], json!(null));
    assert_eq!(released["lease_since"], json!(null));
    // The state stays, and the attempt the claim counted stays with it: only a
    // state change resets `attempts` (`SPEC.md`, "Tasks").
    assert_eq!(released["state"], json!("ready"));
    assert_eq!(released["attempts"], json!(1));
    assert_eq!(released["closed_at"], json!(null));

    let stream = events(&app, pid).await;
    let (kind, payload) = stream.last().expect("the release is the last event");
    assert_eq!(kind, "released");
    assert_eq!(payload["reason"], json!("user"));
    assert_eq!(
        payload["actor"],
        json!({ "kind": "user", "user_id": user.user.id })
    );
    assert_eq!(payload["task"]["lease_holder_session_id"], json!(null));
    // A release is not a state change, so it carries neither end of one.
    assert_eq!(payload.get("from"), None);
    assert_eq!(payload.get("to"), None);
}

#[tokio::test]
async fn releasing_a_task_by_number_works_like_releasing_it_by_id() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "releaser").await;
    let pid = project(&app, &user, "mars").await;

    let task = create_task(&app, &user, pid, json!({ "title": "Wire the tracker" })).await;
    let task_id: Uuid = serde_json::from_value(task["id"].clone()).expect("a task id");
    let session_id = session(&app, pid).await;
    claim(&app, pid, task_id, session_id, user.user.id).await;

    let response = app.post_as(&user, &release_path(pid, "1")).await;
    response.assert_status(StatusCode::OK);
    assert_eq!(
        response.json::<Value>()["lease_holder_session_id"],
        json!(null)
    );
}

#[tokio::test]
async fn releasing_a_task_nobody_holds_is_a_conflict_that_writes_nothing() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "releaser").await;
    let pid = project(&app, &user, "mars").await;

    let task = create_task(&app, &user, pid, json!({ "title": "Nobody has this" })).await;
    let before = events(&app, pid).await.len();

    let response = app
        .post_as(
            &user,
            &release_path(pid, task["id"].as_str().expect("an id")),
        )
        .await;
    assert_error(&response, StatusCode::CONFLICT, "task is not held");

    assert_eq!(
        events(&app, pid).await.len(),
        before,
        "a refusal emits nothing"
    );
}

#[tokio::test]
async fn releasing_needs_a_token_a_project_and_a_task_that_exist() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "releaser").await;
    let pid = project(&app, &user, "mars").await;

    let anonymous = app.server.post(&release_path(pid, "1")).await;
    anonymous.assert_status(StatusCode::UNAUTHORIZED);
    anonymous.assert_json(&unauthorized());

    // An unknown project: the mutation's own lock refuses before anything else.
    let elsewhere = app.post_as(&user, &release_path(Uuid::new_v4(), "1")).await;
    elsewhere.assert_status(StatusCode::NOT_FOUND);

    // A task this project does not have, and a reference that addresses no
    // task at all: one 404 for both.
    let missing = app
        .post_as(&user, &release_path(pid, &Uuid::new_v4().to_string()))
        .await;
    missing.assert_status(StatusCode::NOT_FOUND);

    let unresolvable = app.post_as(&user, &release_path(pid, "not-a-task")).await;
    unresolvable.assert_status(StatusCode::NOT_FOUND);
}

// ---- update ----
//
// `PUT /projects/{pid}/tasks/{id}` (`SPEC.md`, "Tasks"). What each rule *is* —
// which columns a hand-off moves, what a re-parenting does to the `blocked`
// flags — is asserted against the tracker and the repository in
// `tests/tracker_state.rs`, `tests/tracker_graph.rs` and
// `tests/tracker_tasks.rs`. What is asserted here is the endpoint:
// its statuses, its bodies, and the events an edit owes in the order it owes
// them.

/// The project's stream as `(kind, task_id, payload)`.
///
/// The `deleted` event is the reason this exists beside [`events`]: what it
/// carries is the column, not the payload.
async fn event_rows(app: &TestApp, pid: Uuid) -> Vec<(String, Option<Uuid>, Value)> {
    TaskRepository::new(&app.pool)
        .list_task_events_after(pid, 0, 100)
        .await
        .expect("the events read")
        .into_iter()
        .map(|row| (row.kind, row.task_id, row.payload))
        .collect()
}

/// The kinds emitted after the first `skip` events, in order.
async fn kinds_after(app: &TestApp, pid: Uuid, skip: usize) -> Vec<String> {
    events(app, pid)
        .await
        .into_iter()
        .skip(skip)
        .map(|(kind, _)| kind)
        .collect()
}

/// The `id` of a created task's body.
fn id_of(task: &Value) -> Uuid {
    serde_json::from_value(task["id"].clone()).expect("a task id")
}

/// `PUT` this body onto the task and expect it to succeed.
async fn put_task(
    app: &TestApp,
    user: &AuthenticatedUser,
    pid: Uuid,
    id: Uuid,
    body: Value,
) -> Value {
    let response = app
        .put_as(user, &task_path(pid, &id.to_string()))
        .json(&body)
        .await;
    response.assert_status(StatusCode::OK);
    response.json::<Value>()
}

/// The task as the board sees it now.
async fn read_task(app: &TestApp, user: &AuthenticatedUser, pid: Uuid, id: Uuid) -> Value {
    let response = app.get_as(user, &task_path(pid, &id.to_string())).await;
    response.assert_status(StatusCode::OK);
    response.json::<Value>()
}

#[tokio::test]
async fn updating_a_field_emits_updated_and_repeating_it_emits_nothing() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "editor").await;
    let pid = project(&app, &user, "mars").await;

    let task = create_task(&app, &user, pid, json!({ "title": "Wire the tracker" })).await;
    let id = id_of(&task);
    let before = events(&app, pid).await.len();

    let updated = put_task(&app, &user, pid, id, json!({ "title": "Wire the board" })).await;
    assert_eq!(updated["title"], json!("Wire the board"));
    assert_eq!(updated["state"], json!("backlog"));

    let stream = events(&app, pid).await;
    assert_eq!(
        kinds_after(&app, pid, before).await,
        vec!["updated"],
        "a field change is one `updated` and nothing else",
    );
    let (_, payload) = stream.last().expect("the update");
    assert_eq!(payload["task"]["title"], json!("Wire the board"));

    // The same title again changes no column, so it emits nothing at all
    // (`SPEC.md`, "Tasks": "a request with no effective changes emits no task
    // event").
    let again = put_task(&app, &user, pid, id, json!({ "title": "Wire the board" })).await;
    assert_eq!(again["title"], json!("Wire the board"));
    assert_eq!(
        events(&app, pid).await.len(),
        before + 1,
        "a no-op update writes no event",
    );
}

#[tokio::test]
async fn a_different_state_hands_the_task_off() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "editor").await;
    let pid = project(&app, &user, "mars").await;

    let task = create_task(&app, &user, pid, json!({ "title": "Wire the tracker" })).await;
    let id = id_of(&task);
    let session_id = session(&app, pid).await;
    claim(&app, pid, id, session_id, user.user.id).await;
    let before = events(&app, pid).await.len();

    let moved = put_task(&app, &user, pid, id, json!({ "state": "ready" })).await;

    // A user is not bound by the lease: the move clears it and resets the
    // attempt the claim counted.
    assert_eq!(moved["state"], json!("ready"));
    assert_eq!(moved["lease_holder_session_id"], json!(null));
    assert_eq!(moved["lease_since"], json!(null));
    assert_eq!(moved["attempts"], json!(0));
    assert_eq!(moved["closed_at"], json!(null));

    let stream = events(&app, pid).await;
    assert_eq!(kinds_after(&app, pid, before).await, vec!["state_changed"]);
    let (_, payload) = stream.last().expect("the move");
    assert_eq!(payload["from"], json!("backlog"));
    assert_eq!(payload["to"], json!("ready"));
}

#[tokio::test]
async fn the_current_state_is_a_no_op_that_keeps_the_lease_and_the_attempts() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "editor").await;
    let pid = project(&app, &user, "mars").await;

    let task = create_task(&app, &user, pid, json!({ "title": "Wire the tracker" })).await;
    let id = id_of(&task);

    // Two claims with a user release between them: the release keeps the
    // attempts, so the task is held with `attempts = 2`.
    let session_id = session(&app, pid).await;
    claim(&app, pid, id, session_id, user.user.id).await;
    app.post_as(&user, &release_path(pid, &id.to_string()))
        .await
        .assert_status(StatusCode::OK);
    claim(&app, pid, id, session_id, user.user.id).await;

    let before = events(&app, pid).await.len();

    let same = put_task(&app, &user, pid, id, json!({ "state": "backlog" })).await;

    assert_eq!(same["state"], json!("backlog"));
    assert_eq!(
        same["lease_holder_session_id"],
        json!(session_id),
        "a state no-op preserves the lease",
    );
    assert_eq!(same["attempts"], json!(2));
    assert_eq!(
        events(&app, pid).await.len(),
        before,
        "a state no-op emits no state-change event",
    );
}

#[tokio::test]
async fn fields_and_a_state_change_together_emit_updated_then_state_changed() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "editor").await;
    let pid = project(&app, &user, "mars").await;

    let task = create_task(&app, &user, pid, json!({ "title": "Wire the tracker" })).await;
    let id = id_of(&task);
    let before = events(&app, pid).await.len();

    let moved = put_task(
        &app,
        &user,
        pid,
        id,
        json!({ "title": "Wire the board", "state": "ready", "priority": 1 }),
    )
    .await;

    assert_eq!(moved["title"], json!("Wire the board"));
    assert_eq!(moved["priority"], json!(1));
    assert_eq!(moved["state"], json!("ready"));

    assert_eq!(
        kinds_after(&app, pid, before).await,
        vec!["updated", "state_changed"],
        "the fields moved, then the task did",
    );
}

#[tokio::test]
async fn closing_a_task_stamps_closed_at_and_unblocks_its_dependant() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "editor").await;
    let pid = project(&app, &user, "mars").await;

    let prerequisite = create_task(&app, &user, pid, json!({ "title": "First this" })).await;
    let dependant = create_task(
        &app,
        &user,
        pid,
        json!({ "title": "Then this", "depends_on": [prerequisite["id"]] }),
    )
    .await;
    assert_eq!(dependant["blocked"], json!(true));

    let before = events(&app, pid).await.len();
    let closed = put_task(
        &app,
        &user,
        pid,
        id_of(&prerequisite),
        json!({ "state": "done" }),
    )
    .await;

    assert!(
        closed["closed_at"].is_string(),
        "a terminal state closes it"
    );
    assert_eq!(
        kinds_after(&app, pid, before).await,
        vec!["state_changed", "unblocked"],
    );
    assert_eq!(
        read_task(&app, &user, pid, id_of(&dependant)).await["blocked"],
        json!(false),
    );

    // Reopening clears `closed_at` and blocks the dependant again.
    let before = events(&app, pid).await.len();
    let reopened = put_task(
        &app,
        &user,
        pid,
        id_of(&prerequisite),
        json!({ "state": "backlog" }),
    )
    .await;

    assert_eq!(reopened["closed_at"], json!(null));
    assert_eq!(
        kinds_after(&app, pid, before).await,
        vec!["state_changed", "blocked"],
    );
    assert_eq!(
        read_task(&app, &user, pid, id_of(&dependant)).await["blocked"],
        json!(true),
    );
}

#[tokio::test]
async fn re_parenting_recomputes_both_parents() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "editor").await;
    let pid = project(&app, &user, "mars").await;

    let first = create_task(&app, &user, pid, json!({ "title": "Epic one" })).await;
    let second = create_task(&app, &user, pid, json!({ "title": "Epic two" })).await;
    let child = create_task(
        &app,
        &user,
        pid,
        json!({ "title": "The work", "parent_id": first["id"] }),
    )
    .await;

    // The open child already blocks its first parent.
    assert_eq!(
        read_task(&app, &user, pid, id_of(&first)).await["blocked"],
        json!(true),
    );

    let before = events(&app, pid).await.len();
    let moved = put_task(
        &app,
        &user,
        pid,
        id_of(&child),
        json!({ "parent_id": second["id"] }),
    )
    .await;

    assert_eq!(moved["parent_id"], second["id"]);
    assert_eq!(
        kinds_after(&app, pid, before).await,
        vec!["updated", "unblocked", "blocked"],
        "the child moved, the old parent was freed, the new one took it on",
    );
    assert_eq!(
        read_task(&app, &user, pid, id_of(&first)).await["blocked"],
        json!(false),
    );
    assert_eq!(
        read_task(&app, &user, pid, id_of(&second)).await["blocked"],
        json!(true),
    );

    // Clearing the parent makes the task top-level again and frees the second.
    let before = events(&app, pid).await.len();
    let orphaned = put_task(
        &app,
        &user,
        pid,
        id_of(&child),
        json!({ "parent_id": null }),
    )
    .await;

    assert_eq!(orphaned["parent_id"], json!(null));
    assert_eq!(
        kinds_after(&app, pid, before).await,
        vec!["updated", "unblocked"],
    );

    // And clearing it again changes nothing at all.
    let before = events(&app, pid).await.len();
    put_task(
        &app,
        &user,
        pid,
        id_of(&child),
        json!({ "parent_id": null }),
    )
    .await;
    assert_eq!(events(&app, pid).await.len(), before);
}

#[tokio::test]
async fn each_parent_rule_is_refused_with_its_own_message() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "editor").await;
    let pid = project(&app, &user, "mars").await;

    let parent = create_task(&app, &user, pid, json!({ "title": "Epic" })).await;
    let child = create_task(
        &app,
        &user,
        pid,
        json!({ "title": "The work", "parent_id": parent["id"] }),
    )
    .await;
    let loose = create_task(&app, &user, pid, json!({ "title": "Something else" })).await;

    let path = |task: &Value| task_path(pid, task["id"].as_str().expect("an id"));

    // An epic cannot become a child.
    let nested = app
        .put_as(&user, &path(&parent))
        .json(&json!({ "parent_id": loose["id"] }))
        .await;
    assert_error(
        &nested,
        StatusCode::BAD_REQUEST,
        "a task with children cannot get a parent",
    );

    // A child cannot receive children.
    let deep = app
        .put_as(&user, &path(&loose))
        .json(&json!({ "parent_id": child["id"] }))
        .await;
    assert_error(
        &deep,
        StatusCode::BAD_REQUEST,
        "a task with a parent cannot receive children",
    );

    // A parent that is not a task of this project.
    let elsewhere = app
        .put_as(&user, &path(&loose))
        .json(&json!({ "parent_id": Uuid::new_v4() }))
        .await;
    assert_error(
        &elsewhere,
        StatusCode::BAD_REQUEST,
        "parent must be a top-level task of the same project",
    );

    // And a task cannot be its own parent.
    let itself = app
        .put_as(&user, &path(&loose))
        .json(&json!({ "parent_id": loose["id"] }))
        .await;
    assert_error(
        &itself,
        StatusCode::BAD_REQUEST,
        "a task cannot be its own parent",
    );
}

#[tokio::test]
async fn an_assignee_that_is_not_a_user_is_refused() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "editor").await;
    let pid = project(&app, &user, "mars").await;

    let task = create_task(&app, &user, pid, json!({ "title": "Wire the tracker" })).await;
    let id = id_of(&task);
    let before = events(&app, pid).await.len();

    let response = app
        .put_as(&user, &task_path(pid, &id.to_string()))
        .json(&json!({ "assignee_user_id": Uuid::new_v4() }))
        .await;
    assert_error(&response, StatusCode::BAD_REQUEST, "unknown assignee");
    assert_eq!(
        events(&app, pid).await.len(),
        before,
        "a refusal emits nothing",
    );

    // A user that exists is assigned, and `null` unassigns again.
    let assigned = put_task(
        &app,
        &user,
        pid,
        id,
        json!({ "assignee_user_id": user.user.id }),
    )
    .await;
    assert_eq!(assigned["assignee_user_id"], json!(user.user.id));

    let cleared = put_task(&app, &user, pid, id, json!({ "assignee_user_id": null })).await;
    assert_eq!(cleared["assignee_user_id"], json!(null));
}

#[tokio::test]
async fn a_hand_off_is_refused_by_its_input_rules_before_any_git_work() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "editor").await;
    let pid = project(&app, &user, "mars").await;

    let task = create_task(&app, &user, pid, json!({ "title": "Wire the tracker" })).await;
    let path = task_path(pid, task["id"].as_str().expect("an id"));
    let handoff = json!({
        "kind": "revision",
        "source_session_id": Uuid::new_v4(),
        // Not a credential and not a real commit: an obviously fake object id.
        "commit": "0000000000000000000000000000000000000000",
        "comment": "Ready for review.",
    });

    // No state at all, and the state the task is already in: one message.
    let stateless = app
        .put_as(&user, &path)
        .json(&json!({ "handoff": handoff }))
        .await;
    assert_error(
        &stateless,
        StatusCode::BAD_REQUEST,
        "handoff requires a different target state",
    );

    let unmoved = app
        .put_as(&user, &path)
        .json(&json!({ "state": "backlog", "handoff": handoff }))
        .await;
    assert_error(
        &unmoved,
        StatusCode::BAD_REQUEST,
        "handoff requires a different target state",
    );

    let mut blank = handoff.clone();
    blank["comment"] = json!("   ");
    let empty = app
        .put_as(&user, &path)
        .json(&json!({ "state": "review", "handoff": blank }))
        .await;
    assert_error(
        &empty,
        StatusCode::BAD_REQUEST,
        "comment body must not be empty",
    );

    // A well-shaped hand-off gets past the input rules and is refused by the
    // first rule that needs the project: this one has never cloned, so its
    // repository cannot be locked (`SPEC.md`, "Code hand-offs and review").
    // What a *ready* project's publication does is `tests/handoffs_api.rs`.
    let not_ready = app
        .put_as(&user, &path)
        .json(&json!({ "state": "review", "handoff": handoff }))
        .await;
    assert_error(&not_ready, StatusCode::CONFLICT, "project is not ready");

    // None of that moved the task.
    assert_eq!(
        read_task(&app, &user, pid, id_of(&task)).await["state"],
        json!("backlog"),
    );
}

#[tokio::test]
async fn updating_needs_a_token_a_project_and_a_task_that_exist() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "editor").await;
    let pid = project(&app, &user, "mars").await;

    let anonymous = app
        .server
        .put(&task_path(pid, "1"))
        .json(&json!({ "title": "Wire the tracker" }))
        .await;
    anonymous.assert_status(StatusCode::UNAUTHORIZED);
    anonymous.assert_json(&unauthorized());

    let elsewhere = app
        .put_as(&user, &task_path(Uuid::new_v4(), "1"))
        .json(&json!({ "title": "Wire the tracker" }))
        .await;
    elsewhere.assert_status(StatusCode::NOT_FOUND);

    let missing = app
        .put_as(&user, &task_path(pid, &Uuid::new_v4().to_string()))
        .json(&json!({ "title": "Wire the tracker" }))
        .await;
    missing.assert_status(StatusCode::NOT_FOUND);
}

// ---- delete ----
//
// `DELETE /projects/{pid}/tasks/{id}` (`SPEC.md`, "Tasks"): 204, the cascades,
// the surviving dependants' recomputed flags and a `deleted` event that keeps
// the task's identity (ADR 0022).

#[tokio::test]
async fn deleting_a_prerequisite_removes_its_edges_and_keeps_its_identity() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "editor").await;
    let pid = project(&app, &user, "mars").await;

    let prerequisite = create_task(&app, &user, pid, json!({ "title": "First this" })).await;
    let other = create_task(&app, &user, pid, json!({ "title": "And this" })).await;

    // One dependant that also waits on `other`, and one that waits on nothing
    // else and additionally records where it was discovered.
    let waiting = create_task(
        &app,
        &user,
        pid,
        json!({
            "title": "Still waiting",
            "depends_on": [prerequisite["id"], other["id"]],
        }),
    )
    .await;
    let freed = create_task(
        &app,
        &user,
        pid,
        json!({ "title": "Freed by the delete", "depends_on": [prerequisite["id"]] }),
    )
    .await;
    app.post_as(
        &user,
        &format!(
            "{}/dependencies",
            task_path(pid, freed["id"].as_str().expect("an id"))
        ),
    )
    .json(&json!({ "depends_on": prerequisite["id"], "kind": "discovered_from" }))
    .await
    .assert_status(StatusCode::OK);

    let deleted_id = id_of(&prerequisite);
    let before = events(&app, pid).await.len();

    let response = app
        .delete_as(&user, &task_path(pid, &deleted_id.to_string()))
        .await;
    response.assert_status(StatusCode::NO_CONTENT);

    let rows = event_rows(&app, pid).await;
    let after = &rows[before..];

    let removals: Vec<Uuid> = after
        .iter()
        .filter(|(kind, _, _)| kind == "dependency_removed")
        .map(|(_, _, payload)| {
            serde_json::from_value(payload["task"]["id"].clone()).expect("an id")
        })
        .collect();
    assert_eq!(
        removals.len(),
        3,
        "one per incident edge: two `blocks` and one `discovered_from`",
    );
    assert_eq!(
        removals.iter().filter(|id| **id == id_of(&freed)).count(),
        2,
        "the dependant joined by both kinds loses both edges",
    );
    assert_eq!(
        removals.iter().filter(|id| **id == id_of(&waiting)).count(),
        1,
    );

    // Only the dependant with no other open prerequisite is unblocked.
    let flips: Vec<(&str, Uuid)> = after
        .iter()
        .filter(|(kind, _, _)| kind == "unblocked" || kind == "blocked")
        .map(|(kind, _, payload)| {
            (
                kind.as_str(),
                serde_json::from_value(payload["task"]["id"].clone()).expect("an id"),
            )
        })
        .collect();
    assert_eq!(flips, vec![("unblocked", id_of(&freed))]);
    assert_eq!(
        read_task(&app, &user, pid, id_of(&waiting)).await["blocked"],
        json!(true),
        "a dependant with another open prerequisite stays blocked",
    );

    // The `deleted` event comes last, keeps the UUID and carries no task.
    let (kind, task_id, payload) = after.last().expect("the deletion");
    assert_eq!(kind, "deleted");
    assert_eq!(*task_id, Some(deleted_id));
    assert_eq!(payload.get("task"), None);
    assert_eq!(
        payload["actor"],
        json!({ "kind": "user", "user_id": user.user.id }),
    );

    // The earlier events about the task still carry its id.
    assert!(
        rows.iter()
            .any(|(kind, id, _)| kind == "created" && *id == Some(deleted_id)),
        "history is not rewritten",
    );

    assert_eq!(task_count(&app.pool, pid).await, 3);
}

#[tokio::test]
async fn deleting_a_parent_leaves_its_children_top_level_and_silent() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "editor").await;
    let pid = project(&app, &user, "mars").await;

    let parent = create_task(&app, &user, pid, json!({ "title": "Epic" })).await;
    let child = create_task(
        &app,
        &user,
        pid,
        json!({ "title": "The work", "parent_id": parent["id"] }),
    )
    .await;
    let before = events(&app, pid).await.len();

    app.delete_as(
        &user,
        &task_path(pid, parent["id"].as_str().expect("an id")),
    )
    .await
    .assert_status(StatusCode::NO_CONTENT);

    assert_eq!(
        kinds_after(&app, pid, before).await,
        vec!["deleted"],
        "a child loses a parent, not a prerequisite, so it gets no event",
    );

    let orphan = read_task(&app, &user, pid, id_of(&child)).await;
    assert_eq!(orphan["parent_id"], json!(null));
    assert_eq!(orphan["blocked"], json!(false));
}

#[tokio::test]
async fn a_held_task_can_be_deleted_by_a_user() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "editor").await;
    let pid = project(&app, &user, "mars").await;

    let task = create_task(&app, &user, pid, json!({ "title": "Wire the tracker" })).await;
    let id = id_of(&task);
    let session_id = session(&app, pid).await;
    claim(&app, pid, id, session_id, user.user.id).await;

    // By number, which is the other accepted form of the same address.
    app.delete_as(&user, &task_path(pid, "1"))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    assert_eq!(task_count(&app.pool, pid).await, 0);
}

#[tokio::test]
async fn deleting_needs_a_token_a_project_and_a_task_that_exist() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "editor").await;
    let pid = project(&app, &user, "mars").await;

    let anonymous = app.server.delete(&task_path(pid, "1")).await;
    anonymous.assert_status(StatusCode::UNAUTHORIZED);
    anonymous.assert_json(&unauthorized());

    let elsewhere = app.delete_as(&user, &task_path(Uuid::new_v4(), "1")).await;
    elsewhere.assert_status(StatusCode::NOT_FOUND);

    let missing = app
        .delete_as(&user, &task_path(pid, &Uuid::new_v4().to_string()))
        .await;
    missing.assert_status(StatusCode::NOT_FOUND);

    let unresolvable = app.delete_as(&user, &task_path(pid, "not-a-task")).await;
    unresolvable.assert_status(StatusCode::NOT_FOUND);
}

// ---- hand-offs ----
//
// `Task.handoff` and `TaskDetail.handoffs` (`SPEC.md`, "Tasks"; `SPEC.md`,
// "Code hand-offs and review"). Publishing one through `PUT` is the hand-off
// epic's; what is asserted here is what the read side makes of the rows: that
// the embedded record is the one `tasks.current_handoff_id` names rather than
// the newest, that the history is oldest first, that the list endpoint carries
// the same field on every task, and that a deleted actor leaves the branch,
// the commit and the review timestamp behind.

/// An obviously fake but well-formed SHA-1 object id (rule 3).
const HANDOFF_COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

/// Long enough that two mutations get distinct `NOW()` transaction times,
/// short enough not to drag the suite out.
const BETWEEN_HANDOFFS: Duration = Duration::from_millis(50);

/// One hand-off record on `task_id`, with its comment, in one mutation — a
/// row of the task's history that the pointer does not name.
///
/// The insert is the repository's, because the cross-table scope rules of a
/// hand-off insert are all there is to it and no verb wraps them
/// (`CLAUDE.md`, "Testing expectations"). The record that *is* current is
/// published through the verb instead ([`publish_current_handoff`]).
async fn handoff_record(
    app: &TestApp,
    pid: Uuid,
    task_id: Uuid,
    source_session_id: Uuid,
    branch: &str,
    reviewed_by: Option<Uuid>,
) -> Uuid {
    let repository = TaskRepository::new(&app.pool);
    let mut mutation = TrackerMutation::begin(&app.pool, pid, TaskActor::System)
        .await
        .expect("the mutation opens");

    let comment = NewTaskComment::from_session(task_id, source_session_id, format!("on {branch}"));
    repository
        .insert_comment(mutation.conn(), pid, &comment)
        .await
        .expect("the comment inserts");

    let mut handoff = NewTaskHandoff::new(task_id, branch, HANDOFF_COMMIT, comment.id);
    handoff.source_session_id = Some(source_session_id);
    handoff.created_by_session_id = Some(source_session_id);
    if let Some(user_id) = reviewed_by {
        handoff.review_status = ReviewStatus::Approved;
        handoff.reviewed_by_user_id = Some(user_id);
        handoff.reviewed_at = Some(Utc::now());
    }

    let inserted = repository
        .insert_handoff(mutation.conn(), pid, &handoff)
        .await
        .expect("the hand-off inserts");
    mutation.commit().await.expect("the mutation commits");

    inserted.id
}

/// Publish a hand-off on `task_id` and leave it the current one.
///
/// `tracker::handoffs::publish_in_transaction` is the only writer of
/// `tasks.current_handoff_id`, and a publication always moves the task, so the
/// fixture moves it back (`common::tracker::handoff_in_place`). `review` is
/// what the record's review fields say: a revision is unreviewed, a forward
/// carries its reviewer's verdict.
async fn publish_current_handoff(
    app: &TestApp,
    pid: Uuid,
    task_id: Uuid,
    source_session_id: Uuid,
    branch: &str,
    caller: HandoffCaller,
    review: ReviewCarry,
) -> Uuid {
    if let HandoffCaller::Session { session_id } = caller {
        common::tracker::hold(&app.pool, pid, task_id, session_id).await;
    }

    let (_, handoff_id) = common::tracker::handoff_in_place(
        &app.pool,
        pid,
        task_id,
        common::tracker::Handoff {
            source_session_id: Some(source_session_id),
            source_branch: branch,
            commit: HANDOFF_COMMIT,
            comment: "on the branch",
            target_state: "",
            caller,
            review,
        },
    )
    .await;

    handoff_id
}

#[tokio::test]
async fn the_detail_lists_hand_offs_oldest_first_beside_the_one_the_column_names() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "reviewer").await;
    let pid = project(&app, &user, "mars").await;

    let task = create_task(&app, &user, pid, json!({ "title": "Wire the tracker" })).await;
    let id = id_of(&task);
    let session_id = session(&app, pid).await;

    // The *current* record is the pointer's, "never by timestamp"
    // (`docs/data-model.md`): the middle one here, with an older and a newer
    // record of the same task around it.
    let first = handoff_record(&app, pid, id, session_id, "session/one", None).await;
    sleep(BETWEEN_HANDOFFS).await;
    let current = publish_current_handoff(
        &app,
        pid,
        id,
        session_id,
        "session/two",
        HandoffCaller::Session { session_id },
        ReviewCarry::Fresh,
    )
    .await;
    sleep(BETWEEN_HANDOFFS).await;
    let last = handoff_record(&app, pid, id, session_id, "session/three", None).await;
    let published = [first, current, last];

    let detail = read_task(&app, &user, pid, id).await;

    let listed = detail["handoffs"].as_array().expect("a list");
    assert_eq!(
        listed
            .iter()
            .map(|handoff| handoff["source_branch"].clone())
            .collect::<Vec<_>>(),
        vec![
            json!("session/one"),
            json!("session/two"),
            json!("session/three"),
        ],
        "oldest first",
    );
    assert_eq!(
        listed
            .iter()
            .map(|handoff| handoff["id"].clone())
            .collect::<Vec<_>>(),
        published.iter().map(|id| json!(id)).collect::<Vec<_>>(),
    );

    assert_eq!(detail["handoff"]["id"], json!(published[1]));
    assert_eq!(detail["handoff"]["task_id"], json!(id));
    assert_eq!(detail["handoff"]["source_branch"], json!("session/two"));
    assert_eq!(detail["handoff"]["commit"], json!(HANDOFF_COMMIT));
    assert_eq!(detail["handoff"]["source_session_id"], json!(session_id));
    assert_eq!(detail["handoff"]["review_status"], json!("unreviewed"));
    assert_eq!(detail["handoff"]["reviewed_at"], json!(null));
    assert_eq!(detail["handoff"]["reviewed_by_user_id"], json!(null));
    assert_eq!(detail["handoff"]["reviewed_by_session_id"], json!(null));
    assert_eq!(detail["handoff"]["created_by_user_id"], json!(null));
    assert_eq!(
        detail["handoff"]["created_by_session_id"],
        json!(session_id)
    );
    assert!(detail["handoff"]["comment_id"].is_string());
    assert!(detail["handoff"]["created_at"].is_string());
}

#[tokio::test]
async fn the_project_list_embeds_the_current_hand_off_on_every_task_that_has_one() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "reviewer").await;
    let pid = project(&app, &user, "mars").await;

    let handed_off = create_task(&app, &user, pid, json!({ "title": "Handed off" })).await;
    let plain = create_task(&app, &user, pid, json!({ "title": "Plain" })).await;
    let session_id = session(&app, pid).await;

    let handoff_id = publish_current_handoff(
        &app,
        pid,
        id_of(&handed_off),
        session_id,
        "session/one",
        HandoffCaller::Session { session_id },
        ReviewCarry::Fresh,
    )
    .await;

    let response = app.get_as(&user, &tasks_path(pid)).await;
    response.assert_status_ok();
    let list = response.json::<Value>();
    assert_eq!(numbers(&list), vec![1, 2]);

    assert_eq!(list[0]["id"], handed_off["id"]);
    assert_eq!(list[0]["handoff"]["id"], json!(handoff_id));
    assert_eq!(list[0]["handoff"]["source_branch"], json!("session/one"));
    assert_eq!(list[1]["id"], plain["id"]);
    assert_eq!(list[1]["handoff"], json!(null));
}

#[tokio::test]
async fn a_hand_off_keeps_its_branch_commit_and_review_when_its_actors_are_deleted() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "owner").await;
    let approver = signed_in(&app, "approver").await;
    let pid = project(&app, &user, "mars").await;

    let task = create_task(&app, &user, pid, json!({ "title": "Wire the tracker" })).await;
    let id = id_of(&task);
    let session_id = session(&app, pid).await;

    let handoff_id = publish_current_handoff(
        &app,
        pid,
        id,
        session_id,
        "session/one",
        HandoffCaller::User {
            user_id: approver.user.id,
        },
        ReviewCarry::Decision(ReviewDecision::Approved),
    )
    .await;

    let before = read_task(&app, &user, pid, id).await;
    assert_eq!(before["handoff"]["review_status"], json!("approved"));
    assert_eq!(
        before["handoff"]["reviewed_by_user_id"],
        json!(approver.user.id)
    );
    let reviewed_at = before["handoff"]["reviewed_at"].clone();
    assert!(reviewed_at.is_string());

    // No interface deletes a session row or a user here, and the fact under
    // test is the `ON DELETE SET NULL` those columns carry.
    sqlx::query("DELETE FROM sessions WHERE id = $1")
        .bind(session_id)
        .execute(&app.pool)
        .await
        .expect("the session row is deleted");
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(approver.user.id)
        .execute(&app.pool)
        .await
        .expect("the reviewing user is deleted");

    let after = read_task(&app, &user, pid, id).await;

    assert_eq!(after["handoff"]["id"], json!(handoff_id));
    assert_eq!(after["handoff"]["source_session_id"], json!(null));
    assert_eq!(after["handoff"]["created_by_session_id"], json!(null));
    assert_eq!(after["handoff"]["reviewed_by_user_id"], json!(null));
    // What deletion never takes away.
    assert_eq!(after["handoff"]["source_branch"], json!("session/one"));
    assert_eq!(after["handoff"]["commit"], json!(HANDOFF_COMMIT));
    assert_eq!(after["handoff"]["review_status"], json!("approved"));
    assert_eq!(after["handoff"]["reviewed_at"], reviewed_at);

    assert_eq!(after["handoffs"].as_array().expect("a list").len(), 1);
    assert_eq!(after["handoffs"][0]["id"], json!(handoff_id));
}
