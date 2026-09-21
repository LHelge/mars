//! `POST` and `DELETE /api/projects/{pid}/tasks/{id}/dependencies` through the
//! real router (`SPEC.md`, "Tasks" and "TaskEvent"; `ARCHITECTURE.md`, "Task
//! tracker" → "Blocked is stored").
//!
//! What the graph *rules* are is asserted against a real database in
//! `tests/tracker_graph.rs` and `tests/tracker_tasks.rs`; what is asserted
//! here is everything the
//! two endpoints add on top:
//!
//! - the documented status and body of each success and each refusal, and the
//!   JWT requirement on both paths;
//! - that an edge writes the events it owes, in the order `SPEC.md` gives
//!   them: `dependency_added` (or `dependency_removed`) and then the `blocked`
//!   or `unblocked` flip it caused;
//! - that the kind is part of the edge's identity, so the three coexist and
//!   are removed one at a time;
//! - that two reciprocal `blocks` requests issued together cannot both be
//!   accepted, because they serialise on the project lock (ADR 0021).
//!
//! Projects are created through `projects::create_project` for the reason
//! `tests/tasks_api.rs` gives: what these scenarios need is the seven default
//! states and the task-number counter, and no task endpoint touches git or the
//! engine.
//!
//! Every credential-shaped value is an obviously fake stand-in (rule 3).
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use std::time::Duration;

use axum::http::StatusCode;
use axum_test::TestResponse;
use common::races::{RACE_TIMEOUT, hold_project_lock, release_when_blocked};
use common::{AuthenticatedUser, TestApp};
use mars_orchestrator::projects::{NewProjectRequest, create_project};
use mars_orchestrator::repositories::TaskRepository;
use serde_json::{Value, json};
use tokio::time::timeout;
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

/// Create a task and answer with it.
async fn task(app: &TestApp, user: &AuthenticatedUser, pid: Uuid, body: Value) -> Value {
    let response = app
        .post_as(user, &format!("/api/projects/{pid}/tasks"))
        .json(&body)
        .await;
    response.assert_status(StatusCode::CREATED);
    response.json::<Value>()
}

/// A plain task in the default `backlog` state.
async fn open_task(app: &TestApp, user: &AuthenticatedUser, pid: Uuid, title: &str) -> Value {
    task(app, user, pid, json!({ "title": title })).await
}

/// `/api/projects/{pid}/tasks/{id}/dependencies`.
fn dependencies_path(pid: Uuid, id: &str) -> String {
    format!("/api/projects/{pid}/tasks/{id}/dependencies")
}

/// `/api/projects/{pid}/tasks/{id}/dependencies/{dep}`.
fn dependency_path(pid: Uuid, id: &str, dep: &str) -> String {
    format!("/api/projects/{pid}/tasks/{id}/dependencies/{dep}")
}

/// A task's id as the path segment form the endpoints take.
fn id_of(task: &Value) -> String {
    task["id"].as_str().expect("a task id").to_string()
}

/// `POST` an edge and answer with the raw response.
async fn post_dependency(
    app: &TestApp,
    user: &AuthenticatedUser,
    pid: Uuid,
    id: &str,
    body: Value,
) -> TestResponse {
    app.post_as(user, &dependencies_path(pid, id))
        .json(&body)
        .await
}

/// `POST` an edge and expect it to be accepted.
async fn add_dependency(
    app: &TestApp,
    user: &AuthenticatedUser,
    pid: Uuid,
    id: &str,
    body: Value,
) -> Value {
    let response = post_dependency(app, user, pid, id, body).await;
    response.assert_status(StatusCode::OK);
    response.json::<Value>()
}

/// `DELETE` an edge of this kind and answer with the raw response.
async fn delete_dependency(
    app: &TestApp,
    user: &AuthenticatedUser,
    pid: Uuid,
    id: &str,
    dep: &str,
    query: &str,
) -> TestResponse {
    app.delete_as(user, &format!("{}{query}", dependency_path(pid, id, dep)))
        .await
}

/// The project's event kinds from the beginning, in order.
async fn event_kinds(app: &TestApp, pid: Uuid) -> Vec<String> {
    events(app, pid).await.into_iter().map(|e| e.0).collect()
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

/// A task read back through `GET /projects/{pid}/tasks/{id}`.
async fn read_task(app: &TestApp, user: &AuthenticatedUser, pid: Uuid, id: &str) -> Value {
    let response = app
        .get_as(user, &format!("/api/projects/{pid}/tasks/{id}"))
        .await;
    response.assert_status(StatusCode::OK);
    response.json::<Value>()
}

// ---- adding ----

#[tokio::test]
async fn a_blocks_edge_blocks_the_dependant_and_emits_the_edge_then_the_flip() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;

    let prerequisite = open_task(&app, &user, pid, "Ship the schema").await;
    let dependant = open_task(&app, &user, pid, "Ship the routes").await;

    let updated = add_dependency(
        &app,
        &user,
        pid,
        &id_of(&dependant),
        json!({ "depends_on": id_of(&prerequisite) }),
    )
    .await;

    assert_eq!(updated["id"], dependant["id"]);
    assert_eq!(updated["blocked"], json!(true));
    assert_eq!(
        updated["depends_on"],
        json!([{ "task_id": prerequisite["id"], "kind": "blocks" }]),
        "the kind defaults to blocks",
    );

    // The reverse list is on the prerequisite.
    let prerequisite = read_task(&app, &user, pid, &id_of(&prerequisite)).await;
    assert_eq!(prerequisite["blocks"], json!([dependant["id"]]));
    assert_eq!(
        prerequisite["blocked"],
        json!(false),
        "a prerequisite is not blocked by its dependant",
    );

    let stream = events(&app, pid).await;
    let kinds: Vec<&str> = stream.iter().map(|e| e.0.as_str()).collect();
    assert_eq!(
        kinds,
        vec!["created", "created", "dependency_added", "blocked"]
    );

    let (_, added) = &stream[2];
    assert_eq!(added["task"]["id"], dependant["id"]);
    assert_eq!(
        added["actor"],
        json!({ "kind": "user", "user_id": user.user.id })
    );
    let (_, blocked) = &stream[3];
    assert_eq!(blocked["task"]["blocked"], json!(true));
}

#[tokio::test]
async fn a_second_kind_on_the_same_pair_is_accepted_and_flips_nothing() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;

    let prerequisite = open_task(&app, &user, pid, "Ship the schema").await;
    let dependant = open_task(&app, &user, pid, "Ship the routes").await;
    let id = id_of(&dependant);

    add_dependency(
        &app,
        &user,
        pid,
        &id,
        json!({ "depends_on": id_of(&prerequisite), "kind": "blocks" }),
    )
    .await;
    let before = event_kinds(&app, pid).await.len();

    let updated = add_dependency(
        &app,
        &user,
        pid,
        &id,
        json!({ "depends_on": id_of(&prerequisite), "kind": "discovered_from" }),
    )
    .await;

    assert_eq!(
        updated["depends_on"],
        json!([
            { "task_id": prerequisite["id"], "kind": "blocks" },
            { "task_id": prerequisite["id"], "kind": "discovered_from" },
        ]),
        "the kind is part of the edge's identity, so both stand",
    );
    assert_eq!(updated["blocked"], json!(true));

    let kinds = event_kinds(&app, pid).await;
    assert_eq!(
        &kinds[before..],
        ["dependency_added"],
        "a non-blocking kind flips nothing",
    );
}

#[tokio::test]
async fn a_blocks_edge_on_a_terminal_prerequisite_flips_nothing() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;

    let prerequisite = task(
        &app,
        &user,
        pid,
        json!({ "title": "Already done", "state": "done" }),
    )
    .await;
    let dependant = open_task(&app, &user, pid, "Ship the routes").await;
    let before = event_kinds(&app, pid).await.len();

    let updated = add_dependency(
        &app,
        &user,
        pid,
        &id_of(&dependant),
        json!({ "depends_on": id_of(&prerequisite) }),
    )
    .await;

    assert_eq!(
        updated["blocked"],
        json!(false),
        "a terminal prerequisite satisfies the dependency",
    );

    let kinds = event_kinds(&app, pid).await;
    assert_eq!(&kinds[before..], ["dependency_added"]);
}

#[tokio::test]
async fn a_task_is_addressed_by_its_number_as_well_as_its_uuid() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;

    let prerequisite = open_task(&app, &user, pid, "Ship the schema").await;
    let dependant = open_task(&app, &user, pid, "Ship the routes").await;

    let updated = add_dependency(&app, &user, pid, "2", json!({ "depends_on": "1" })).await;

    assert_eq!(updated["id"], dependant["id"]);
    assert_eq!(
        updated["depends_on"],
        json!([{ "task_id": prerequisite["id"], "kind": "blocks" }])
    );
}

// ---- refusals ----

#[tokio::test]
async fn an_edge_that_would_close_a_cycle_is_409_and_only_among_blocks_edges() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;

    // A → B → C: A depends on B, B depends on C.
    let a = open_task(&app, &user, pid, "A").await;
    let b = open_task(&app, &user, pid, "B").await;
    let c = open_task(&app, &user, pid, "C").await;

    add_dependency(
        &app,
        &user,
        pid,
        &id_of(&a),
        json!({ "depends_on": id_of(&b) }),
    )
    .await;
    add_dependency(
        &app,
        &user,
        pid,
        &id_of(&b),
        json!({ "depends_on": id_of(&c) }),
    )
    .await;

    let response = post_dependency(
        &app,
        &user,
        pid,
        &id_of(&c),
        json!({ "depends_on": id_of(&a) }),
    )
    .await;
    assert_error(
        &response,
        StatusCode::CONFLICT,
        "dependency would create a cycle",
    );

    // The same ring of `related` edges is merely a fact about the work.
    let updated = add_dependency(
        &app,
        &user,
        pid,
        &id_of(&c),
        json!({ "depends_on": id_of(&a), "kind": "related" }),
    )
    .await;
    assert_eq!(
        updated["depends_on"],
        json!([{ "task_id": a["id"], "kind": "related" }])
    );
    assert_eq!(updated["blocked"], json!(false));
}

#[tokio::test]
async fn the_same_edge_twice_is_409() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;

    let prerequisite = open_task(&app, &user, pid, "Ship the schema").await;
    let dependant = open_task(&app, &user, pid, "Ship the routes").await;
    let body = json!({ "depends_on": id_of(&prerequisite) });

    add_dependency(&app, &user, pid, &id_of(&dependant), body.clone()).await;
    let before = event_kinds(&app, pid).await.len();

    let response = post_dependency(&app, &user, pid, &id_of(&dependant), body).await;
    assert_error(&response, StatusCode::CONFLICT, "dependency already exists");

    assert_eq!(
        event_kinds(&app, pid).await.len(),
        before,
        "a refused edge writes nothing",
    );
}

#[tokio::test]
async fn a_task_cannot_depend_on_itself() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;

    let task = open_task(&app, &user, pid, "Ship the routes").await;
    let id = id_of(&task);

    let response = post_dependency(&app, &user, pid, &id, json!({ "depends_on": id })).await;
    assert_error(
        &response,
        StatusCode::BAD_REQUEST,
        "a task cannot depend on itself",
    );
}

#[tokio::test]
async fn an_end_in_another_project_is_400_by_uuid_and_404_by_number() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;
    let other = project(&app, &user, "phobos").await;

    let dependant = open_task(&app, &user, pid, "Ship the routes").await;
    assert_eq!(dependant["number"], json!(1), "this project has only a 1");

    // Number 2 exists in the other project and nowhere else.
    open_task(&app, &user, other, "Somebody else's first").await;
    let elsewhere = open_task(&app, &user, other, "Somebody else's work").await;
    assert_eq!(elsewhere["number"], json!(2));

    // The UUID names a task, so the caller is told the edge is illegal.
    let response = post_dependency(
        &app,
        &user,
        pid,
        &id_of(&dependant),
        json!({ "depends_on": id_of(&elsewhere) }),
    )
    .await;
    assert_error(
        &response,
        StatusCode::BAD_REQUEST,
        "dependency must reference tasks of the same project",
    );

    // A number is per project, so `2` names nothing here.
    let response = post_dependency(&app, &user, pid, "1", json!({ "depends_on": "2" })).await;
    assert_error(&response, StatusCode::NOT_FOUND, "not found");
}

#[tokio::test]
async fn an_unknown_kind_project_task_or_prerequisite_is_refused() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;

    let prerequisite = open_task(&app, &user, pid, "Ship the schema").await;
    let dependant = open_task(&app, &user, pid, "Ship the routes").await;
    let id = id_of(&dependant);

    let response = post_dependency(
        &app,
        &user,
        pid,
        &id,
        json!({ "depends_on": id_of(&prerequisite), "kind": "supersedes" }),
    )
    .await;
    assert_error(
        &response,
        StatusCode::BAD_REQUEST,
        "invalid dependency kind",
    );

    // A UUID nobody carries names no task anywhere.
    let response = post_dependency(
        &app,
        &user,
        pid,
        &id,
        json!({ "depends_on": Uuid::new_v4().to_string() }),
    )
    .await;
    assert_error(&response, StatusCode::NOT_FOUND, "not found");

    // An unknown `{id}`, and an unknown project.
    let response = post_dependency(
        &app,
        &user,
        pid,
        "999",
        json!({ "depends_on": id_of(&prerequisite) }),
    )
    .await;
    assert_error(&response, StatusCode::NOT_FOUND, "not found");

    let response = post_dependency(
        &app,
        &user,
        Uuid::new_v4(),
        &id,
        json!({ "depends_on": id_of(&prerequisite) }),
    )
    .await;
    assert_error(&response, StatusCode::NOT_FOUND, "not found");
}

#[tokio::test]
async fn both_dependency_paths_need_a_token() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;
    let task = open_task(&app, &user, pid, "Ship the routes").await;
    let id = id_of(&task);

    let unauthorized = json!({ "status": 401, "error": "authentication required" });

    let response = app
        .server
        .post(&dependencies_path(pid, &id))
        .json(&json!({ "depends_on": "1" }))
        .await;
    response.assert_status(StatusCode::UNAUTHORIZED);
    response.assert_json(&unauthorized);

    let response = app
        .server
        .delete(&format!("{}?kind=blocks", dependency_path(pid, &id, "1")))
        .await;
    response.assert_status(StatusCode::UNAUTHORIZED);
    response.assert_json(&unauthorized);
}

// ---- removing ----

#[tokio::test]
async fn removing_the_blocker_unblocks_and_leaves_the_other_kind_standing() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;

    let prerequisite = open_task(&app, &user, pid, "Ship the schema").await;
    let dependant = open_task(&app, &user, pid, "Ship the routes").await;
    let id = id_of(&dependant);
    let dep = id_of(&prerequisite);

    add_dependency(&app, &user, pid, &id, json!({ "depends_on": dep })).await;
    add_dependency(
        &app,
        &user,
        pid,
        &id,
        json!({ "depends_on": dep, "kind": "discovered_from" }),
    )
    .await;
    let before = event_kinds(&app, pid).await.len();

    let response = delete_dependency(&app, &user, pid, &id, &dep, "?kind=blocks").await;
    response.assert_status(StatusCode::OK);
    let updated = response.json::<Value>();

    assert_eq!(updated["blocked"], json!(false));
    assert_eq!(
        updated["depends_on"],
        json!([{ "task_id": prerequisite["id"], "kind": "discovered_from" }]),
        "only that kind is removed",
    );

    let kinds = event_kinds(&app, pid).await;
    assert_eq!(&kinds[before..], ["dependency_removed", "unblocked"]);
}

#[tokio::test]
async fn removing_a_blocker_whose_prerequisite_is_terminal_flips_nothing() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;

    let prerequisite = task(
        &app,
        &user,
        pid,
        json!({ "title": "Already done", "state": "done" }),
    )
    .await;
    let dependant = open_task(&app, &user, pid, "Ship the routes").await;
    let id = id_of(&dependant);
    let dep = id_of(&prerequisite);

    add_dependency(&app, &user, pid, &id, json!({ "depends_on": dep })).await;
    let before = event_kinds(&app, pid).await.len();

    let response = delete_dependency(&app, &user, pid, &id, &dep, "?kind=blocks").await;
    response.assert_status(StatusCode::OK);
    assert_eq!(response.json::<Value>()["blocked"], json!(false));

    let kinds = event_kinds(&app, pid).await;
    assert_eq!(
        &kinds[before..],
        ["dependency_removed"],
        "the flag was already false, so nothing flipped",
    );
}

#[tokio::test]
async fn a_removal_needs_a_kind_and_refuses_an_edge_that_is_not_there() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;

    let prerequisite = open_task(&app, &user, pid, "Ship the schema").await;
    let dependant = open_task(&app, &user, pid, "Ship the routes").await;
    let id = id_of(&dependant);
    let dep = id_of(&prerequisite);

    add_dependency(&app, &user, pid, &id, json!({ "depends_on": dep })).await;

    let response = delete_dependency(&app, &user, pid, &id, &dep, "").await;
    assert_error(&response, StatusCode::BAD_REQUEST, "kind is required");

    let response = delete_dependency(&app, &user, pid, &id, &dep, "?kind=supersedes").await;
    assert_error(
        &response,
        StatusCode::BAD_REQUEST,
        "invalid dependency kind",
    );

    // The pair is joined by `blocks` alone, so the other two kinds are not
    // there to remove — and the body says so rather than the generic
    // `not found` an unknown task gets.
    let response = delete_dependency(&app, &user, pid, &id, &dep, "?kind=related").await;
    assert_error(&response, StatusCode::NOT_FOUND, "dependency not found");

    // And the edge survived all three refusals.
    let task = read_task(&app, &user, pid, &id).await;
    assert_eq!(
        task["depends_on"],
        json!([{ "task_id": prerequisite["id"], "kind": "blocks" }])
    );
}

/// The three 404s a removal can produce, told apart by their bodies.
///
/// A caller that asked for an edge to be gone has to know whether it was the
/// edge that was absent — in which case its request already holds — or one of
/// the two tasks, which means it named something wrong. The generic `not
/// found` cannot say (`SPEC.md`, "Tasks"; `ARCHITECTURE.md`, "Orchestrator
/// internals").
#[tokio::test]
async fn the_three_removal_404s_have_three_different_bodies() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;

    let prerequisite = open_task(&app, &user, pid, "Ship the schema").await;
    let dependant = open_task(&app, &user, pid, "Ship the routes").await;
    let id = id_of(&dependant);
    let dep = id_of(&prerequisite);
    let unknown = Uuid::new_v4().to_string();

    // Both tasks are real and nothing joins them: the edge is what is missing.
    let response = delete_dependency(&app, &user, pid, &id, &dep, "?kind=blocks").await;
    assert_error(&response, StatusCode::NOT_FOUND, "dependency not found");

    // An unknown `{id}` and an unknown `{dep}` stay generic.
    let response = delete_dependency(&app, &user, pid, &unknown, &dep, "?kind=blocks").await;
    assert_error(&response, StatusCode::NOT_FOUND, "not found");

    let response = delete_dependency(&app, &user, pid, &id, &unknown, "?kind=blocks").await;
    assert_error(&response, StatusCode::NOT_FOUND, "not found");

    // As does an unknown project, which never reaches either lookup.
    let response = delete_dependency(&app, &user, Uuid::new_v4(), &id, &dep, "?kind=blocks").await;
    assert_error(&response, StatusCode::NOT_FOUND, "not found");
}

// ---- concurrency ----

/// Two reciprocal `blocks` edges asked for at once.
///
/// Both requests are launched behind the project lock, so they are provably in
/// flight together and neither could have committed before the other began.
/// Whichever reaches the lock first inserts its edge; the second one's cycle
/// walk then sees it and refuses, because the walk runs under the same lock
/// and after the first transaction committed (ADR 0021).
#[tokio::test]
async fn two_reciprocal_blocks_requests_leave_exactly_one_edge() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;

    let a = open_task(&app, &user, pid, "A").await;
    let b = open_task(&app, &user, pid, "B").await;

    let gate = hold_project_lock(&app.pool, pid).await;
    let a_depends_on_b = app
        .post_as(&user, &dependencies_path(pid, &id_of(&a)))
        .json(&json!({ "depends_on": id_of(&b) }));
    let b_depends_on_a = app
        .post_as(&user, &dependencies_path(pid, &id_of(&b)))
        .json(&json!({ "depends_on": id_of(&a) }));

    let (first, second, ()) = timeout(RACE_TIMEOUT, async {
        tokio::join!(
            a_depends_on_b,
            b_depends_on_a,
            release_when_blocked(&app.pool, gate, 2)
        )
    })
    .await
    .expect("the two edges did not deadlock");

    let statuses = [first.status_code(), second.status_code()];
    assert!(
        statuses.contains(&StatusCode::OK) && statuses.contains(&StatusCode::CONFLICT),
        "expected one 200 and one 409, got {statuses:?}",
    );

    let edges: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM task_dependencies AS d JOIN tasks AS t ON t.id = d.task_id \
         WHERE t.project_id = $1",
    )
    .bind(pid)
    .fetch_one(&app.pool)
    .await
    .expect("the count runs");
    assert_eq!(edges, 1, "a ring of two `blocks` edges was created");

    // Nothing hangs on to the lock after the race.
    timeout(
        Duration::from_secs(5),
        read_task(&app, &user, pid, &id_of(&a)),
    )
    .await
    .expect("the project is not still locked");
}
