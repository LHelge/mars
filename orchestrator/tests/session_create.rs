//! `session::create_session` driven directly, with no HTTP request and no user
//! (`ARCHITECTURE.md`, "Unattended launches").
//!
//! What a *user* launch does is asserted through the route in
//! `tests/sessions_api.rs`, which is the proof that moving the launch out of
//! the route changed nothing. What is asserted here is the other half of the
//! entry point: the two unattended actors, which no endpoint can reach. A
//! launch by the dispatcher or by a schedule writes no `created_by`, claims its
//! task as `system`, queues its message as nobody's, and refuses a task outside
//! the profile's served states when it asks for that check — the one decision
//! this function leaves to its caller.
//!
//! The project is created through `POST /api/projects` over a real bare
//! repository, as every session suite's is, and waited on until it is `ready`:
//! the arrangement is a user's, only the launch under test is not.
//!
//! Every credential-shaped value is an obviously fake stand-in (rule 3).
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use std::time::Duration;

use axum::http::StatusCode;
use common::{AuthenticatedUser, TestApp};
use mars_orchestrator::events::{TaskActor, TaskEvent};
use mars_orchestrator::git::testutil::TestUpstream;
use mars_orchestrator::models::{
    NewSession, NewTask, ProfileKind, ProjectStatus, Session, Task, TaskRef,
};
use mars_orchestrator::projects::clone_job;
use mars_orchestrator::repositories::{SessionRepository, TaskRepository};
use mars_orchestrator::session::{LaunchActor, LaunchRequest, McpToken, create_session};
use mars_orchestrator::tracker::{TaskDto, TrackerMutation};
use serde_json::{Value, json};
use uuid::Uuid;

/// How long a clone or a launch may take before the scenario gives up with a
/// message rather than hanging.
const PATIENCE: Duration = Duration::from_secs(30);

/// How often the polls re-read what they are waiting for.
const POLL: Duration = Duration::from_millis(25);

/// A signed-in user, a `ready` project and its seeded profiles.
struct Fixture {
    /// Held because dropping it removes the upstream repository the mirror was
    /// cloned from.
    _upstream: TestUpstream,
    user: AuthenticatedUser,
    project_id: Uuid,
    profile_id: Uuid,
}

impl Fixture {
    async fn create(app: &TestApp) -> Self {
        let upstream = TestUpstream::create().await;
        let name = Uuid::new_v4().simple().to_string()[..8].to_string();
        let user = app
            .create_user(
                &format!("user-{name}"),
                &format!("user-{name}@example.test"),
                &format!("fake-password-{name}"),
            )
            .await;

        let response = app
            .post_as(&user, "/api/projects")
            .json(&json!({
                "name": format!("project-{name}"),
                "remote_url": format!("file://{}", upstream.path.display()),
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

        let profile_id = default_profile(app, &user, project_id).await;

        Self {
            _upstream: upstream,
            user,
            project_id,
            profile_id,
        }
    }

    /// An ephemeral profile of this project, for the scheduled-agent shape.
    async fn ephemeral_profile(&self, app: &TestApp) -> Uuid {
        let response = app
            .post_as(
                &self.user,
                &format!("/api/projects/{}/profiles", self.project_id),
            )
            .json(&json!({ "name": "scanner", "kind": "ephemeral" }))
            .await;
        response.assert_status(StatusCode::CREATED);

        id_of(&response.json::<Value>())
    }

    /// The launch under test, as `actor`.
    async fn launch(
        &self,
        app: &TestApp,
        request: LaunchRequest,
        actor: LaunchActor,
    ) -> mars_orchestrator::prelude::Result<Session> {
        create_session(&app.state, self.project_id, request, actor).await
    }
}

/// The `id` of a resource body, as a [`Uuid`].
fn id_of(body: &Value) -> Uuid {
    body["id"]
        .as_str()
        .expect("a resource carries an id")
        .parse()
        .expect("the id is a uuid")
}

/// The id of the project's seeded `default` profile, which serves `ready`.
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

    id_of(default)
}

/// A task of this project in the named state, inserted as the tracker inserts
/// one.
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

/// A task in the project's `ready` queue, which the default profile serves.
async fn ready_task(app: &TestApp, fixture: &Fixture, title: &str) -> Task {
    task_in(app, fixture, title, "ready").await
}

/// The task as it is committed.
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

/// A session row with no launch, for a lease that has to be held by somebody.
async fn seed_session(app: &TestApp, fixture: &Fixture) -> Session {
    let mut new_session = NewSession::new(
        fixture.project_id,
        fixture.profile_id,
        ProfileKind::Conversational,
        "main",
        McpToken::generate().hash(),
    );
    new_session.created_by = Some(fixture.user.user.id);
    let new_session = new_session
        .with_title(Some("the holder"))
        .expect("the fixture title is valid");

    let mut tx = app.pool.begin().await.expect("a transaction begins");
    let inserted = SessionRepository::new(&app.pool)
        .insert(&mut tx, &new_session)
        .await
        .expect("the session inserts");
    tx.commit().await.expect("the transaction commits");

    inserted
}

/// How many sessions this project has.
async fn session_count(app: &TestApp, fixture: &Fixture) -> usize {
    SessionRepository::new(&app.pool)
        .list_by_project(fixture.project_id, None)
        .await
        .expect("the list runs")
        .len()
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

/// The session's `user_message` payloads, oldest first, once there are `count`
/// of them.
async fn user_messages(app: &TestApp, session_id: Uuid, count: usize) -> Vec<Value> {
    let deadline = tokio::time::Instant::now() + PATIENCE;

    loop {
        let rows = sqlx::query_scalar::<_, Value>(
            "SELECT payload FROM events WHERE session_id = $1 AND kind = 'user_message' \
             ORDER BY seq",
        )
        .bind(session_id)
        .fetch_all(&app.pool)
        .await
        .expect("the events are readable");

        if rows.len() >= count {
            return rows;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "only {} of {count} user messages within {PATIENCE:?}",
            rows.len(),
        );
        tokio::time::sleep(POLL).await;
    }
}

#[tokio::test]
async fn an_unattended_task_launch_claims_as_system_and_records_no_user() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    let task = ready_task(&app, &fixture, "Fix the login form").await;

    let mut request = LaunchRequest::new(fixture.profile_id);
    request.task = Some(TaskRef::Id(task.id));
    request.message = Some("Start with the tests".to_string());

    let session = fixture
        .launch(&app, request, LaunchActor::Dispatcher)
        .await
        .expect("the dispatcher launch succeeds");

    // Nobody launched it, so there is nobody to attribute it to
    // (`docs/data-model.md`, `sessions.created_by`).
    assert_eq!(session.created_by, None);
    assert_eq!(session.task_id, Some(task.id));
    assert_eq!(session.title.as_deref(), Some("Fix the login form"));

    // The claim committed with the row, under the orchestrator's own actor.
    let claimed = read_task(&app, &fixture, task.id).await;
    assert_eq!(claimed.lease_holder_session_id, Some(session.id));
    assert_eq!(claimed.attempts, 1);

    let events = task_events(&app, &fixture, task.id).await;
    let last = events.last().expect("the claim is recorded");
    assert_eq!(last.kind.as_str(), "claimed");
    assert_eq!(
        last.actor,
        TaskActor::System,
        "an unattended launch claims as the orchestrator",
    );

    // Both first messages are nobody's: the generated one because nobody typed
    // it, the caller's because nobody asked for the launch.
    let messages = user_messages(&app, session.id, 2).await;
    assert_eq!(messages[0]["user_id"], Value::Null);
    assert_eq!(messages[1]["text"], json!("Start with the tests"));
    assert_eq!(messages[1]["user_id"], Value::Null);
}

#[tokio::test]
async fn a_scheduled_ephemeral_launch_runs_its_message_as_the_prompt() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    let ephemeral = fixture.ephemeral_profile(&app).await;

    let mut request = LaunchRequest::new(ephemeral);
    request.message = Some("Scan the repository for tech debt".to_string());

    let session = fixture
        .launch(&app, request, LaunchActor::Schedule)
        .await
        .expect("the scheduled launch succeeds");

    assert_eq!(session.created_by, None);
    assert_eq!(session.task_id, None, "a scheduled agent needs no task");

    wait_for("the launch created a container", || {
        app.engine().container_id_for_session(session.id).is_some()
    })
    .await;
    let container_id = app
        .engine()
        .container_id_for_session(session.id)
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
    assert_eq!(prompt, "Scan the repository for tech debt");
}

#[tokio::test]
async fn a_lost_claim_rolls_the_session_row_back() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    let task = ready_task(&app, &fixture, "held by somebody else").await;

    // Another session holds it, so the claim this launch makes loses.
    let holder = seed_session(&app, &fixture).await;
    common::tracker::hold(&app.pool, fixture.project_id, task.id, holder.id).await;

    let before = session_count(&app, &fixture).await;

    let mut request = LaunchRequest::new(fixture.profile_id);
    request.task = Some(TaskRef::Id(task.id));
    let error = fixture
        .launch(&app, request, LaunchActor::Dispatcher)
        .await
        .expect_err("a held task is not claimable");

    assert_eq!(error.status(), StatusCode::CONFLICT);
    assert_eq!(error.to_string(), "task is not claimable");
    assert_eq!(
        session_count(&app, &fixture).await,
        before,
        "a refused claim left a session behind",
    );
}

#[tokio::test]
async fn an_unattended_launch_can_require_the_profile_s_served_states() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;

    // The seeded default profile serves `ready` and nothing else, so a task
    // waiting for a person is not one it may be put on by itself.
    let escalated = task_in(&app, &fixture, "escalated", "needs_human").await;
    let before = session_count(&app, &fixture).await;

    let mut request = LaunchRequest::new(fixture.profile_id);
    request.task = Some(TaskRef::Id(escalated.id));
    request.require_served_state = true;

    let error = fixture
        .launch(&app, request, LaunchActor::Dispatcher)
        .await
        .expect_err("the human state is not served");
    assert_eq!(error.status(), StatusCode::CONFLICT);
    assert_eq!(
        error.to_string(),
        "task is not in a state this profile serves",
    );
    assert_eq!(
        session_count(&app, &fixture).await,
        before,
        "a refused launch left a session behind",
    );

    // The task lost nothing: no lease, no attempt, no claim.
    let untouched = read_task(&app, &fixture, escalated.id).await;
    assert_eq!(untouched.lease_holder_session_id, None);
    assert_eq!(untouched.attempts, 0);

    // A task in the one state it does serve is launched.
    let ready = ready_task(&app, &fixture, "Fix the login form").await;
    let mut request = LaunchRequest::new(fixture.profile_id);
    request.task = Some(TaskRef::Id(ready.id));
    request.require_served_state = true;

    let session = fixture
        .launch(&app, request, LaunchActor::Dispatcher)
        .await
        .expect("a served state is claimable");
    assert_eq!(
        read_task(&app, &fixture, ready.id)
            .await
            .lease_holder_session_id,
        Some(session.id),
    );
}
