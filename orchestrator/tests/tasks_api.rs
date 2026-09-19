//! `/api/projects/{pid}/tasks`, `/api/projects/{pid}/tasks/{id}` and the
//! dashboard's `/api/tasks` through the real router (`SPEC.md`, "Tasks";
//! `SPEC.md`, "Frontend" → "Dashboard").
//!
//! The read side of the tracker and the creation that fills it. What the
//! *rules* are is asserted against the models and against a real database in
//! `tests/repositories_tasks_rows.rs` and `tests/repositories_tasks_core.rs`;
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

use axum::http::StatusCode;
use axum_test::TestResponse;
use common::{AuthenticatedUser, TestApp};
use mars_orchestrator::events::TaskActor;
use mars_orchestrator::models::{NewSession, ProfileKind, TaskRef};
use mars_orchestrator::prelude::*;
use mars_orchestrator::projects::{NewProjectRequest, create_project};
use mars_orchestrator::repositories::{ProjectRepository, SessionRepository, TaskRepository};
use mars_orchestrator::tracker::{TrackerMutation, claim_for_launch};
use serde_json::{Value, json};
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
/// Through the project's own `default` profile, which `create_project` seeded
/// serving `ready`.
async fn session(app: &TestApp, pid: Uuid) -> Uuid {
    let profile = ProjectRepository::new(&app.pool)
        .list_profiles(pid)
        .await
        .expect("the profiles read")
        .into_iter()
        .next()
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
