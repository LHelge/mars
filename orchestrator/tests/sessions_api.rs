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
//! `DELETE /api/sessions/{id}` is not here: it is mounted by the session-action
//! routes, with its own tests.
//!
//! Every credential-shaped value is an obviously fake stand-in (rule 3).
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use std::time::Duration;

use axum::http::StatusCode;
use axum_test::TestResponse;
use common::{AuthenticatedUser, TestApp};
use mars_orchestrator::git::testutil::{TestUpstream, run_git};
use mars_orchestrator::models::{
    NewEvent, NewSession, ProfileKind, ProjectStatus, Session, SessionState, StateChange,
};
use mars_orchestrator::projects::clone_job;
use mars_orchestrator::repositories::SessionRepository;
use mars_orchestrator::session::McpToken;
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
            .json(&json!({ "name": "implementer", "kind": "ephemeral" }))
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
    let mut new_session = NewSession::new(
        fixture.project_id,
        fixture.profile_id,
        ProfileKind::Conversational,
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
        let mut tx = app.pool.begin().await.expect("a transaction begins");
        current = sessions
            .set_state(&mut tx, current.id, *target, &StateChange::plain())
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
        app.get_as(&gated, &events_path(session.id)).await,
    ];

    for response in responses {
        response.assert_status(StatusCode::FORBIDDEN);
        response.assert_json(&expected);
    }
}
