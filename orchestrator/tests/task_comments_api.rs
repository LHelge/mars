//! `POST /api/projects/{pid}/tasks/{id}/comments` through the real router
//! (`SPEC.md`, "Tasks" and "TaskEvent"; `docs/data-model.md`,
//! `task_comments`).
//!
//! The tracker's one open write: there is no lease to hold and no state to be
//! in, because a comment is how whoever noticed something tells whoever picks
//! the task up next. What is asserted here is the endpoint's contract —
//! the 201 and the `Comment` body, the caller recorded as the author, the one
//! `commented` event carrying the comment, the refusals, and the comment
//! showing up on the task's detail.
//!
//! Bodies are stored and displayed unredacted (ADR 0027), which is why the
//! assertions compare them verbatim.
//!
//! Every credential-shaped value is an obviously fake stand-in (rule 3).
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use axum::http::StatusCode;
use axum_test::TestResponse;
use common::{AuthenticatedUser, TestApp};
use mars_orchestrator::prelude::*;
use mars_orchestrator::projects::{NewProjectRequest, create_project};
use mars_orchestrator::repositories::TaskRepository;
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

/// A plain task in the default `backlog` state.
async fn open_task(app: &TestApp, user: &AuthenticatedUser, pid: Uuid, title: &str) -> Value {
    let response = app
        .post_as(user, &format!("/api/projects/{pid}/tasks"))
        .json(&json!({ "title": title }))
        .await;
    response.assert_status(StatusCode::CREATED);
    response.json::<Value>()
}

/// A task's id as the path segment form the endpoint takes.
fn id_of(task: &Value) -> String {
    task["id"].as_str().expect("a task id").to_string()
}

/// `/api/projects/{pid}/tasks/{id}/comments`.
fn comments_path(pid: Uuid, id: &str) -> String {
    format!("/api/projects/{pid}/tasks/{id}/comments")
}

/// `POST` a comment and answer with the raw response.
async fn post_comment(
    app: &TestApp,
    user: &AuthenticatedUser,
    pid: Uuid,
    id: &str,
    body: Value,
) -> TestResponse {
    app.post_as(user, &comments_path(pid, id)).json(&body).await
}

/// The project's event stream from the beginning, as `(kind, payload)`.
async fn events(app: &TestApp, pid: Uuid) -> Vec<(String, Value)> {
    TaskRepository::new(&app.pool)
        .list_task_events_after(pid, 0, 100)
        .await
        .expect("the events read")
        .into_iter()
        .map(|row| (row.kind, row.payload))
        .collect()
}

/// How many comments the task has, straight from the table.
async fn comment_count(pool: &PgPool, task_id: Uuid) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM task_comments WHERE task_id = $1")
        .bind(task_id)
        .fetch_one(pool)
        .await
        .expect("the count runs")
}

// ---- writing ----

#[tokio::test]
async fn a_comment_is_201_with_the_caller_as_its_author_and_one_event() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;
    let task = open_task(&app, &user, pid, "Ship the routes").await;

    let body = "The migration has to land first.";
    let response = post_comment(&app, &user, pid, &id_of(&task), json!({ "body": body })).await;

    response.assert_status(StatusCode::CREATED);
    let comment = response.json::<Value>();

    assert_eq!(comment["task_id"], task["id"]);
    assert_eq!(comment["author_user_id"], json!(user.user.id));
    assert_eq!(comment["author_session_id"], json!(null));
    assert_eq!(comment["system"], json!(false));
    assert_eq!(comment["body"], json!(body));
    assert!(comment["created_at"].is_string());

    let stream = events(&app, pid).await;
    let kinds: Vec<&str> = stream.iter().map(|e| e.0.as_str()).collect();
    assert_eq!(kinds, vec!["created", "commented"]);

    let (_, commented) = &stream[1];
    assert_eq!(
        commented["actor"],
        json!({ "kind": "user", "user_id": user.user.id })
    );
    assert_eq!(commented["task"]["id"], task["id"]);
    assert_eq!(commented["comment"]["body"], json!(body));
    assert_eq!(commented["comment"]["id"], comment["id"]);
}

#[tokio::test]
async fn a_comment_shows_up_on_the_tasks_detail_oldest_first() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;
    let task = open_task(&app, &user, pid, "Ship the routes").await;
    let id = id_of(&task);

    for body in ["First thought.", "Second thought."] {
        post_comment(&app, &user, pid, &id, json!({ "body": body }))
            .await
            .assert_status(StatusCode::CREATED);
    }

    let response = app
        .get_as(&user, &format!("/api/projects/{pid}/tasks/{id}"))
        .await;
    response.assert_status(StatusCode::OK);
    let detail = response.json::<Value>();

    let bodies: Vec<&str> = detail["comments"]
        .as_array()
        .expect("a comment list")
        .iter()
        .map(|comment| comment["body"].as_str().expect("a body"))
        .collect();
    assert_eq!(bodies, vec!["First thought.", "Second thought."]);
}

#[tokio::test]
async fn any_user_may_comment_on_any_task_of_the_project() {
    let app = TestApp::spawn().await;
    let owner = signed_in(&app, "ada").await;
    let other = signed_in(&app, "bob").await;
    let pid = project(&app, &owner, "mars").await;
    let task = open_task(&app, &owner, pid, "Ship the routes").await;

    let response = post_comment(
        &app,
        &other,
        pid,
        &id_of(&task),
        json!({ "body": "Reviewed the plan; looks right." }),
    )
    .await;

    response.assert_status(StatusCode::CREATED);
    assert_eq!(
        response.json::<Value>()["author_user_id"],
        json!(other.user.id)
    );
}

#[tokio::test]
async fn a_task_is_addressed_by_its_number_as_well_as_its_uuid() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;
    let task = open_task(&app, &user, pid, "Ship the routes").await;

    let response = post_comment(&app, &user, pid, "1", json!({ "body": "By number." })).await;

    response.assert_status(StatusCode::CREATED);
    assert_eq!(response.json::<Value>()["task_id"], task["id"]);
}

// ---- refusals ----

#[tokio::test]
async fn an_empty_body_is_400_and_writes_nothing() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;
    let task = open_task(&app, &user, pid, "Ship the routes").await;
    let task_id: Uuid = task["id"].as_str().expect("an id").parse().expect("a UUID");

    for body in ["", "   \n  "] {
        let response = post_comment(&app, &user, pid, &id_of(&task), json!({ "body": body })).await;
        assert_error(
            &response,
            StatusCode::BAD_REQUEST,
            "comment body must not be empty",
        );
    }

    assert_eq!(comment_count(&app.pool, task_id).await, 0);
    let kinds: Vec<String> = events(&app, pid).await.into_iter().map(|e| e.0).collect();
    assert_eq!(kinds, vec!["created"], "a refused comment emits nothing");
}

#[tokio::test]
async fn a_task_of_another_project_and_an_unknown_project_are_both_404() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;
    let other = project(&app, &user, "phobos").await;

    let elsewhere = open_task(&app, &user, other, "Somebody else's work").await;

    // The task exists, but not under this project's path.
    let response = post_comment(
        &app,
        &user,
        pid,
        &id_of(&elsewhere),
        json!({ "body": "Wrong door." }),
    )
    .await;
    assert_error(&response, StatusCode::NOT_FOUND, "not found");

    // An unknown task, and an unknown project.
    let response = post_comment(
        &app,
        &user,
        pid,
        &Uuid::new_v4().to_string(),
        json!({ "body": "Nobody home." }),
    )
    .await;
    assert_error(&response, StatusCode::NOT_FOUND, "not found");

    let response = post_comment(
        &app,
        &user,
        Uuid::new_v4(),
        "1",
        json!({ "body": "Nobody home." }),
    )
    .await;
    assert_error(&response, StatusCode::NOT_FOUND, "not found");
}

#[tokio::test]
async fn commenting_needs_a_token() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;
    let task = open_task(&app, &user, pid, "Ship the routes").await;

    let response = app
        .server
        .post(&comments_path(pid, &id_of(&task)))
        .json(&json!({ "body": "Anonymous." }))
        .await;

    response.assert_status(StatusCode::UNAUTHORIZED);
    response.assert_json(&json!({ "status": 401, "error": "authentication required" }));
}
