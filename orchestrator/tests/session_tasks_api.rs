//! `GET /api/sessions/{id}/tasks` and the end-of-session tracker hook
//! (`SPEC.md`, "Sessions"; `ARCHITECTURE.md`, "Task tracker" → "Liveness comes
//! from the session, not from tool calls").
//!
//! The two halves of closing the loop between a session and the tracker:
//!
//! - the read: every task a session touched — claimed, commented on, handed
//!   back — as full `Task` DTOs, most recently touched first, with the 404 and
//!   the 401 the endpoint owes;
//! - the write: `AppState::session_ended`, the hook every path that makes a
//!   session `done` or `failed` runs, which releases the leases the session
//!   still held, escalates what ran out of attempts, emails the escalation and
//!   does nothing at all the second time it is called.
//!
//! The hook is the one `TestApp` installs, which is the one `main` installs
//! (`tracker::hooks::session_ended_hook`), so what is asserted here is the
//! production wiring rather than a closure this file wrote.
//!
//! What a release *is* — which columns move, which event it writes, what the
//! system comment says — belongs to `tests/tracker_escalation.rs`; this suite
//! asserts that the hook reaches it, with the right reason, from the right
//! side of the session transaction.
//!
//! Every credential-shaped value is an obviously fake stand-in (rule 3).
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use axum::http::StatusCode;
use common::{AuthenticatedUser, TestApp};
use mars_orchestrator::events::{TaskActor, TaskEvent};
use mars_orchestrator::models::{NewSession, NewTask, ProfileKind, SessionState, Task, TaskRef};
use mars_orchestrator::projects::{NewProjectRequest, create_project};
use mars_orchestrator::repositories::{ProjectRepository, SessionRepository, TaskRepository};
use mars_orchestrator::session::SessionService;
use mars_orchestrator::tracker::{
    CommentAuthor, TaskDto, TrackerMutation, add_comment, claim_for_launch,
};
use serde_json::{Value, json};
use uuid::Uuid;

/// Not a real remote: `.invalid` can never resolve (rule 3).
const TEST_REMOTE: &str = "https://example.invalid/org/repo.git";

/// The project default this suite relies on (`SPEC.md`, "Projects").
const MAX_ATTEMPTS: i16 = 3;

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

/// A project with the seeded default states and its `default` profile.
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

/// A session of this project, through its own `default` profile.
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

/// End a session the way the lifecycle leaves the row behind it.
///
/// Written directly: what the transitions themselves do is
/// `tests/session_service.rs`'s and `tests/session_recovery.rs`'s, and what
/// this suite needs is only the row the hook reads its reason off.
async fn mark_session(app: &TestApp, session_id: Uuid, state: &str, error: Option<&str>) {
    sqlx::query(
        "UPDATE sessions SET state = $1::session_state, error = $2, ended_at = NOW() WHERE id = $3",
    )
    .bind(state)
    .bind(error)
    .bind(session_id)
    .execute(&app.pool)
    .await
    .expect("the session row is ended");
}

/// A `ready` task of this project.
async fn ready_task(app: &TestApp, pid: Uuid, title: &str) -> Task {
    let mut new = NewTask::new(pid, title).expect("the title parses");
    new.state_id = Some(
        TaskRepository::new(&app.pool)
            .find_state_by_name(pid, "ready")
            .await
            .expect("the state reads")
            .expect("the project has a ready state")
            .id,
    );

    let mut mutation = TrackerMutation::begin(&app.pool, pid, TaskActor::System)
        .await
        .expect("the mutation opens");
    let inserted = TaskRepository::new(&app.pool)
        .insert_task(mutation.conn(), pid, &new)
        .await
        .expect("the task inserts");
    mutation.commit().await.expect("the mutation commits");

    inserted
}

/// Put this session's lease on the task, the way a launch for a task does.
async fn claim(app: &TestApp, pid: Uuid, task_id: Uuid, session_id: Uuid) -> TaskDto {
    let mut mutation = TrackerMutation::begin(&app.pool, pid, TaskActor::Session { session_id })
        .await
        .expect("the mutation opens");
    let locked = TaskRepository::new(&app.pool)
        .find_task_for_update(mutation.conn(), pid, TaskRef::Id(task_id))
        .await
        .expect("the row reads")
        .expect("the task is in this project");
    let claimed = claim_for_launch(&mut mutation, &locked, session_id)
        .await
        .expect("the claim succeeds");
    mutation.commit().await.expect("the mutation commits");

    claimed
}

/// Comment on a task as the session, which is a touch and nothing more.
async fn comment(app: &TestApp, pid: Uuid, task_id: Uuid, session_id: Uuid, body: &str) {
    let mut mutation = TrackerMutation::begin(&app.pool, pid, TaskActor::Session { session_id })
        .await
        .expect("the mutation opens");
    let locked = TaskRepository::new(&app.pool)
        .find_task_for_update(mutation.conn(), pid, TaskRef::Id(task_id))
        .await
        .expect("the row reads")
        .expect("the task is in this project");
    add_comment(
        &mut mutation,
        &locked,
        CommentAuthor::Session(session_id),
        body,
    )
    .await
    .expect("the comment is written");
    mutation.commit().await.expect("the mutation commits");
}

/// Spend this task's attempts, so that the next release escalates it
/// (`ARCHITECTURE.md`, "Task tracker" → "Attempts and escalation").
async fn spend_attempts(app: &TestApp, task_id: Uuid) {
    sqlx::query("UPDATE tasks SET attempts = $1 WHERE id = $2")
        .bind(MAX_ATTEMPTS)
        .bind(task_id)
        .execute(&app.pool)
        .await
        .expect("the attempts are set");
}

/// The task as it is committed.
async fn read(app: &TestApp, pid: Uuid, task_id: Uuid) -> TaskDto {
    TaskRepository::new(&app.pool)
        .load_task_dto(pid, task_id)
        .await
        .expect("the task reads")
        .expect("the task is in this project")
}

/// This task's events, oldest first.
async fn events_for(app: &TestApp, pid: Uuid, task_id: Uuid) -> Vec<TaskEvent> {
    TaskRepository::new(&app.pool)
        .list_task_events_after(pid, 0, 200)
        .await
        .expect("the events read")
        .into_iter()
        .map(|row| TaskEvent::from_row(row).expect("the row is a documented event"))
        .filter(|event| event.task_id == Some(task_id))
        .collect()
}

/// `/api/sessions/{id}/tasks`.
fn path(session_id: Uuid) -> String {
    format!("/api/sessions/{session_id}/tasks")
}

/// The `title` of each task in a list response, in the order it came back.
fn titles(list: &Value) -> Vec<String> {
    list.as_array()
        .expect("a list")
        .iter()
        .map(|task| task["title"].as_str().expect("a title").to_string())
        .collect()
}

// ---- GET /sessions/{id}/tasks ----

#[tokio::test]
async fn the_list_is_every_task_the_session_touched_most_recently_touched_first() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "reader").await;
    let pid = project(&app, &user, "mars").await;
    let session_id = session(&app, pid).await;

    let first = ready_task(&app, pid, "claimed first").await;
    let second = ready_task(&app, pid, "claimed second").await;
    let third = ready_task(&app, pid, "only commented on").await;

    claim(&app, pid, first.id, session_id).await;
    claim(&app, pid, second.id, session_id).await;
    comment(&app, pid, third.id, session_id, "had a look").await;

    let response = app.get_as(&user, &path(session_id)).await;
    response.assert_status_ok();
    let body = response.json::<Value>();

    assert_eq!(
        titles(&body),
        vec!["only commented on", "claimed second", "claimed first"],
        "ordered by last_touched_at DESC",
    );

    // Full `Task` DTOs, not summaries: the session view's side panel needs no
    // second request per row (`SPEC.md`, "Tasks").
    let newest = &body.as_array().expect("a list")[0];
    assert_eq!(newest["id"], json!(third.id));
    assert_eq!(newest["number"], json!(third.number));
    assert_eq!(newest["state"], json!("ready"));
    assert_eq!(newest["priority"], json!(third.priority as i64));
    assert_eq!(newest["blocked"], json!(false));
    assert_eq!(newest["labels"], json!([]));
    assert_eq!(newest["depends_on"], json!([]));
}

#[tokio::test]
async fn a_session_that_touched_nothing_lists_nothing() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "reader").await;
    let pid = project(&app, &user, "mars").await;
    let session_id = session(&app, pid).await;

    // A task of the same project that this session never touched is not its
    // task: the list is the links, not the project.
    ready_task(&app, pid, "nobody's").await;

    let response = app.get_as(&user, &path(session_id)).await;
    response.assert_status_ok();
    response.assert_json(&json!([]));
}

#[tokio::test]
async fn an_unknown_session_is_not_an_empty_list() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "reader").await;

    let response = app.get_as(&user, &path(Uuid::new_v4())).await;
    response.assert_status(StatusCode::NOT_FOUND);
    response.assert_json(&json!({ "status": 404, "error": "not found" }));
}

#[tokio::test]
async fn the_list_needs_a_token() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "reader").await;
    let pid = project(&app, &user, "mars").await;
    let session_id = session(&app, pid).await;

    let response = app.server.get(&path(session_id)).await;
    response.assert_status(StatusCode::UNAUTHORIZED);
    response.assert_json(&json!({ "status": 401, "error": "authentication required" }));
}

// ---- the end-of-session hook ----

#[tokio::test]
async fn ending_a_session_releases_what_it_held_and_leaves_the_rest_alone() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "reader").await;
    let pid = project(&app, &user, "mars").await;
    let session_id = session(&app, pid).await;

    let held = ready_task(&app, pid, "held").await;
    let also_held = ready_task(&app, pid, "also held").await;
    let touched = ready_task(&app, pid, "only commented on").await;

    claim(&app, pid, held.id, session_id).await;
    claim(&app, pid, also_held.id, session_id).await;
    comment(&app, pid, touched.id, session_id, "had a look").await;
    let untouched_events = events_for(&app, pid, touched.id).await.len();

    mark_session(&app, session_id, "done", None).await;
    app.state.session_ended(session_id).await;

    for task in [&held, &also_held] {
        let stored = read(&app, pid, task.id).await;
        assert!(
            stored.lease_holder_session_id.is_none(),
            "{} keeps its lease",
            task.title,
        );
        assert_eq!(stored.state, "ready", "{} left its queue", task.title);

        let written = events_for(&app, pid, task.id).await;
        let last = written.last().expect("the release is recorded");
        assert_eq!(last.kind.as_str(), "released");
        assert_eq!(last.reason.as_deref(), Some("session_ended"));
        assert_eq!(last.actor, TaskActor::System);

        let system = TaskRepository::new(&app.pool)
            .list_comments(pid, task.id)
            .await
            .expect("the comments read")
            .into_iter()
            .filter(|comment| comment.system)
            .collect::<Vec<_>>();
        assert_eq!(system.len(), 1, "one system comment per released task");
        assert_eq!(
            system[0].body,
            format!("Lease released by the orchestrator: holder session {session_id} ended."),
        );
    }

    // The task this session only commented on was never held, so nothing
    // happened to it.
    assert_eq!(
        events_for(&app, pid, touched.id).await.len(),
        untouched_events,
    );

    // And the link survives the release: the session still worked on all three
    // (ADR 0030).
    let response = app.get_as(&user, &path(session_id)).await;
    response.assert_status_ok();
    assert_eq!(titles(&response.json::<Value>()).len(), 3);
}

#[tokio::test]
async fn ending_a_parked_session_through_the_service_releases_its_lease() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "reader").await;
    let pid = project(&app, &user, "mars").await;
    let session_id = session(&app, pid).await;

    let held = ready_task(&app, pid, "held").await;
    claim(&app, pid, held.id, session_id).await;

    // `end` is allowed from `parked`, and a parked session has no container to
    // discard, so this is the UI's own path with nothing mocked around it
    // (`SPEC.md`, "Sessions", `POST /sessions/{id}/end`). The branch fetch it
    // attempts is recorded and never fatal, which is the contract
    // `tests/session_service.rs` asserts.
    sqlx::query("UPDATE sessions SET state = 'parked', parked_at = NOW() WHERE id = $1")
        .bind(session_id)
        .execute(&app.pool)
        .await
        .expect("the session parks");

    let ended = SessionService::new(&app.state)
        .end(session_id)
        .await
        .expect("the session ends");
    assert_eq!(ended.state, SessionState::Done);

    let stored = read(&app, pid, held.id).await;
    assert!(
        stored.lease_holder_session_id.is_none(),
        "ending a session from the UI releases its leases immediately",
    );
}

#[tokio::test]
async fn the_hook_run_twice_releases_nothing_the_second_time() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "reader").await;
    let pid = project(&app, &user, "mars").await;
    let session_id = session(&app, pid).await;

    let held = ready_task(&app, pid, "held").await;
    claim(&app, pid, held.id, session_id).await;

    mark_session(&app, session_id, "done", None).await;
    app.state.session_ended(session_id).await;
    let after_first = events_for(&app, pid, held.id).await.len();

    app.state.session_ended(session_id).await;

    assert_eq!(
        events_for(&app, pid, held.id).await.len(),
        after_first,
        "the locked re-read finds no held task the second time",
    );
}

#[tokio::test]
async fn a_stalled_session_releases_with_the_stalled_reason() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "reader").await;
    let pid = project(&app, &user, "mars").await;
    let session_id = session(&app, pid).await;

    let held = ready_task(&app, pid, "held").await;
    claim(&app, pid, held.id, session_id).await;

    // What the idle reaper leaves behind (`ARCHITECTURE.md`, "Session
    // lifecycle"): `failed` with `error = "stalled"`.
    mark_session(&app, session_id, "failed", Some("stalled")).await;
    app.state.session_ended(session_id).await;

    let written = events_for(&app, pid, held.id).await;
    let last = written.last().expect("the release is recorded");
    assert_eq!(last.kind.as_str(), "released");
    assert_eq!(last.reason.as_deref(), Some("stalled"));

    let system = TaskRepository::new(&app.pool)
        .list_comments(pid, held.id)
        .await
        .expect("the comments read")
        .into_iter()
        .find(|comment| comment.system)
        .expect("a system comment is written");
    assert_eq!(
        system.body,
        format!("Lease released by the orchestrator: holder session {session_id} stalled."),
    );
}

#[tokio::test]
async fn a_failure_that_is_not_stalled_is_an_ordinary_session_ended() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "reader").await;
    let pid = project(&app, &user, "mars").await;
    let session_id = session(&app, pid).await;

    let held = ready_task(&app, pid, "held").await;
    claim(&app, pid, held.id, session_id).await;

    // A launch that failed in `creating` is the case the launch-for-task route
    // relies on: the task it claimed goes back to its queue.
    mark_session(&app, session_id, "failed", Some("image pull failed")).await;
    app.state.session_ended(session_id).await;

    let stored = read(&app, pid, held.id).await;
    assert!(stored.lease_holder_session_id.is_none());

    let written = events_for(&app, pid, held.id).await;
    assert_eq!(
        written.last().expect("the release is recorded").reason,
        Some("session_ended".to_string()),
    );
}

#[tokio::test]
async fn a_task_out_of_attempts_is_escalated_and_emailed_when_its_session_ends() {
    let app = TestApp::spawn().await;
    let admin = app
        .create_admin("keeper", "keeper@example.test", &password("keeper"))
        .await;
    let pid = project(&app, &admin, "mars").await;
    let session_id = session(&app, pid).await;

    let exhausted = ready_task(&app, pid, "out of attempts").await;
    claim(&app, pid, exhausted.id, session_id).await;
    spend_attempts(&app, exhausted.id).await;

    mark_session(&app, session_id, "done", None).await;
    app.state.session_ended(session_id).await;

    let stored = read(&app, pid, exhausted.id).await;
    assert_eq!(stored.state, "needs_human");
    assert!(stored.lease_holder_session_id.is_none());
    assert!(
        stored
            .needs_human_reason
            .as_deref()
            .expect("a reason is recorded")
            .contains("attempt limit reached"),
    );

    let kinds = events_for(&app, pid, exhausted.id)
        .await
        .into_iter()
        .map(|event| event.kind.as_str().to_string())
        .collect::<Vec<_>>();
    assert!(
        kinds.contains(&"escalated".to_string()),
        "the escalation is announced: {kinds:?}",
    );

    // The unassigned escalation goes to the administrators, and the hook is
    // what sends it — after the tracker transaction committed.
    let sent = app.mock_email().sent();
    assert_eq!(sent.len(), 1, "exactly one escalation email: {sent:?}");
    assert_eq!(sent[0].to, admin.user.email);
}
