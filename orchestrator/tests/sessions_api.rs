//! `/api/projects/{pid}/sessions` and `/api/sessions` through the real router
//! (`SPEC.md`, "Sessions").
//!
//! The resource endpoints of the sessions table: the two lists, the create, the
//! read, the retitle and the event page. What the *rules* are is asserted
//! against the models in `src/models/session.rs` and against a real database in
//! `tests/repositories_sessions.rs`; what is asserted here is the adapter — the
//! paths, the JWT requirement on each of them, the status and body of every
//! success and of every documented failure, and that `POST` writes the row and
//! starts the launch the launch sequence expects.
//!
//! The project every scenario hangs its sessions on is created through
//! `POST /api/projects` against a real bare repository over the `file://` form
//! the `integration-tests` feature accepts, and waited on until its clone job
//! has made it `ready`: git is never mocked, and a base ref that resolves is
//! the whole point of half of these assertions. The engine is `MockEngine`, so
//! a launch records the container it would have created instead of creating one.
//!
//! Sessions the read-only endpoints need are inserted through
//! [`SessionRepository`] rather than posted, so that a list, a retitle or an
//! event page costs no launch. The `POST` scenarios launch for real.
//!
//! The action verbs — `input`, `stop`, `end`, `retry`, `sync` — and `DELETE`
//! are here too, asserted the same way: what the *rules* are is asserted
//! against the real [`SessionService`] in `tests/session_service.rs`, and what
//! is asserted here is the adapter — the path, the token, the status, the body
//! and each documented refusal. The scenarios that need a live session or a
//! work tree launch one for real through `POST`, so the fetch-back and the
//! resume have the git objects they need.
//!
//! Every credential-shaped value is an obviously fake stand-in (rule 3).
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use axum_test::TestResponse;
use common::{AuthenticatedUser, TestApp};
use mars_orchestrator::engine::{ContainerId, Signal};
use mars_orchestrator::events::{TaskActor, TaskEvent};
use mars_orchestrator::git::DataPaths;
use mars_orchestrator::git::testutil::{TestUpstream, run_git};
use mars_orchestrator::models::{
    HandoffCaller, NewEvent, NewSession, NewTask, ProfileKind, ProjectStatus, Session,
    SessionLaunchSource, SessionState, StateChange, Task, TaskRef,
};
use mars_orchestrator::projects::clone_job;
use mars_orchestrator::repositories::{SessionRepository, TaskRepository};
use mars_orchestrator::session::{McpToken, Phase, SessionDirs};
use mars_orchestrator::tracker::{ReviewCarry, TaskDto, TrackerMutation, claim_for_launch};
use serde_json::{Value, json};
use uuid::Uuid;

/// How long a clone, a launch or an owner's first write may take before the
/// scenario gives up with a message rather than hanging.
const PATIENCE: Duration = Duration::from_secs(30);

/// How often the polls re-read what they are waiting for.
const POLL: Duration = Duration::from_millis(25);

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

/// The `file://` URL of a local bare repository, which is the remote form
/// `RemoteUrl::parse` accepts under the `integration-tests` feature.
fn file_url(upstream: &TestUpstream) -> String {
    format!("file://{}", upstream.path.display())
}

/// `/api/projects/{pid}/sessions`.
fn sessions_path(pid: Uuid) -> String {
    format!("/api/projects/{pid}/sessions")
}

/// `/api/sessions/{id}`.
fn session_path(id: Uuid) -> String {
    format!("/api/sessions/{id}")
}

/// `/api/sessions/{id}/events`.
fn events_path(id: Uuid) -> String {
    format!("/api/sessions/{id}/events")
}

/// `/api/sessions/{id}/<verb>`, one of the five action routes.
fn action_path(id: Uuid, verb: &str) -> String {
    format!("/api/sessions/{id}/{verb}")
}

/// A project cloned from `upstream` and waited on until it is `ready`, with the
/// seeded default profile.
///
/// Created through `POST /api/projects` so it is seeded exactly as a real one
/// is: the default task states and the `default` profile the sessions here
/// launch with.
struct Fixture {
    upstream: TestUpstream,
    user: AuthenticatedUser,
    project_id: Uuid,
    profile_id: Uuid,
}

impl Fixture {
    /// A signed-in user, a `ready` project and its default profile.
    async fn create(app: &TestApp) -> Self {
        let upstream = TestUpstream::create().await;
        let user = signed_in(app, &format!("user-{}", suffix())).await;
        let project_id = ready_project(app, &user, &upstream).await;
        let profile_id = default_profile(app, &user, project_id).await;

        Self {
            upstream,
            user,
            project_id,
            profile_id,
        }
    }

    /// `POST /api/projects/{pid}/sessions` with `body`, as the fixture's user.
    async fn post(&self, app: &TestApp, body: &Value) -> TestResponse {
        app.post_as(&self.user, &sessions_path(self.project_id))
            .json(body)
            .await
    }

    /// The same, asserting 201 and answering the created session.
    async fn create_session(&self, app: &TestApp, body: &Value) -> Value {
        let response = self.post(app, body).await;
        response.assert_status(StatusCode::CREATED);

        response.json::<Value>()
    }

    /// An ephemeral profile of this project, for the launch-prompt rule.
    async fn ephemeral_profile(&self, app: &TestApp) -> Uuid {
        let response = app
            .post_as(
                &self.user,
                &format!("/api/projects/{}/profiles", self.project_id),
            )
            .json(&json!({ "name": "one-shot", "kind": "ephemeral" }))
            .await;
        response.assert_status(StatusCode::CREATED);

        id_of(&response.json::<Value>())
    }
}

/// Eight hex characters, so parallel scenarios never collide on a user name.
fn suffix() -> String {
    Uuid::new_v4().simple().to_string()[..8].to_string()
}

/// A `ready` project over `upstream`, created and cloned through its own
/// endpoints.
async fn ready_project(app: &TestApp, user: &AuthenticatedUser, upstream: &TestUpstream) -> Uuid {
    let response = app
        .post_as(user, "/api/projects")
        .json(&json!({
            "name": format!("project-{}", suffix()),
            "remote_url": file_url(upstream),
            "default_branch": "main",
        }))
        .await;
    response.assert_status(StatusCode::CREATED);
    let project_id = id_of(&response.json::<Value>());

    let cloned = clone_job::wait_for_clone(&app.state, project_id, PATIENCE).await;
    assert_eq!(
        cloned.status,
        ProjectStatus::Ready,
        "the fixture clone failed: {:?}",
        cloned.status_message,
    );

    project_id
}

/// The id of the project's seeded `default` profile.
async fn default_profile(app: &TestApp, user: &AuthenticatedUser, pid: Uuid) -> Uuid {
    let response = app
        .get_as(user, &format!("/api/projects/{pid}/profiles"))
        .await;
    response.assert_status_ok();

    let profiles = response.json::<Vec<Value>>();
    let default = profiles
        .iter()
        .find(|profile| profile["is_default"] == json!(true))
        .expect("a project is seeded with a default profile");
    assert_eq!(default["kind"], json!("conversational"));

    id_of(default)
}

/// The `id` of a resource body, as a [`Uuid`].
fn id_of(body: &Value) -> Uuid {
    body["id"]
        .as_str()
        .expect("a resource carries an id")
        .parse()
        .expect("the id is a uuid")
}

/// A session row, inserted the way `POST` inserts one but with no launch.
///
/// What every read-only scenario uses: the endpoints under test read rows, and
/// a launch would only add a container and a clone to wait for.
async fn seed_session(
    app: &TestApp,
    fixture: &Fixture,
    state: SessionState,
    title: Option<&str>,
) -> Session {
    seed_session_of_kind(app, fixture, ProfileKind::Conversational, state, title).await
}

/// The same for a session of either kind.
///
/// `kind` is the session row's, which is what every lifecycle rule reads; the
/// profile it points at stays the fixture's, because nothing under test here
/// looks the profile up (`docs/data-model.md`, `sessions.kind`).
async fn seed_session_of_kind(
    app: &TestApp,
    fixture: &Fixture,
    kind: ProfileKind,
    state: SessionState,
    title: Option<&str>,
) -> Session {
    let mut new_session = NewSession::new(
        fixture.project_id,
        fixture.profile_id,
        kind,
        "main",
        McpToken::generate().hash(),
    );
    new_session.created_by = Some(fixture.user.user.id);
    let new_session = new_session
        .with_title(title)
        .expect("the fixture title is valid");

    let sessions = SessionRepository::new(&app.pool);
    let mut tx = app.pool.begin().await.expect("a transaction begins");
    let inserted = sessions
        .insert(&mut tx, &new_session)
        .await
        .expect("the session inserts");
    tx.commit().await.expect("the transaction commits");

    if state == SessionState::Creating {
        return inserted;
    }

    // The lifecycle diagram's own path to the state the scenario wants
    // (`ARCHITECTURE.md`, "Session lifecycle").
    let path: &[SessionState] = match state {
        SessionState::Running => &[SessionState::Running],
        SessionState::Parked => &[SessionState::Running, SessionState::Parked],
        SessionState::Done => &[SessionState::Running, SessionState::Done],
        SessionState::Failed => &[SessionState::Failed],
        SessionState::Creating => &[],
    };

    let mut current = inserted;
    for target in path {
        // A `failed` fixture carries an error, so that a retry clearing it is
        // something a scenario can see (`docs/data-model.md`, `sessions.error`).
        let change = if *target == SessionState::Failed {
            StateChange::failed("arranged by the test")
        } else {
            StateChange::plain()
        };

        let mut tx = app.pool.begin().await.expect("a transaction begins");
        current = sessions
            .set_state(&mut tx, current.id, *target, &change)
            .await
            .expect("the fixture transition is legal");
        tx.commit().await.expect("the transaction commits");
    }

    current
}

/// Append `count` `text` events to `session_id`, each carrying an `_offset`.
///
/// The offset is the internal field `SPEC.md`, "AgentEvent" says never reaches
/// a client, so seeding one is what makes the stripping assertion mean
/// something.
async fn seed_events(app: &TestApp, session_id: Uuid, count: usize) {
    let events: Vec<NewEvent> = (1..=count)
        .map(|n| {
            NewEvent::now(
                "text",
                json!({ "text": format!("event {n}"), "_offset": n * 100 }),
            )
        })
        .collect();

    let mut tx = app.pool.begin().await.expect("a transaction begins");
    SessionRepository::new(&app.pool)
        .append_events(&mut tx, session_id, &events)
        .await
        .expect("the events append");
    tx.commit().await.expect("the transaction commits");
}

/// The session row as it is now.
async fn reload(app: &TestApp, id: Uuid) -> Session {
    SessionRepository::new(&app.pool)
        .get(id)
        .await
        .expect("the session is readable")
}

/// Wait until `predicate` holds, or give up with `what`.
async fn wait_for(what: &str, mut predicate: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + PATIENCE;

    while !predicate() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "{what} did not happen within {PATIENCE:?}",
        );
        tokio::time::sleep(POLL).await;
    }
}

/// Wait until the session has an event of `kind`, and answer its payload.
async fn wait_for_event(app: &TestApp, session_id: Uuid, kind: &str) -> Value {
    let deadline = tokio::time::Instant::now() + PATIENCE;

    loop {
        let payload = sqlx::query_scalar::<_, Value>(
            "SELECT payload FROM events WHERE session_id = $1 AND kind = $2 ORDER BY seq LIMIT 1",
        )
        .bind(session_id)
        .bind(kind)
        .fetch_optional(&app.pool)
        .await
        .expect("the events are readable");

        if let Some(payload) = payload {
            return payload;
        }

        assert!(
            tokio::time::Instant::now() < deadline,
            "no {kind} event within {PATIENCE:?}",
        );
        tokio::time::sleep(POLL).await;
    }
}

/// A session launched for real through `POST`, waited on until it is `running`,
/// with the container the launch created.
///
/// What every scenario that needs a live session, a work tree or a branch to
/// publish uses: the launcher's own clone and container, over `MockEngine`.
async fn launched_session(app: &TestApp, fixture: &Fixture) -> (Uuid, ContainerId) {
    let body = fixture
        .create_session(app, &json!({ "profile_id": fixture.profile_id }))
        .await;
    let id = id_of(&body);

    let deadline = tokio::time::Instant::now() + PATIENCE;
    loop {
        let session = reload(app, id).await;
        if session.state == SessionState::Running {
            let container_id = session
                .container_id
                .clone()
                .expect("a running session records its container");
            return (id, ContainerId(container_id));
        }

        assert!(
            tokio::time::Instant::now() < deadline,
            "the session did not reach running within {PATIENCE:?}: {:?} {:?}",
            session.state,
            session.error,
        );
        tokio::time::sleep(POLL).await;
    }
}

/// Record a `cli_session_id`, the way the owner's first `init` event does.
///
/// A resume only passes `--resume` for a session that has one (ADR 0032), so a
/// scenario asserting the relaunch arranges it.
async fn record_cli_session_id(app: &TestApp, session_id: Uuid, cli_session_id: &str) {
    let mut tx = app.pool.begin().await.expect("a transaction begins");
    SessionRepository::new(&app.pool)
        .set_cli_session_id(&mut tx, session_id, cli_session_id)
        .await
        .expect("the CLI session id is recorded");
    tx.commit().await.expect("the transaction commits");
}

/// Let the container exit 0 as soon as the stop reaches it as a `SIGINT`.
///
/// The mock container runs no CLI, so nothing would answer the stop and `end`
/// would wait out its whole grace period. The handle answers whether the signal
/// arrived, which is the assertion the stop scenarios make
/// (`ARCHITECTURE.md`, "Stop semantics").
fn exit_on_sigint(app: &TestApp, container_id: &ContainerId) -> tokio::task::JoinHandle<bool> {
    let engine = Arc::clone(&app.engine);
    let container_id = container_id.clone();

    tokio::spawn(async move {
        let deadline = tokio::time::Instant::now() + PATIENCE;
        while tokio::time::Instant::now() < deadline {
            if engine.signals(&container_id).contains(&Signal::Sigint) {
                engine.exit(&container_id, 0);
                return true;
            }
            tokio::time::sleep(POLL).await;
        }
        false
    })
}

/// The commit `refs/sessions/<sid>` points at in the project repository, or
/// `None` when the mirror has no such ref.
async fn mirror_session_ref(app: &TestApp, fixture: &Fixture, session_id: Uuid) -> Option<String> {
    let repo = DataPaths::from_config(&app.state.config).project_repo(fixture.project_id);
    let listed = run_git(
        &repo,
        &[
            "for-each-ref",
            "--format=%(objectname)",
            &format!("refs/sessions/{session_id}"),
        ],
    )
    .await;

    let commit = listed.trim().to_string();
    (!commit.is_empty()).then_some(commit)
}

// ---- create ----

#[tokio::test]
async fn a_create_stores_the_documented_row_and_starts_the_launch() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;

    let body = fixture
        .create_session(&app, &json!({ "profile_id": fixture.profile_id }))
        .await;
    let id = id_of(&body);

    // The response: the documented `Session`, `creating`, with no hash in it.
    assert_eq!(body["project_id"], json!(fixture.project_id));
    assert_eq!(body["profile_id"], json!(fixture.profile_id));
    assert_eq!(body["kind"], json!("conversational"));
    assert_eq!(body["created_by"], json!(fixture.user.user.id));
    // A person pressed the button, which is what `launch_source` records
    // (`SPEC.md`, "Sessions"; ADR 0042).
    assert_eq!(body["launch_source"], json!("user"));
    assert_eq!(body["state"], json!("creating"));
    assert_eq!(body["base_ref"], json!("main"));
    assert_eq!(body["branch"], json!(format!("session/{id}")));
    assert_eq!(body["title"], Value::Null);
    assert_eq!(body["task_id"], Value::Null);
    assert_eq!(body["handoff_id"], Value::Null);
    assert!(
        body.get("mcp_token_hash").is_none(),
        "the token hash reached a client: {body}",
    );

    // The row: the same, plus the hash the launcher's token has to match.
    let row = reload(&app, id).await;
    assert_eq!(row.branch, format!("session/{id}"));
    assert_eq!(row.kind, ProfileKind::Conversational);
    assert_eq!(row.created_by, Some(fixture.user.user.id));
    assert_eq!(row.launch_source, SessionLaunchSource::User);
    assert_eq!(row.base_ref, "main");
    assert!(!row.mcp_token_hash.is_empty(), "no token hash was stored");

    // The launch the response did not wait for: one container, after the fact.
    wait_for("the launch created a container", || {
        !app.engine().specs().is_empty()
    })
    .await;
    assert_eq!(
        app.engine().specs().len(),
        1,
        "one create per launch, and only after the response",
    );
    assert!(
        app.engine().container_id_for_session(id).is_some(),
        "the container the engine recorded is not this session's",
    );
}

#[tokio::test]
async fn a_create_derives_its_title_from_the_first_message() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    let long_line = "n".repeat(120);

    // An explicit title is kept as it is.
    let explicit = fixture
        .create_session(
            &app,
            &json!({
                "profile_id": fixture.profile_id,
                "title": "  Rename the widget  ",
                "message": "Fix login\nand the rest",
            }),
        )
        .await;
    assert_eq!(explicit["title"], json!("Rename the widget"));

    // Otherwise the first line of the message, not the first non-empty one.
    let derived = fixture
        .create_session(
            &app,
            &json!({
                "profile_id": fixture.profile_id,
                "message": "Fix login\nand the rest",
            }),
        )
        .await;
    assert_eq!(derived["title"], json!("Fix login"));

    // Truncated to 80 characters (`SPEC.md`, "Sessions").
    let truncated = fixture
        .create_session(
            &app,
            &json!({
                "profile_id": fixture.profile_id,
                "message": long_line,
            }),
        )
        .await;
    assert_eq!(truncated["title"], json!("n".repeat(80)));

    // A title of 201 characters is a 400, and nothing is created.
    let before = SessionRepository::new(&app.pool)
        .list_by_project(fixture.project_id, None)
        .await
        .expect("the list runs")
        .len();
    let response = fixture
        .post(
            &app,
            &json!({ "profile_id": fixture.profile_id, "title": "t".repeat(201) }),
        )
        .await;
    assert_error(
        &response,
        StatusCode::BAD_REQUEST,
        "title must be 1 to 200 characters",
    );
    assert_eq!(
        SessionRepository::new(&app.pool)
            .list_by_project(fixture.project_id, None)
            .await
            .expect("the list runs")
            .len(),
        before,
        "a refused create wrote a row",
    );
}

#[tokio::test]
async fn the_first_message_of_a_conversational_session_is_its_first_user_message() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;

    let body = fixture
        .create_session(
            &app,
            &json!({
                "profile_id": fixture.profile_id,
                "message": "Have a look at the login form",
            }),
        )
        .await;
    let id = id_of(&body);

    // Queued by the route before the launch, delivered by the owner once stdin
    // is attached, and attributed to the caller (`ARCHITECTURE.md`, "Session
    // owner task").
    let payload = wait_for_event(&app, id, "user_message").await;
    assert_eq!(payload["text"], json!("Have a look at the login form"));
    assert_eq!(payload["user_id"], json!(fixture.user.user.id));
}

#[tokio::test]
async fn a_create_answers_the_documented_refusals() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;

    // Unauthenticated, before anything else.
    let response = app
        .server
        .post(&sessions_path(fixture.project_id))
        .json(&json!({ "profile_id": fixture.profile_id }))
        .await;
    response.assert_status(StatusCode::UNAUTHORIZED);
    response.assert_json(&unauthorized());

    // An unknown project.
    let response = app
        .post_as(&fixture.user, &sessions_path(Uuid::new_v4()))
        .json(&json!({ "profile_id": fixture.profile_id }))
        .await;
    assert_error(&response, StatusCode::NOT_FOUND, "not found");

    // A project that is still cloning: created against a remote that cannot
    // resolve, so its job has not answered.
    let cloning = app
        .post_as(&fixture.user, "/api/projects")
        .json(&json!({
            "name": format!("cloning-{}", suffix()),
            "remote_url": "https://example.invalid/org/repo.git",
        }))
        .await;
    cloning.assert_status(StatusCode::CREATED);
    let cloning_id = id_of(&cloning.json::<Value>());
    let response = app
        .post_as(&fixture.user, &sessions_path(cloning_id))
        .json(&json!({ "profile_id": fixture.profile_id }))
        .await;
    assert_error(&response, StatusCode::CONFLICT, "project is not ready");

    // A profile of another project, and one that does not exist at all.
    let foreign = default_profile(&app, &fixture.user, cloning_id).await;
    for profile_id in [foreign, Uuid::new_v4()] {
        let response = fixture
            .post(&app, &json!({ "profile_id": profile_id }))
            .await;
        assert_error(&response, StatusCode::BAD_REQUEST, "unknown profile");
    }

    // A base ref the mirror does not have.
    let response = fixture
        .post(
            &app,
            &json!({ "profile_id": fixture.profile_id, "base_ref": "no-such-branch" }),
        )
        .await;
    assert_error(
        &response,
        StatusCode::BAD_REQUEST,
        "base_ref does not resolve",
    );

    // An ephemeral profile with nothing to run.
    let ephemeral = fixture.ephemeral_profile(&app).await;
    let response = fixture
        .post(&app, &json!({ "profile_id": ephemeral }))
        .await;
    assert_error(
        &response,
        StatusCode::BAD_REQUEST,
        "an ephemeral session needs a task_id or a message",
    );

    // The same profile with a message is accepted.
    let created = fixture
        .create_session(
            &app,
            &json!({ "profile_id": ephemeral, "message": "run the tests" }),
        )
        .await;
    assert_eq!(created["kind"], json!("ephemeral"));
    assert_eq!(created["title"], json!("run the tests"));
}

#[tokio::test]
async fn a_create_takes_every_documented_kind_of_base_ref() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;

    fixture.upstream.tag("v1.0.0", "main", true).await;
    app.post_as(
        &fixture.user,
        &format!("/api/projects/{}/fetch", fixture.project_id),
    )
    .await
    .assert_status_ok();

    let commit = run_git(&fixture.upstream.path, &["rev-parse", "refs/heads/main"])
        .await
        .trim()
        .to_string();

    // An integration head, an upstream-tracking ref, a tag and a commit id
    // (`SPEC.md`, "Projects").
    for base_ref in ["main", "origin/main", "v1.0.0", commit.as_str()] {
        let body = fixture
            .create_session(
                &app,
                &json!({ "profile_id": fixture.profile_id, "base_ref": base_ref }),
            )
            .await;

        assert_eq!(body["base_ref"], json!(base_ref), "{base_ref}");
        assert_eq!(body["state"], json!("creating"), "{base_ref}");
    }
}

// ---- launch for a task ----
//
// `POST /projects/{pid}/sessions` with `task_id` (`SPEC.md`, "Sessions";
// `ARCHITECTURE.md`, "Task tracker" → "Launching a session for a task"). What
// a claim or a release *is* belongs to `tests/tracker_leases.rs` and
// `tests/session_tasks_api.rs`; what is asserted here is the launch: the row
// and the claim committing together, the hand-off the base comes from, the
// generated message and the container environment.

/// A task of this project in the named state.
async fn task_in(app: &TestApp, fixture: &Fixture, title: &str, state: &str) -> Task {
    let tasks = TaskRepository::new(&app.pool);
    let mut new = NewTask::new(fixture.project_id, title).expect("the title parses");
    new.state_id = Some(
        tasks
            .find_state_by_name(fixture.project_id, state)
            .await
            .expect("the state reads")
            .unwrap_or_else(|| panic!("a project is seeded with a {state} state"))
            .id,
    );

    let mut mutation = TrackerMutation::begin(&app.pool, fixture.project_id, TaskActor::System)
        .await
        .expect("the mutation opens");
    let inserted = tasks
        .insert_task(mutation.conn(), fixture.project_id, &new)
        .await
        .expect("the task inserts");
    mutation.commit().await.expect("the mutation commits");

    inserted
}

/// A task of this project in its `ready` queue state.
async fn ready_task(app: &TestApp, fixture: &Fixture, title: &str) -> Task {
    task_in(app, fixture, title, "ready").await
}

/// The task as it is committed, as the API sends it.
async fn read_task(app: &TestApp, fixture: &Fixture, task_id: Uuid) -> TaskDto {
    TaskRepository::new(&app.pool)
        .load_task_dto(fixture.project_id, task_id)
        .await
        .expect("the task reads")
        .expect("the task is in this project")
}

/// This task's events, oldest first.
async fn task_events(app: &TestApp, fixture: &Fixture, task_id: Uuid) -> Vec<TaskEvent> {
    TaskRepository::new(&app.pool)
        .list_task_events_after(fixture.project_id, 0, 200)
        .await
        .expect("the events read")
        .into_iter()
        .map(|row| TaskEvent::from_row(row).expect("the row is a documented event"))
        .filter(|event| event.task_id == Some(task_id))
        .collect()
}

/// Put another session's lease on the task, so the next claim loses.
async fn hold(app: &TestApp, fixture: &Fixture, task_id: Uuid, session_id: Uuid) {
    let mut mutation = TrackerMutation::begin(
        &app.pool,
        fixture.project_id,
        TaskActor::Session { session_id },
    )
    .await
    .expect("the mutation opens");
    let locked = TaskRepository::new(&app.pool)
        .find_task_for_update(mutation.conn(), fixture.project_id, TaskRef::Id(task_id))
        .await
        .expect("the row reads")
        .expect("the task is in this project");
    claim_for_launch(&mut mutation, &locked, session_id)
        .await
        .expect("the arranged claim succeeds");
    mutation.commit().await.expect("the mutation commits");
}

/// Mark the task blocked, the way an open `blocks` prerequisite does: with
/// one (`common::tracker::block`).
async fn block(app: &TestApp, fixture: &Fixture, task_id: Uuid) {
    common::tracker::block(&app.pool, fixture.project_id, task_id).await;
}

/// Publish a hand-off of `commit` on this task and make it the current one.
///
/// Through the verb that publishes one in production: the source session takes
/// the lease, hands its work off and the task comes back to the column it was
/// in (`common::tracker::handoff_in_place`). The lease is gone again
/// afterwards, which is what a task waiting for the next worker looks like.
async fn publish_handoff(
    app: &TestApp,
    fixture: &Fixture,
    task: &Task,
    source_session_id: Uuid,
    commit: &str,
    body: &str,
) -> Uuid {
    common::tracker::hold(&app.pool, fixture.project_id, task.id, source_session_id).await;

    let (_, handoff_id) = common::tracker::handoff_in_place(
        &app.pool,
        fixture.project_id,
        task.id,
        common::tracker::Handoff {
            source_session_id: Some(source_session_id),
            source_branch: "session/source",
            commit,
            comment: body,
            target_state: "",
            caller: HandoffCaller::Session {
                session_id: source_session_id,
            },
            review: ReviewCarry::Fresh,
        },
    )
    .await;

    handoff_id
}

/// The first paragraph of the generated message, for `task`.
fn generated_for(task: &Task) -> String {
    format!(
        "You hold task #{}: {}. Call get_task to read it before starting.",
        task.number, task.title,
    )
}

/// The commit `main` points at in the upstream, which is in the mirror.
async fn upstream_head(fixture: &Fixture) -> String {
    run_git(&fixture.upstream.path, &["rev-parse", "refs/heads/main"])
        .await
        .trim()
        .to_string()
}

/// Wait until the session reaches `state`, and answer the row.
async fn wait_for_state(app: &TestApp, id: Uuid, state: SessionState) -> Session {
    let deadline = tokio::time::Instant::now() + PATIENCE;

    loop {
        let session = reload(app, id).await;
        if session.state == state {
            return session;
        }

        assert!(
            tokio::time::Instant::now() < deadline,
            "the session did not reach {state} within {PATIENCE:?}: {:?} {:?}",
            session.state,
            session.error,
        );
        tokio::time::sleep(POLL).await;
    }
}

/// The recorded container specification of this session's launch.
async fn recorded_env(app: &TestApp, id: Uuid) -> Vec<(String, String)> {
    wait_for("the launch created a container", || {
        app.engine().container_id_for_session(id).is_some()
    })
    .await;

    let container_id = app
        .engine()
        .container_id_for_session(id)
        .expect("the container was recorded");

    app.engine()
        .spec_of(&container_id)
        .expect("the specification was recorded")
        .env
}

#[tokio::test]
async fn a_launch_for_a_task_claims_it_with_the_session_row() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    let task = ready_task(&app, &fixture, "Fix the login form").await;

    let body = fixture
        .create_session(
            &app,
            &json!({ "profile_id": fixture.profile_id, "task_id": task.id }),
        )
        .await;
    let id = id_of(&body);

    // The row records the task, and the title defaults to the task's.
    assert_eq!(body["task_id"], json!(task.id));
    assert_eq!(body["handoff_id"], Value::Null);
    assert_eq!(body["title"], json!("Fix the login form"));
    assert_eq!(body["base_ref"], json!("main"));
    assert_eq!(reload(&app, id).await.task_id, Some(task.id));

    // The claim committed with it: the lease, the attempt, the event and the
    // link (`docs/data-model.md`, `tasks`, `task_sessions`).
    let claimed = read_task(&app, &fixture, task.id).await;
    assert_eq!(claimed.lease_holder_session_id, Some(id));
    assert_eq!(claimed.attempts, 1);

    let events = task_events(&app, &fixture, task.id).await;
    let last = events.last().expect("the claim is recorded");
    assert_eq!(last.kind.as_str(), "claimed");
    assert_eq!(
        last.actor,
        TaskActor::User {
            user_id: fixture.user.user.id
        },
        "a launch is the user's claim, not the session's",
    );

    // And the session lists it, through the endpoint that reads the link.
    let response = app
        .get_as(&fixture.user, &format!("/api/sessions/{id}/tasks"))
        .await;
    response.assert_status_ok();
    let listed = response.json::<Vec<Value>>();
    assert_eq!(listed.len(), 1, "the session touched one task");
    assert_eq!(listed[0]["id"], json!(task.id));
}

#[tokio::test]
async fn a_task_is_addressed_by_uuid_or_by_its_number() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    let as_text = ready_task(&app, &fixture, "by number as a string").await;
    let as_integer = ready_task(&app, &fixture, "by number as an integer").await;

    let text = fixture
        .create_session(
            &app,
            &json!({
                "profile_id": fixture.profile_id,
                "task_id": as_text.number.to_string(),
            }),
        )
        .await;
    assert_eq!(text["task_id"], json!(as_text.id));

    let integer = fixture
        .create_session(
            &app,
            &json!({ "profile_id": fixture.profile_id, "task_id": as_integer.number }),
        )
        .await;
    assert_eq!(integer["task_id"], json!(as_integer.id));

    // A reference that addresses no task of this project is a 404, whichever
    // form it takes, and nothing is created.
    for reference in [json!(Uuid::new_v4()), json!(9_999), json!("not-a-task")] {
        let response = fixture
            .post(
                &app,
                &json!({ "profile_id": fixture.profile_id, "task_id": reference }),
            )
            .await;
        assert_error(&response, StatusCode::NOT_FOUND, "not found");
    }
}

#[tokio::test]
async fn a_task_that_is_not_claimable_is_a_conflict_and_writes_no_session() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;

    let held = ready_task(&app, &fixture, "held by somebody else").await;
    let holder = seed_session(&app, &fixture, SessionState::Running, None).await;
    hold(&app, &fixture, held.id, holder.id).await;

    let blocked = ready_task(&app, &fixture, "blocked").await;
    block(&app, &fixture, blocked.id).await;

    let finished = task_in(&app, &fixture, "already done", "done").await;

    let before = SessionRepository::new(&app.pool)
        .list_by_project(fixture.project_id, None)
        .await
        .expect("the list runs")
        .len();

    for task in [&held, &blocked, &finished] {
        let response = fixture
            .post(
                &app,
                &json!({ "profile_id": fixture.profile_id, "task_id": task.id }),
            )
            .await;
        assert_error(&response, StatusCode::CONFLICT, "task is not claimable");
    }

    // The session row is inserted before the claim and rolls back with it.
    assert_eq!(
        SessionRepository::new(&app.pool)
            .list_by_project(fixture.project_id, None)
            .await
            .expect("the list runs")
            .len(),
        before,
        "a refused claim left a session behind",
    );
}

#[tokio::test]
async fn a_task_waiting_for_a_person_can_be_launched() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    let task = task_in(&app, &fixture, "escalated", "needs_human").await;

    let body = fixture
        .create_session(
            &app,
            &json!({ "profile_id": fixture.profile_id, "task_id": task.id }),
        )
        .await;

    assert_eq!(body["task_id"], json!(task.id));
    let claimed = read_task(&app, &fixture, task.id).await;
    assert_eq!(
        claimed.lease_holder_session_id,
        Some(id_of(&body)),
        "the human state qualifies for a launch",
    );
}

#[tokio::test]
async fn a_hand_off_pins_the_base_and_the_clone_starts_there() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    let task = ready_task(&app, &fixture, "Continue the parser").await;
    let source = seed_session(&app, &fixture, SessionState::Done, None).await;
    let commit = upstream_head(&fixture).await;
    let handoff_id = publish_handoff(
        &app,
        &fixture,
        &task,
        source.id,
        &commit,
        "First pass; the lexer still drops comments.",
    )
    .await;

    let body = fixture
        .create_session(
            &app,
            &json!({ "profile_id": fixture.profile_id, "task_id": task.id }),
        )
        .await;
    let id = id_of(&body);

    assert_eq!(body["handoff_id"], json!(handoff_id));
    assert_eq!(body["base_ref"], json!(commit));
    assert_eq!(commit.len(), 40, "a hand-off pins a full object id");

    // And the checkout is that exact commit.
    wait_for_state(&app, id, SessionState::Running).await;
    let work = SessionDirs::from_config(&app.state.config, id).work();
    assert_eq!(
        run_git(&work, &["rev-parse", "HEAD"]).await.trim(),
        commit,
        "the clone did not start from the hand-off commit",
    );

    // The generated message names the revision it was handed.
    let payload = wait_for_event(&app, id, "user_message").await;
    let text = payload["text"].as_str().expect("a message").to_string();
    assert!(text.starts_with(&generated_for(&task)), "{text}");
    assert!(
        text.contains(&format!(
            "Current hand-off {handoff_id} from session {} on branch session/source at commit \
             {commit} (review: unreviewed). Hand-off comment: First pass; the lexer still drops \
             comments.",
            source.id,
        )),
        "{text}",
    );
    assert!(
        !text.contains("Your checkout starts from"),
        "nothing overrode the hand-off: {text}",
    );
}

#[tokio::test]
async fn an_explicit_base_overrides_the_hand_off_and_says_so() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    let task = ready_task(&app, &fixture, "Continue the parser").await;
    let source = seed_session(&app, &fixture, SessionState::Done, None).await;
    let commit = upstream_head(&fixture).await;
    let handoff_id = publish_handoff(
        &app,
        &fixture,
        &task,
        source.id,
        &commit,
        "Ready for review.",
    )
    .await;

    let body = fixture
        .create_session(
            &app,
            &json!({
                "profile_id": fixture.profile_id,
                "task_id": task.id,
                "base_ref": "main",
            }),
        )
        .await;
    let id = id_of(&body);

    assert_eq!(body["base_ref"], json!("main"));
    assert_eq!(
        body["handoff_id"],
        Value::Null,
        "an explicit base records no hand-off",
    );

    let payload = wait_for_event(&app, id, "user_message").await;
    let text = payload["text"].as_str().expect("a message").to_string();
    assert!(
        text.contains(&format!("Current hand-off {handoff_id}")),
        "{text}"
    );
    assert!(
        text.ends_with(&format!(
            "Your checkout starts from main, not from the hand-off commit; fetch \
             refs/handoffs/{handoff_id} before continuing that work.",
        )),
        "{text}",
    );
}

#[tokio::test]
async fn the_generated_message_is_the_first_user_message_and_carries_no_user() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    let task = ready_task(&app, &fixture, "Fix the login form").await;

    let body = fixture
        .create_session(
            &app,
            &json!({
                "profile_id": fixture.profile_id,
                "task_id": task.id,
                "message": "Start with the tests",
            }),
        )
        .await;
    let id = id_of(&body);

    // Two `user_message` events, in the documented order: nobody typed the
    // first one (`SPEC.md`, "Sessions").
    let deadline = tokio::time::Instant::now() + PATIENCE;
    let messages = loop {
        let rows = sqlx::query_scalar::<_, Value>(
            "SELECT payload FROM events WHERE session_id = $1 AND kind = 'user_message' \
             ORDER BY seq",
        )
        .bind(id)
        .fetch_all(&app.pool)
        .await
        .expect("the events are readable");

        if rows.len() >= 2 {
            break rows;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "only {} user messages within {PATIENCE:?}",
            rows.len(),
        );
        tokio::time::sleep(POLL).await;
    };

    assert_eq!(messages[0]["text"], json!(generated_for(&task)));
    assert_eq!(messages[0]["user_id"], Value::Null);
    assert_eq!(messages[1]["text"], json!("Start with the tests"));
    assert_eq!(messages[1]["user_id"], json!(fixture.user.user.id));

    // And the CLI read them in that order.
    let container_id = app
        .engine()
        .container_id_for_session(id)
        .expect("the launch created a container");
    wait_for("both messages reached the container's stdin", || {
        app.engine().stdin_lines(&container_id).len() >= 2
    })
    .await;
    let written = app.engine().stdin_lines(&container_id);
    assert!(written[0].contains("You hold task #"), "{}", written[0]);
    assert!(
        written[1].contains("Start with the tests"),
        "{}",
        written[1],
    );
}

#[tokio::test]
async fn an_ephemeral_launch_joins_the_generated_message_and_the_message() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    let ephemeral = fixture.ephemeral_profile(&app).await;
    let task = ready_task(&app, &fixture, "Run the tests").await;

    let body = fixture
        .create_session(
            &app,
            &json!({
                "profile_id": ephemeral,
                "task_id": task.id,
                "message": "Start with the tests",
            }),
        )
        .await;
    let id = id_of(&body);

    wait_for("the launch created a container", || {
        app.engine().container_id_for_session(id).is_some()
    })
    .await;
    let container_id = app
        .engine()
        .container_id_for_session(id)
        .expect("the container was recorded");
    let cmd = app
        .engine()
        .spec_of(&container_id)
        .expect("the specification was recorded")
        .cmd;

    let prompt = cmd
        .iter()
        .position(|argument| argument == "-p")
        .map(|at| cmd[at + 1].clone())
        .expect("an ephemeral session runs its prompt");
    assert_eq!(
        prompt,
        format!("{}\n\nStart with the tests", generated_for(&task)),
    );
}

#[tokio::test]
async fn an_ephemeral_launch_for_a_task_needs_no_message() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    let ephemeral = fixture.ephemeral_profile(&app).await;
    let task = ready_task(&app, &fixture, "Run the tests").await;

    let body = fixture
        .create_session(
            &app,
            &json!({ "profile_id": ephemeral, "task_id": task.id }),
        )
        .await;
    assert_eq!(body["task_id"], json!(task.id));

    // Neither a task nor a message is still a 400 (ADR 0003).
    let response = fixture
        .post(&app, &json!({ "profile_id": ephemeral }))
        .await;
    assert_error(
        &response,
        StatusCode::BAD_REQUEST,
        "an ephemeral session needs a task_id or a message",
    );
}

#[tokio::test]
async fn mars_task_id_is_in_the_container_only_for_a_task_launch() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    let task = ready_task(&app, &fixture, "Fix the login form").await;

    let without = id_of(
        &fixture
            .create_session(&app, &json!({ "profile_id": fixture.profile_id }))
            .await,
    );
    let with = id_of(
        &fixture
            .create_session(
                &app,
                &json!({ "profile_id": fixture.profile_id, "task_id": task.id }),
            )
            .await,
    );

    let names = |env: &[(String, String)]| {
        env.iter()
            .map(|(name, _)| name.clone())
            .collect::<Vec<String>>()
    };

    let plain = recorded_env(&app, without).await;
    assert!(
        !names(&plain).contains(&"MARS_TASK_ID".to_string()),
        "a session without a task carries MARS_TASK_ID: {plain:?}",
    );

    let claiming = recorded_env(&app, with).await;
    let at = names(&claiming)
        .iter()
        .position(|name| name == "MARS_TASK_ID")
        .expect("a task launch carries MARS_TASK_ID");
    assert_eq!(claiming[at].1, task.id.to_string());
    assert_eq!(
        names(&claiming)[at - 1],
        "MARS_PROJECT_ID",
        "MARS_TASK_ID comes after MARS_PROJECT_ID and before the secrets",
    );
}

#[tokio::test]
async fn a_launch_that_fails_while_creating_releases_the_task() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    let task = ready_task(&app, &fixture, "Fix the login form").await;

    // The image the project's default profile names, made unpullable.
    let profiles = app
        .get_as(
            &fixture.user,
            &format!("/api/projects/{}/profiles", fixture.project_id),
        )
        .await
        .json::<Vec<Value>>();
    let image = profiles
        .iter()
        .find(|profile| id_of(profile) == fixture.profile_id)
        .and_then(|profile| profile["image"].as_str())
        .expect("a profile names its image")
        .to_string();
    app.engine().set_missing_images([image]);
    app.engine().fail_next_pull("manifest unknown");

    let body = fixture
        .create_session(
            &app,
            &json!({ "profile_id": fixture.profile_id, "task_id": task.id }),
        )
        .await;
    let id = id_of(&body);

    let failed = wait_for_state(&app, id, SessionState::Failed).await;
    assert_eq!(
        failed.error.as_deref(),
        Some("image pull failed: manifest unknown")
    );

    // The end-of-session hook released the lease: `ARCHITECTURE.md`, "Task
    // tracker" → "Launching a session for a task".
    let deadline = tokio::time::Instant::now() + PATIENCE;
    let released = loop {
        let stored = read_task(&app, &fixture, task.id).await;
        if stored.lease_holder_session_id.is_none() {
            break stored;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the lease was still held {PATIENCE:?} after the launch failed",
        );
        tokio::time::sleep(POLL).await;
    };
    assert_eq!(released.state, "ready", "the task left its queue");

    let events = task_events(&app, &fixture, task.id).await;
    let last = events.last().expect("the release is recorded");
    assert_eq!(last.kind.as_str(), "released");
    assert_eq!(last.reason.as_deref(), Some("session_ended"));
    assert_eq!(last.actor, TaskActor::System);

    let system = TaskRepository::new(&app.pool)
        .list_comments(fixture.project_id, task.id)
        .await
        .expect("the comments read")
        .into_iter()
        .filter(|comment| comment.system)
        .collect::<Vec<_>>();
    assert_eq!(system.len(), 1, "one system comment for the release");
    assert_eq!(
        system[0].body,
        format!("Lease released by the orchestrator: holder session {id} ended."),
    );
}

// ---- lists ----

#[tokio::test]
async fn the_lists_answer_a_projects_sessions_and_every_session() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;

    let creating = seed_session(&app, &fixture, SessionState::Creating, None).await;
    let running = seed_session(&app, &fixture, SessionState::Running, Some("second")).await;

    // Another project's session, which only the dashboard list sees.
    let other = Fixture::create(&app).await;
    let elsewhere = seed_session(&app, &other, SessionState::Creating, None).await;

    // Newest first.
    let response = app
        .get_as(&fixture.user, &sessions_path(fixture.project_id))
        .await;
    response.assert_status_ok();
    let ids: Vec<Uuid> = response.json::<Vec<Value>>().iter().map(id_of).collect();
    assert_eq!(ids, [running.id, creating.id]);

    // The `state` filter, on the project list and on the dashboard.
    let response = app
        .get_as(
            &fixture.user,
            &format!("{}?state=running", sessions_path(fixture.project_id)),
        )
        .await;
    response.assert_status_ok();
    assert_eq!(
        response
            .json::<Vec<Value>>()
            .iter()
            .map(id_of)
            .collect::<Vec<_>>(),
        [running.id],
    );

    let response = app.get_as(&fixture.user, "/api/sessions").await;
    response.assert_status_ok();
    let all: Vec<Uuid> = response.json::<Vec<Value>>().iter().map(id_of).collect();
    for id in [creating.id, running.id, elsewhere.id] {
        assert!(all.contains(&id), "the dashboard list is missing {id}");
    }

    let response = app
        .get_as(&fixture.user, "/api/sessions?state=running")
        .await;
    response.assert_status_ok();
    assert_eq!(
        response
            .json::<Vec<Value>>()
            .iter()
            .map(id_of)
            .collect::<Vec<_>>(),
        [running.id],
    );

    // A state that is not one of the five, on both.
    for path in [
        format!("{}?state=stopped", sessions_path(fixture.project_id)),
        "/api/sessions?state=stopped".to_string(),
    ] {
        let response = app.get_as(&fixture.user, &path).await;
        assert_error(&response, StatusCode::BAD_REQUEST, "unknown state");
    }

    // An unknown project is a 404, not an empty list.
    let response = app
        .get_as(&fixture.user, &sessions_path(Uuid::new_v4()))
        .await;
    assert_error(&response, StatusCode::NOT_FOUND, "not found");

    // Both lists need a token.
    for path in [
        sessions_path(fixture.project_id),
        "/api/sessions".to_string(),
    ] {
        let response = app.server.get(&path).await;
        response.assert_status(StatusCode::UNAUTHORIZED);
        response.assert_json(&unauthorized());
    }
}

// ---- read and retitle ----

#[tokio::test]
async fn a_session_reads_back_without_its_token_hash() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    let session = seed_session(&app, &fixture, SessionState::Creating, Some("a name")).await;

    let response = app.get_as(&fixture.user, &session_path(session.id)).await;
    response.assert_status_ok();
    let body = response.json::<Value>();
    assert_eq!(id_of(&body), session.id);
    assert_eq!(body["title"], json!("a name"));
    assert!(
        body.get("mcp_token_hash").is_none(),
        "the token hash reached a client: {body}",
    );

    let response = app
        .get_as(&fixture.user, &session_path(Uuid::new_v4()))
        .await;
    assert_error(&response, StatusCode::NOT_FOUND, "not found");

    let response = app.server.get(&session_path(session.id)).await;
    response.assert_status(StatusCode::UNAUTHORIZED);
    response.assert_json(&unauthorized());
}

#[tokio::test]
async fn a_retitle_stores_the_new_title_and_refuses_an_invalid_one() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    let session = seed_session(&app, &fixture, SessionState::Creating, Some("before")).await;

    let response = app
        .put_as(&fixture.user, &session_path(session.id))
        .json(&json!({ "title": "  after  " }))
        .await;
    response.assert_status_ok();
    assert_eq!(response.json::<Value>()["title"], json!("after"));
    assert_eq!(
        reload(&app, session.id).await.title.as_deref(),
        Some("after")
    );

    // Empty once trimmed, and too long.
    let too_long = "t".repeat(201);
    for title in ["   ", too_long.as_str()] {
        let response = app
            .put_as(&fixture.user, &session_path(session.id))
            .json(&json!({ "title": title }))
            .await;
        assert_error(
            &response,
            StatusCode::BAD_REQUEST,
            "title must be 1 to 200 characters",
        );
    }
    assert_eq!(
        reload(&app, session.id).await.title.as_deref(),
        Some("after")
    );

    let response = app
        .put_as(&fixture.user, &session_path(Uuid::new_v4()))
        .json(&json!({ "title": "anything" }))
        .await;
    assert_error(&response, StatusCode::NOT_FOUND, "not found");

    let response = app
        .server
        .put(&session_path(session.id))
        .json(&json!({ "title": "anything" }))
        .await;
    response.assert_status(StatusCode::UNAUTHORIZED);
    response.assert_json(&unauthorized());
}

// ---- events ----

#[tokio::test]
async fn an_event_page_walks_backwards_and_strips_internal_fields() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    let session = seed_session(&app, &fixture, SessionState::Creating, None).await;
    seed_events(&app, session.id, 7).await;

    /// The sequences of one page, and its `has_more`.
    fn page(body: &Value) -> (Vec<i64>, bool) {
        let events = body["events"].as_array().expect("a page carries events");
        for event in events {
            assert!(
                event.get("_offset").is_none(),
                "an internal field reached a client: {event}",
            );
        }

        (
            events
                .iter()
                .map(|event| event["seq"].as_i64().expect("an event carries a seq"))
                .collect(),
            body["has_more"].as_bool().expect("a page says has_more"),
        )
    }

    let response = app
        .get_as(
            &fixture.user,
            &format!("{}?limit=3", events_path(session.id)),
        )
        .await;
    response.assert_status_ok();
    assert_eq!(page(&response.json::<Value>()), (vec![5, 6, 7], true));

    let response = app
        .get_as(
            &fixture.user,
            &format!("{}?before=5&limit=3", events_path(session.id)),
        )
        .await;
    response.assert_status_ok();
    assert_eq!(page(&response.json::<Value>()), (vec![2, 3, 4], true));

    let response = app
        .get_as(
            &fixture.user,
            &format!("{}?before=2", events_path(session.id)),
        )
        .await;
    response.assert_status_ok();
    assert_eq!(page(&response.json::<Value>()), (vec![1], false));

    // A session with no events at all.
    let empty = seed_session(&app, &fixture, SessionState::Creating, None).await;
    let response = app.get_as(&fixture.user, &events_path(empty.id)).await;
    response.assert_status_ok();
    response.assert_json(&json!({ "events": [], "has_more": false }));
}

#[tokio::test]
async fn an_event_page_refuses_a_limit_or_cursor_it_cannot_serve() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    let session = seed_session(&app, &fixture, SessionState::Creating, None).await;

    for limit in ["0", "501"] {
        let response = app
            .get_as(
                &fixture.user,
                &format!("{}?limit={limit}", events_path(session.id)),
            )
            .await;
        assert_error(
            &response,
            StatusCode::BAD_REQUEST,
            "limit must be between 1 and 500",
        );
    }

    let response = app
        .get_as(
            &fixture.user,
            &format!("{}?before=0", events_path(session.id)),
        )
        .await;
    assert_error(
        &response,
        StatusCode::BAD_REQUEST,
        "before must be 1 or greater",
    );

    let response = app
        .get_as(&fixture.user, &events_path(Uuid::new_v4()))
        .await;
    assert_error(&response, StatusCode::NOT_FOUND, "not found");

    let response = app.server.get(&events_path(session.id)).await;
    response.assert_status(StatusCode::UNAUTHORIZED);
    response.assert_json(&unauthorized());
}

// ---- input ----

#[tokio::test]
async fn input_to_a_creating_session_is_accepted_and_queued() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    let session = seed_session(&app, &fixture, SessionState::Creating, None).await;

    // The entry a launch would have made, with the receiver held so the queue
    // is the owner's to drain (`ARCHITECTURE.md`, "Session owner task").
    let _owner =
        app.session_registry()
            .register(session.id, ProfileKind::Conversational, Phase::Creating);

    let response = app
        .post_as(&fixture.user, &action_path(session.id, "input"))
        .json(&json!({ "kind": "message", "text": "start with the login form" }))
        .await;
    response.assert_status(StatusCode::ACCEPTED);
    response.assert_text("");

    assert_eq!(
        app.session_registry().queued(session.id),
        1,
        "the accepted input was not queued for the owner",
    );
}

#[tokio::test]
async fn input_to_a_running_session_reaches_the_transcript() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    let (id, _container_id) = launched_session(&app, &fixture).await;

    let response = app
        .post_as(&fixture.user, &action_path(id, "input"))
        .json(&json!({ "kind": "message", "text": "have a look at the login form" }))
        .await;
    response.assert_status(StatusCode::ACCEPTED);
    response.assert_text("");

    // Drained once stdin is attached and recorded by the owner, attributed to
    // the caller of this route (ADR 0032).
    let payload = wait_for_event(&app, id, "user_message").await;
    assert_eq!(payload["text"], json!("have a look at the login form"));
    assert_eq!(payload["user_id"], json!(fixture.user.user.id));
    // No `client_id` was sent, so the event carries none.
    assert_eq!(payload.get("client_id"), None);
}

/// The socket's echo key on the REST route: a client whose socket is closed
/// sends its input here and still gets the id back on the `user_message`, so
/// its optimistic message reconciles (`SPEC.md`, "Sessions"; "WebSocket:
/// session stream").
#[tokio::test]
async fn input_echoes_the_client_id_on_the_user_message() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    let (id, _container_id) = launched_session(&app, &fixture).await;

    let response = app
        .post_as(&fixture.user, &action_path(id, "input"))
        .json(&json!({
            "kind": "message",
            "text": "and the signup form",
            "client_id": "client-1",
        }))
        .await;
    response.assert_status(StatusCode::ACCEPTED);

    let payload = wait_for_event(&app, id, "user_message").await;
    assert_eq!(payload["text"], json!("and the signup form"));
    assert_eq!(payload["client_id"], json!("client-1"));

    // The same bound the socket applies, in bytes.
    let response = app
        .post_as(&fixture.user, &action_path(id, "input"))
        .json(&json!({
            "kind": "message",
            "text": "anything",
            "client_id": "x".repeat(129),
        }))
        .await;
    assert_error(&response, StatusCode::BAD_REQUEST, "client_id too long");
}

#[tokio::test]
async fn input_to_a_parked_session_resumes_it() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    let session = seed_session(&app, &fixture, SessionState::Parked, None).await;
    record_cli_session_id(&app, session.id, "fake-cli-session-id").await;

    let response = app
        .post_as(&fixture.user, &action_path(session.id, "input"))
        .json(&json!({ "kind": "message", "text": "carry on" }))
        .await;
    response.assert_status(StatusCode::ACCEPTED);
    response.assert_text("");

    // The registry knew nothing of this session — a restart's `parked` row —
    // and the relaunch resumes the conversation the queued input belongs to.
    wait_for("the relaunch to create a container", || {
        !app.engine().specs().is_empty()
    })
    .await;
    let spec = app.engine().specs().remove(0);
    assert!(
        spec.cmd
            .windows(2)
            .any(|pair| pair == ["--resume".to_string(), "fake-cli-session-id".to_string()]),
        "the relaunch did not resume the conversation: {:?}",
        spec.cmd,
    );
}

#[tokio::test]
async fn input_answers_the_documented_refusals() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    let session = seed_session(&app, &fixture, SessionState::Creating, None).await;
    let message = json!({ "kind": "message", "text": "anything" });

    // An ephemeral session takes only its launch prompt (ADR 0003).
    let ephemeral = seed_session_of_kind(
        &app,
        &fixture,
        ProfileKind::Ephemeral,
        SessionState::Running,
        None,
    )
    .await;
    let response = app
        .post_as(&fixture.user, &action_path(ephemeral.id, "input"))
        .json(&message)
        .await;
    assert_error(
        &response,
        StatusCode::CONFLICT,
        "ephemeral sessions accept no input",
    );

    // A session that has ended.
    for state in [SessionState::Done, SessionState::Failed] {
        let ended = seed_session(&app, &fixture, state, None).await;
        let response = app
            .post_as(&fixture.user, &action_path(ended.id, "input"))
            .json(&message)
            .await;
        assert_error(
            &response,
            StatusCode::CONFLICT,
            &format!("session is {state}"),
        );
    }

    // Blank text, and text over the documented 1 MiB cap.
    for text in ["", "   \n"] {
        let response = app
            .post_as(&fixture.user, &action_path(session.id, "input"))
            .json(&json!({ "kind": "message", "text": text }))
            .await;
        assert_error(&response, StatusCode::BAD_REQUEST, "text must not be empty");
    }
    let response = app
        .post_as(&fixture.user, &action_path(session.id, "input"))
        .json(&json!({ "kind": "message", "text": "x".repeat(1024 * 1024 + 1) }))
        .await;
    assert_error(&response, StatusCode::BAD_REQUEST, "text too long");

    // A kind that is not `message` — `answer` included, which was a kind until
    // the probe showed nothing can ask (ADR 0033) — and a body that is not a
    // `SessionInput` at all.
    for body in [
        json!({ "kind": "answer", "reply_to": 7, "text": "yes" }),
        json!({ "kind": "shout", "text": "hi" }),
        json!({ "text": "no kind at all" }),
        json!({ "kind": "message" }),
        json!("not an object"),
    ] {
        let response = app
            .post_as(&fixture.user, &action_path(session.id, "input"))
            .json(&body)
            .await;
        response.assert_status(StatusCode::BAD_REQUEST);
    }

    // An unknown session, and no token at all.
    let response = app
        .post_as(&fixture.user, &action_path(Uuid::new_v4(), "input"))
        .json(&message)
        .await;
    assert_error(&response, StatusCode::NOT_FOUND, "not found");

    let response = app
        .server
        .post(&action_path(session.id, "input"))
        .json(&message)
        .await;
    response.assert_status(StatusCode::UNAUTHORIZED);
    response.assert_json(&unauthorized());
}

// ---- stop ----

#[tokio::test]
async fn a_stop_reaches_the_running_container_as_a_sigint() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    let (id, container_id) = launched_session(&app, &fixture).await;

    let response = app.post_as(&fixture.user, &action_path(id, "stop")).await;
    response.assert_status(StatusCode::ACCEPTED);
    response.assert_text("");

    let engine = Arc::clone(&app.engine);
    wait_for("the stop to reach the container", || {
        engine.signals(&container_id).contains(&Signal::Sigint)
    })
    .await;
}

#[tokio::test]
async fn a_stop_refuses_a_session_that_is_not_running() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    let parked = seed_session(&app, &fixture, SessionState::Parked, None).await;

    let response = app
        .post_as(&fixture.user, &action_path(parked.id, "stop"))
        .await;
    assert_error(&response, StatusCode::CONFLICT, "session is parked");

    let response = app
        .post_as(&fixture.user, &action_path(Uuid::new_v4(), "stop"))
        .await;
    assert_error(&response, StatusCode::NOT_FOUND, "not found");

    let response = app.server.post(&action_path(parked.id, "stop")).await;
    response.assert_status(StatusCode::UNAUTHORIZED);
    response.assert_json(&unauthorized());
}

// ---- end ----

#[tokio::test]
async fn an_end_closes_a_running_session_and_publishes_its_branch() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    let (id, container_id) = launched_session(&app, &fixture).await;
    let stopped = exit_on_sigint(&app, &container_id);

    // A commit of the session's own: one that ends at its base commit keeps
    // no ref (ADR 0050; `tests/session_ref_lifetime.rs`).
    let work = DataPaths::from_config(&app.state.config).session_work(id);
    std::fs::write(work.join("WORK.md"), "the agent's work\n").expect("the file is written");
    run_git(&work, &["add", "--", "WORK.md"]).await;
    run_git(&work, &["commit", "--quiet", "-m", "feat: the agent's work"]).await;
    let tip = run_git(&work, &["rev-parse", "HEAD"])
        .await
        .trim()
        .to_string();

    let response = app.post_as(&fixture.user, &action_path(id, "end")).await;
    response.assert_status_ok();
    assert!(
        stopped.await.expect("the exit helper does not panic"),
        "the end did not stop the container",
    );

    let body = response.json::<Value>();
    assert_eq!(id_of(&body), id);
    assert_eq!(body["state"], json!("done"));
    assert_eq!(body["container_id"], Value::Null);
    assert_ne!(body["ended_at"], Value::Null);

    let row = reload(&app, id).await;
    assert_eq!(row.state, SessionState::Done);
    assert_eq!(row.container_id, None);
    assert_eq!(
        app.engine().state_of(&container_id),
        None,
        "the container was not removed",
    );

    // The fetch-back, and the transcript entry that records it.
    let published = mirror_session_ref(&app, &fixture, id)
        .await
        .expect("the mirror has the session ref");
    assert_eq!(published, tip);
    let sync = wait_for_event(&app, id, "git").await;
    assert_eq!(sync["op"], json!("sync"));
    assert_eq!(sync["ok"], json!(true), "{sync}");
    assert_eq!(sync["detail"]["ref"], json!(format!("refs/sessions/{id}")));
    assert_eq!(sync["detail"]["commit"], json!(published));
}

/// A user who launched a session by mistake closes it without waiting for the
/// container they never wanted (`SPEC.md`, "Sessions"; task `qhyhw`). Nothing
/// is launching this one, so the end has nothing to wait for and the row it
/// answers is `done`.
#[tokio::test]
async fn an_end_closes_a_session_that_is_still_creating() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    let creating = seed_session(&app, &fixture, SessionState::Creating, None).await;

    let response = app
        .post_as(&fixture.user, &action_path(creating.id, "end"))
        .await;
    response.assert_status_ok();

    let body = response.json::<Value>();
    assert_eq!(body["state"], json!("done"));
    assert_eq!(body["container_id"], Value::Null);
    assert_ne!(body["ended_at"], Value::Null);

    let row = reload(&app, creating.id).await;
    assert_eq!(row.state, SessionState::Done);
    assert!(
        app.engine().specs().is_empty(),
        "ending a creating session started a container",
    );
}

#[tokio::test]
async fn an_end_refuses_a_session_that_has_nothing_to_end() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;

    for state in [SessionState::Done, SessionState::Failed] {
        let session = seed_session(&app, &fixture, state, None).await;
        let response = app
            .post_as(&fixture.user, &action_path(session.id, "end"))
            .await;
        assert_error(
            &response,
            StatusCode::CONFLICT,
            &format!("session is {state}"),
        );
    }

    let response = app
        .post_as(&fixture.user, &action_path(Uuid::new_v4(), "end"))
        .await;
    assert_error(&response, StatusCode::NOT_FOUND, "not found");

    let session = seed_session(&app, &fixture, SessionState::Parked, None).await;
    let response = app.server.post(&action_path(session.id, "end")).await;
    response.assert_status(StatusCode::UNAUTHORIZED);
    response.assert_json(&unauthorized());
}

// ---- retry ----

#[tokio::test]
async fn a_retry_parks_a_failed_session_and_clears_its_error() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    let failed = seed_session(&app, &fixture, SessionState::Failed, None).await;
    assert!(failed.error.is_some(), "the fixture records no error");

    // No body at all, which is the documented "absent" case.
    let response = app
        .post_as(&fixture.user, &action_path(failed.id, "retry"))
        .await;
    response.assert_status_ok();

    let body = response.json::<Value>();
    assert_eq!(body["state"], json!("parked"));
    assert_eq!(body["error"], Value::Null);
    assert_eq!(reload(&app, failed.id).await.state, SessionState::Parked);
    assert!(
        app.engine().specs().is_empty(),
        "a retry with no message launched a container",
    );

    // An empty object is the same thing said in JSON.
    let other = seed_session(&app, &fixture, SessionState::Failed, None).await;
    let response = app
        .post_as(&fixture.user, &action_path(other.id, "retry"))
        .json(&json!({}))
        .await;
    response.assert_status_ok();
    assert_eq!(response.json::<Value>()["state"], json!("parked"));
}

#[tokio::test]
async fn a_retry_with_a_message_relaunches_the_session() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    let failed = seed_session(&app, &fixture, SessionState::Failed, None).await;
    record_cli_session_id(&app, failed.id, "fake-cli-session-id").await;

    let response = app
        .post_as(&fixture.user, &action_path(failed.id, "retry"))
        .json(&json!({ "message": "try that again" }))
        .await;
    response.assert_status_ok();
    // The documented response is the row the transition produced, even though
    // the relaunch is already under way.
    assert_eq!(response.json::<Value>()["state"], json!("parked"));

    wait_for("the retry to create a container", || {
        !app.engine().specs().is_empty()
    })
    .await;
    let payload = wait_for_event(&app, failed.id, "user_message").await;
    assert_eq!(payload["text"], json!("try that again"));
}

#[tokio::test]
async fn a_retry_answers_the_documented_refusals() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;

    // An ephemeral session is never retried; a new one is launched instead.
    let ephemeral = seed_session_of_kind(
        &app,
        &fixture,
        ProfileKind::Ephemeral,
        SessionState::Failed,
        None,
    )
    .await;
    let response = app
        .post_as(&fixture.user, &action_path(ephemeral.id, "retry"))
        .await;
    assert_error(
        &response,
        StatusCode::CONFLICT,
        "ephemeral sessions are not retried; launch a new one",
    );

    // Only `failed` has an edge back into the lifecycle.
    let parked = seed_session(&app, &fixture, SessionState::Parked, None).await;
    let response = app
        .post_as(&fixture.user, &action_path(parked.id, "retry"))
        .await;
    assert_error(
        &response,
        StatusCode::CONFLICT,
        "session is parked, only a failed session can be retried",
    );

    // A body that is not the documented shape.
    let failed = seed_session(&app, &fixture, SessionState::Failed, None).await;
    for body in [json!({ "text": "try again" }), json!({ "message": 7 })] {
        let response = app
            .post_as(&fixture.user, &action_path(failed.id, "retry"))
            .json(&body)
            .await;
        response.assert_status(StatusCode::BAD_REQUEST);
    }
    assert_eq!(reload(&app, failed.id).await.state, SessionState::Failed);

    let response = app
        .post_as(&fixture.user, &action_path(Uuid::new_v4(), "retry"))
        .await;
    assert_error(&response, StatusCode::NOT_FOUND, "not found");

    let response = app.server.post(&action_path(failed.id, "retry")).await;
    response.assert_status(StatusCode::UNAUTHORIZED);
    response.assert_json(&unauthorized());
}

// ---- sync ----

#[tokio::test]
async fn a_sync_answers_the_ref_and_commit_it_published() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    let (id, _container_id) = launched_session(&app, &fixture).await;

    let response = app.post_as(&fixture.user, &action_path(id, "sync")).await;
    response.assert_status_ok();

    let work = DataPaths::from_config(&app.state.config).session_work(id);
    let head = run_git(&work, &["rev-parse", "HEAD"])
        .await
        .trim()
        .to_string();
    response.assert_json(&json!({
        "ref": format!("refs/sessions/{id}"),
        "commit": head,
    }));
    assert_eq!(
        mirror_session_ref(&app, &fixture, id).await,
        Some(head),
        "the mirror does not point at the commit the sync reported",
    );
}

#[tokio::test]
async fn a_sync_refuses_a_session_with_no_work_tree_yet() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    let creating = seed_session(&app, &fixture, SessionState::Creating, None).await;

    let response = app
        .post_as(&fixture.user, &action_path(creating.id, "sync"))
        .await;
    assert_error(&response, StatusCode::CONFLICT, "session is creating");

    let response = app
        .post_as(&fixture.user, &action_path(Uuid::new_v4(), "sync"))
        .await;
    assert_error(&response, StatusCode::NOT_FOUND, "not found");

    let response = app.server.post(&action_path(creating.id, "sync")).await;
    response.assert_status(StatusCode::UNAUTHORIZED);
    response.assert_json(&unauthorized());
}

// ---- delete ----

#[tokio::test]
async fn a_delete_removes_a_failed_session_with_its_directory_and_its_events() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    let failed = seed_session(&app, &fixture, SessionState::Failed, None).await;
    seed_events(&app, failed.id, 3).await;

    let directory = SessionDirs::from_config(&app.state.config, failed.id);
    directory
        .ensure()
        .await
        .expect("the session directories are made");
    let session_dir = DataPaths::from_config(&app.state.config).session_dir(failed.id);
    assert!(session_dir.is_dir(), "the fixture has no directory");

    let response = app.delete_as(&fixture.user, &session_path(failed.id)).await;
    response.assert_status(StatusCode::NO_CONTENT);
    response.assert_text("");

    assert!(!session_dir.exists(), "the session directory is left");
    assert!(
        SessionRepository::new(&app.pool)
            .find(failed.id)
            .await
            .expect("the lookup runs")
            .is_none(),
        "the session row is left",
    );
    let events: i64 = sqlx::query_scalar("SELECT count(*) FROM events WHERE session_id = $1")
        .bind(failed.id)
        .fetch_one(&app.pool)
        .await
        .expect("the events are countable");
    assert_eq!(events, 0, "the session's events did not cascade");
}

#[tokio::test]
async fn a_delete_refuses_a_session_that_has_not_stopped() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;

    for state in [
        SessionState::Creating,
        SessionState::Running,
        SessionState::Parked,
    ] {
        let session = seed_session(&app, &fixture, state, None).await;
        let response = app
            .delete_as(&fixture.user, &session_path(session.id))
            .await;
        assert_error(
            &response,
            StatusCode::CONFLICT,
            "session must be done or failed",
        );
        assert_eq!(reload(&app, session.id).await.state, state);
    }

    let response = app
        .delete_as(&fixture.user, &session_path(Uuid::new_v4()))
        .await;
    assert_error(&response, StatusCode::NOT_FOUND, "not found");

    let done = seed_session(&app, &fixture, SessionState::Done, None).await;
    let response = app.server.delete(&session_path(done.id)).await;
    response.assert_status(StatusCode::UNAUTHORIZED);
    response.assert_json(&unauthorized());
    assert!(
        SessionRepository::new(&app.pool)
            .find(done.id)
            .await
            .expect("the lookup runs")
            .is_some(),
        "an unauthenticated delete removed the row",
    );
}

// ---- the password-change gate ----

#[tokio::test]
async fn every_session_route_is_behind_the_password_change_gate() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    let session = seed_session(&app, &fixture, SessionState::Creating, None).await;
    let gated = app
        .create_gated_user("gated-sessions", "gated-sessions@example.test")
        .await;

    let expected = json!({ "status": 403, "error": "password change required" });
    let responses = [
        app.get_as(&gated, &sessions_path(fixture.project_id)).await,
        app.post_as(&gated, &sessions_path(fixture.project_id))
            .json(&json!({ "profile_id": fixture.profile_id }))
            .await,
        app.get_as(&gated, "/api/sessions").await,
        app.get_as(&gated, &session_path(session.id)).await,
        app.put_as(&gated, &session_path(session.id))
            .json(&json!({ "title": "nope" }))
            .await,
        app.delete_as(&gated, &session_path(session.id)).await,
        app.get_as(&gated, &events_path(session.id)).await,
        app.post_as(&gated, &action_path(session.id, "input"))
            .json(&json!({ "kind": "message", "text": "nope" }))
            .await,
        app.post_as(&gated, &action_path(session.id, "stop")).await,
        app.post_as(&gated, &action_path(session.id, "end")).await,
        app.post_as(&gated, &action_path(session.id, "retry")).await,
        app.post_as(&gated, &action_path(session.id, "sync")).await,
    ];

    for response in responses {
        response.assert_status(StatusCode::FORBIDDEN);
        response.assert_json(&expected);
    }
}
