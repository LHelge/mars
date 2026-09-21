//! The whole project lifecycle through the public REST API, against real
//! local bare repositories.
//!
//! The Projects epic's three acceptance criteria end to end rather than one
//! endpoint at a time: a project created with nothing but a name and a remote
//! reaches `ready` with a discovered `default_branch` and a repository on
//! disk; an unreachable remote reaches `error` and `retry-clone` recovers once
//! the remote exists; deleting a project removes every row and every on-disk
//! artefact it owned (epic `pkaee`, "Acceptance criteria"; `SPEC.md`,
//! "Projects"; `ARCHITECTURE.md`, "Storage" and "Git model").
//!
//! What each endpoint answers for each bad input is asserted per endpoint in
//! `tests/projects.rs`, `tests/profiles.rs` and `tests/shared_dirs.rs`;
//! nothing here repeats that. What is asserted here is the *sequence*: the
//! states a project moves through, what the repository on disk looks like at
//! each of them, and what survives a deletion (nothing).
//!
//! Git is never mocked (`CLAUDE.md`, "Testing expectations"): every remote is
//! a real bare repository in a `tempfile` directory, built by
//! [`common::git::BareFixture`], which can also make the remote disappear and
//! come back. Nothing here sleeps for a fixed time; every wait goes through
//! [`clone_job::wait_for_clone`] with a finite budget.
//!
//! Every credential and identity is an obviously fake stand-in (rule 3).
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use std::collections::BTreeMap;
use std::time::Duration;

use axum::http::StatusCode;
use chrono::{DateTime, FixedOffset};
use common::git::BareFixture;
use common::{AuthenticatedUser, TestApp};
use mars_orchestrator::events::TaskActor;
use mars_orchestrator::git::DataPaths;
use mars_orchestrator::models::{
    NewEvent, NewSession, NewTask, NewTaskComment, ProfileKind, Project, ProjectStatus,
    SecretUsePurpose, SessionState, StateChange, TaskDependencyKind,
};
use mars_orchestrator::projects::clone_job;
use mars_orchestrator::repositories::{
    ProjectRepository, SecretRepository, SessionRepository, TaskRepository,
};
use mars_orchestrator::tracker::TrackerMutation;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

/// How long a clone of a one-commit local repository may take before the test
/// calls it stuck. Generous: a loaded CI machine runs a dozen git processes
/// for it.
const CLONE_TIMEOUT: Duration = Duration::from_secs(30);

/// Not a real credential: an obviously fake stand-in (rule 3).
const FAKE_CREDENTIAL: &str = "fake-git-credential-for-tests";

/// An obviously fake second project secret, and the name it is stored under
/// (rule 3).
const EXTRA_SECRET_NAME: &str = "FAKE_DEPLOY_TOKEN";
const EXTRA_SECRET_VALUE: &str = "fake-deploy-token-for-tests";

// ---- helpers ----

/// A signed-in ordinary user to make requests as. The password is an obviously
/// fake stand-in of the length `POST /api/test/users` requires (rule 3).
async fn signed_in(app: &TestApp, name: &str) -> AuthenticatedUser {
    app.create_user(
        name,
        &format!("{name}@example.test"),
        &format!("fake-pw-{name}"),
    )
    .await
}

/// `/api/projects/{id}`.
fn project_path(id: Uuid) -> String {
    format!("/api/projects/{id}")
}

/// The id of a project the API answered with.
fn id_of(project: &Value) -> Uuid {
    project["id"]
        .as_str()
        .expect("a project carries an id")
        .parse()
        .expect("the id is a uuid")
}

/// `POST /api/projects` with `body`, asserting the documented 201 and the
/// `cloning` status every create starts in.
async fn create(app: &TestApp, user: &AuthenticatedUser, body: &Value) -> Value {
    let response = app.post_as(user, "/api/projects").json(body).await;

    response.assert_status(StatusCode::CREATED);
    let project = response.json::<Value>();
    assert_eq!(project["status"], json!("cloning"), "{project}");

    project
}

/// Wait for the clone job and assert it ended in `ready`, naming the status
/// message if it did not.
async fn ready(app: &TestApp, id: Uuid) -> Project {
    let project = clone_job::wait_for_clone(&app.state, id, CLONE_TIMEOUT).await;
    assert_eq!(
        project.status,
        ProjectStatus::Ready,
        "{:?}",
        project.status_message
    );

    project
}

/// A `ready` project cloned from `fixture`, and its id.
async fn ready_project(
    app: &TestApp,
    user: &AuthenticatedUser,
    name: &str,
    fixture: &BareFixture,
) -> Uuid {
    let id = id_of(
        &create(
            app,
            user,
            &json!({ "name": name, "remote_url": fixture.url() }),
        )
        .await,
    );
    ready(app, id).await;

    id
}

/// `GET /api/projects/{id}/branches` as `{ name: commit }`, asserting 200.
async fn branches(app: &TestApp, user: &AuthenticatedUser, id: Uuid) -> BTreeMap<String, String> {
    let response = app
        .get_as(user, &format!("/api/projects/{id}/branches"))
        .await;
    response.assert_status_ok();

    response
        .json::<Value>()
        .as_array()
        .expect("the listing is an array")
        .iter()
        .map(|branch| {
            (
                branch["name"].as_str().expect("a branch has a name").into(),
                branch["commit"]
                    .as_str()
                    .expect("a branch has a commit")
                    .into(),
            )
        })
        .collect()
}

/// `GET /api/projects/{id}`, asserting 200.
async fn read(app: &TestApp, user: &AuthenticatedUser, id: Uuid) -> Value {
    let response = app.get_as(user, &project_path(id)).await;
    response.assert_status_ok();

    response.json::<Value>()
}

/// A project's `last_fetched_at`, which every `ready` project has.
fn fetched_at(project: &Value) -> DateTime<FixedOffset> {
    let raw = project["last_fetched_at"]
        .as_str()
        .expect("a fetched project carries a timestamp");

    DateTime::parse_from_rfc3339(raw).expect("the timestamp is RFC 3339")
}

/// The branch the project repository's bare `HEAD` names.
fn head_of(app: &TestApp, id: Uuid) -> String {
    let repo = DataPaths::from_config(&app.state.config).project_repo(id);
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["symbolic-ref", "--end-of-options", "HEAD"])
        .output()
        .expect("git runs");

    assert!(
        output.status.success(),
        "git symbolic-ref failed in {repo:?}"
    );
    String::from_utf8(output.stdout)
        .expect("git wrote utf-8")
        .trim()
        .to_string()
}

// ---- create to ready ----

#[tokio::test]
async fn creates_project_and_reaches_ready_with_discovered_branch() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let fixture = BareFixture::new();

    // Only a name and a remote: the branch is the remote's to tell us.
    let created = create(
        &app,
        &user,
        &json!({ "name": "mars", "remote_url": fixture.url() }),
    )
    .await;
    assert_eq!(created["default_branch"], Value::Null);
    assert_eq!(created["last_fetched_at"], Value::Null);
    let id = id_of(&created);

    let project = ready(&app, id).await;
    assert_eq!(project.default_branch.as_deref(), Some("main"));
    assert_eq!(project.status_message, None);
    assert!(
        project.last_fetched_at.is_some(),
        "the clone is the first fetch"
    );

    // The integration head and the upstream-tracking ref are the same commit
    // the fixture's `main` is at, and nothing else was seeded.
    let listed = branches(&app, &user, id).await;
    let tip = fixture.commit_of("refs/heads/main");
    assert_eq!(
        listed,
        BTreeMap::from([
            ("main".to_string(), tip.clone()),
            ("origin/main".to_string(), tip),
        ])
    );

    // The layout `ARCHITECTURE.md`, "Storage", specifies.
    let layout = app.state.config.project_layout(id);
    assert!(layout.repo_git().is_dir(), "the mirror");
    assert!(layout.claude_dir().is_dir(), "the CLI state directory");
    assert!(layout.shared_root().is_dir(), "the shared-directory root");
    assert_eq!(head_of(&app, id), "refs/heads/main");
}

// ---- error and retry ----

#[tokio::test]
async fn unreachable_remote_reaches_error_and_retry_recovers() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;

    // The ordinary mistyped-or-not-yet-created remote: the URL is well formed
    // and nothing is there.
    let fixture = BareFixture::new();
    fixture.remove();

    let id = id_of(
        &create(
            &app,
            &user,
            &json!({ "name": "mars", "remote_url": fixture.url() }),
        )
        .await,
    );

    let failed = clone_job::wait_for_clone(&app.state, id, CLONE_TIMEOUT).await;
    assert_eq!(failed.status, ProjectStatus::Error);
    let message = failed
        .status_message
        .expect("a failed clone says what happened");
    assert!(!message.trim().is_empty(), "{message}");
    // Nothing that could be the userinfo half of a URL ever reaches the row
    // (rule 3).
    assert!(!message.contains('@'), "{message}");

    // A retry while the remote is still gone answers `cloning` and fails
    // again; the response is the row as it was accepted, not the outcome.
    let retried = app
        .post_as(&user, &format!("/api/projects/{id}/retry-clone"))
        .await;
    retried.assert_status_ok();
    assert_eq!(retried.json::<Value>()["status"], json!("cloning"));
    assert_eq!(
        clone_job::wait_for_clone(&app.state, id, CLONE_TIMEOUT)
            .await
            .status,
        ProjectStatus::Error
    );

    // The operator finally creates the remote, and the same retry recovers.
    fixture.recreate();
    app.post_as(&user, &format!("/api/projects/{id}/retry-clone"))
        .await
        .assert_status_ok();

    let project = ready(&app, id).await;
    assert_eq!(project.default_branch.as_deref(), Some("main"));
    assert_eq!(project.status_message, None);
    assert_eq!(head_of(&app, id), "refs/heads/main");

    // And there is nothing left to retry (`SPEC.md`, "Projects").
    let refused = app
        .post_as(&user, &format!("/api/projects/{id}/retry-clone"))
        .await;
    refused.assert_status(StatusCode::CONFLICT);
    refused.assert_json(&json!({ "status": 409, "error": "project is not in error state" }));
}

#[tokio::test]
async fn supplied_default_branch_is_validated() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let fixture = BareFixture::new();
    fixture.add_branch("develop");

    // A branch the remote has: it becomes the project's head, and the bare
    // repository's `HEAD` names it rather than the remote's own default.
    let chosen = id_of(
        &create(
            &app,
            &user,
            &json!({
                "name": "mars",
                "remote_url": fixture.url(),
                "default_branch": "develop",
            }),
        )
        .await,
    );

    let project = ready(&app, chosen).await;
    assert_eq!(project.default_branch.as_deref(), Some("develop"));
    assert_eq!(head_of(&app, chosen), "refs/heads/develop");

    // A branch it does not have: the clone job is what validates the name, and
    // it says which one it could not find (`src/projects/clone_job.rs`).
    let missing = id_of(
        &create(
            &app,
            &user,
            &json!({
                "name": "phobos",
                "remote_url": fixture.url(),
                "default_branch": "nope",
            }),
        )
        .await,
    );

    let failed = clone_job::wait_for_clone(&app.state, missing, CLONE_TIMEOUT).await;
    assert_eq!(failed.status, ProjectStatus::Error);
    assert_eq!(
        failed.status_message.as_deref(),
        Some("default branch \"nope\" not found on remote")
    );
}

// ---- fetch ----

#[tokio::test]
async fn fetch_refreshes_upstream_without_moving_heads() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let fixture = BareFixture::new();
    let id = ready_project(&app, &user, "mars", &fixture).await;

    let before = branches(&app, &user, id).await;
    let cloned_at = fetched_at(&read(&app, &user, id).await);

    // Upstream moves on under the mirror.
    let moved = fixture.add_commit("main", "next.txt");
    assert_ne!(before["origin/main"], moved);

    app.post_as(&user, &format!("/api/projects/{id}/fetch"))
        .await
        .assert_status_ok();

    // The upstream-tracking ref advanced and Mars's integration head stayed
    // where it was (`ARCHITECTURE.md`, "Git model", Ref ownership).
    let after = branches(&app, &user, id).await;
    assert_eq!(after["origin/main"], moved);
    assert_eq!(after["main"], before["main"]);

    let refreshed = read(&app, &user, id).await;
    assert_eq!(refreshed["status"], json!("ready"));
    assert!(
        fetched_at(&refreshed) > cloned_at,
        "an explicit fetch records itself: {cloned_at} -> {refreshed}"
    );
}

// ---- delete ----

/// The ids a deleted project's rows are found by afterwards.
///
/// The tables reached from the project id alone can be counted directly; the
/// rest hang off a session, a profile, a task or a secret, all of which are
/// gone once the project is. Counting those through a join would report zero
/// whether or not the child rows survived, so the ids are captured while the
/// project is still there and the counts are taken against them.
struct Owned {
    project: Uuid,
    sessions: Vec<Uuid>,
    profiles: Vec<Uuid>,
    tasks: Vec<Uuid>,
    secrets: Vec<Uuid>,
}

/// How many rows every table still holds for this project.
///
/// Runtime `query_scalar` rather than the macro: the table and the column are
/// what varies, and both are this file's own constants, never anything a
/// request carried.
async fn rows_left(pool: &PgPool, owned: &Owned) -> Vec<(&'static str, i64)> {
    let project = std::slice::from_ref(&owned.project);
    let tables: [(&'static str, &'static str, &[Uuid]); 13] = [
        ("projects", "id", project),
        ("agent_profiles", "project_id", project),
        ("project_shared_dirs", "project_id", project),
        ("sessions", "project_id", project),
        ("task_states", "project_id", project),
        ("tasks", "project_id", project),
        ("secrets", "scope_id", project),
        ("events", "session_id", &owned.sessions),
        ("profile_states", "profile_id", &owned.profiles),
        ("task_dependencies", "task_id", &owned.tasks),
        ("task_comments", "task_id", &owned.tasks),
        ("secret_uses", "secret_id", &owned.secrets),
        ("task_events", "project_id", project),
    ];

    let mut counts = Vec::with_capacity(tables.len());
    for (table, column, ids) in tables {
        // `AssertSqlSafe` is the audit sqlx asks for: both halves of the
        // statement come from the table above and never from a request.
        let count: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT COUNT(*) FROM {table} WHERE {column} = ANY($1)"
        )))
        .bind(ids)
        .fetch_one(pool)
        .await
        .expect("the count reads");

        counts.push((table, count));
    }

    counts
}

/// Not one row is left anywhere.
fn assert_every_table_empty(counts: &[(&'static str, i64)]) {
    for (table, count) in counts {
        assert_eq!(*count, 0, "{table} still holds rows of the deleted project");
    }
}

/// A session of `project_id` in `state`, with an event and a populated work
/// directory.
///
/// A session starts `creating`; anything further is reached through the
/// repository's own transitions, so the row is one the lifecycle could really
/// have produced.
async fn session_in(app: &TestApp, project_id: Uuid, state: SessionState) -> Uuid {
    let profile = ProjectRepository::new(&app.state.pool)
        .find_default_profile(project_id)
        .await
        .expect("the profile reads")
        .expect("a created project has a default profile");

    let sessions = SessionRepository::new(&app.state.pool);
    let new_session = NewSession::new(
        project_id,
        profile.id,
        ProfileKind::Conversational,
        "main",
        // Not a credential: a fake stand-in for the hashed MCP token (rule 3).
        format!("fake-mcp-token-hash-{}", Uuid::new_v4()),
    );

    let mut tx = app.state.pool.begin().await.expect("a transaction begins");
    let session = sessions
        .insert(&mut tx, &new_session)
        .await
        .expect("the session inserts");
    if state != SessionState::Creating {
        sessions
            .set_state(
                &mut tx,
                session.id,
                SessionState::Running,
                &StateChange::plain(),
            )
            .await
            .expect("a created session may start running");

        if state != SessionState::Running {
            sessions
                .set_state(&mut tx, session.id, state, &StateChange::plain())
                .await
                .expect("the transition is a legal one");
        }
    }
    sessions
        .append_events(
            &mut tx,
            session.id,
            &[NewEvent::now("status", json!({ "state": "seeded" }))],
        )
        .await
        .expect("the event appends");
    tx.commit().await.expect("the transaction commits");

    let work = DataPaths::from_config(&app.state.config).session_work(session.id);
    tokio::fs::create_dir_all(&work)
        .await
        .expect("the session work directory is created");
    tokio::fs::write(work.join("build.log"), "a session left something behind\n")
        .await
        .expect("the work directory is populated");

    session.id
}

/// Everything a `ready` project can own, through the documented endpoints
/// where they exist and the repositories where they do not.
///
/// A second agent profile, a shared directory with a file in it, a second
/// project secret with an audit row, two tasks with a dependency and a
/// comment, and a `parked` session with an event and a populated directory.
async fn seed_everything(app: &TestApp, user: &AuthenticatedUser, project_id: Uuid) -> Owned {
    let pool = &app.state.pool;

    app.post_as(user, &format!("/api/projects/{project_id}/profiles"))
        .json(&json!({ "name": "archivist" }))
        .await
        .assert_status(StatusCode::CREATED);

    app.post_as(user, &format!("/api/projects/{project_id}/shared-dirs"))
        .json(&json!({ "name": "target", "container_path": "/session/work/target" }))
        .await
        .assert_status(StatusCode::CREATED);
    let layout = app.state.config.project_layout(project_id);
    layout
        .ensure_shared_dir("target")
        .await
        .expect("the shared directory is created");
    let shared = layout.shared_dir("target").expect("the name is a good one");
    tokio::fs::write(shared.join("artifact.bin"), "fake build output\n")
        .await
        .expect("the shared directory is populated");

    app.post_as(user, "/api/secrets")
        .json(&json!({
            "scope": "project",
            "scope_id": project_id,
            "name": EXTRA_SECRET_NAME,
            "value": EXTRA_SECRET_VALUE,
        }))
        .await
        .assert_status(StatusCode::CREATED);

    let session_id = session_in(app, project_id, SessionState::Parked).await;

    let tasks = TaskRepository::new(pool);
    let mut mutation = TrackerMutation::begin(
        pool,
        project_id,
        TaskActor::User {
            user_id: user.user.id,
        },
    )
    .await
    .expect("the mutation opens");
    let blocker = tasks
        .insert_task(
            mutation.conn(),
            project_id,
            &NewTask::new(project_id, "the blocker").expect("the title parses"),
        )
        .await
        .expect("the task inserts");
    let blocked = tasks
        .insert_task(
            mutation.conn(),
            project_id,
            &NewTask::new(project_id, "the blocked one").expect("the title parses"),
        )
        .await
        .expect("the task inserts");
    tasks
        .insert_dependency(
            mutation.conn(),
            project_id,
            blocked.id,
            blocker.id,
            TaskDependencyKind::Blocks,
        )
        .await
        .expect("the dependency inserts");
    tasks
        .insert_comment(
            mutation.conn(),
            project_id,
            &NewTaskComment::from_user(blocker.id, user.user.id, "implemented and pushed"),
        )
        .await
        .expect("the comment inserts");
    // The board's own notification rows, which hang off the project rather
    // than off any task (`SPEC.md`, "TaskEvent").
    mutation
        .emit_states_changed(&[])
        .expect("the board event is emitted");
    mutation.commit().await.expect("the mutation commits");

    // The `GIT_CREDENTIAL` the project was created with and the extra secret.
    let secrets: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM secrets WHERE scope = 'project' AND scope_id = $1")
            .bind(project_id)
            .fetch_all(pool)
            .await
            .expect("the project's secrets read");
    assert_eq!(secrets.len(), 2, "the credential and the extra secret");

    // Attributed to the user and to no session, so the only thing that can
    // remove it is the secret going with the project.
    let mut tx = pool.begin().await.expect("a transaction begins");
    SecretRepository::new(pool)
        .insert_use(
            &mut tx,
            secrets[0],
            None,
            Some(user.user.id),
            SecretUsePurpose::Git,
        )
        .await
        .expect("the use records");
    tx.commit().await.expect("the transaction commits");

    let profiles: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM agent_profiles WHERE project_id = $1")
            .bind(project_id)
            .fetch_all(pool)
            .await
            .expect("the project's profiles read");

    Owned {
        project: project_id,
        sessions: vec![session_id],
        profiles,
        tasks: vec![blocker.id, blocked.id],
        secrets,
    }
}

/// A `ready` project created with a credential, so it starts with the
/// project-scoped orchestrator-only `GIT_CREDENTIAL` secret.
async fn credentialled_project(
    app: &TestApp,
    user: &AuthenticatedUser,
    fixture: &BareFixture,
) -> Uuid {
    let created = create(
        app,
        user,
        &json!({
            "name": "mars",
            "remote_url": fixture.url(),
            "credential": FAKE_CREDENTIAL,
        }),
    )
    .await;
    assert_eq!(created["has_credential"], json!(true));

    let id = id_of(&created);
    ready(app, id).await;

    id
}

#[tokio::test]
async fn delete_removes_rows_and_disk() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let fixture = BareFixture::new();
    let id = credentialled_project(&app, &user, &fixture).await;

    let owned = seed_everything(&app, &user, id).await;
    for (table, count) in rows_left(&app.state.pool, &owned).await {
        assert!(count > 0, "{table} was not seeded");
    }

    let paths = DataPaths::from_config(&app.state.config);
    let project_dir = paths.project_dir(id);
    let session_dir = paths.session_dir(owned.sessions[0]);
    assert!(project_dir.is_dir());
    assert!(session_dir.is_dir());

    app.delete_as(&user, &project_path(id))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    assert_every_table_empty(&rows_left(&app.state.pool, &owned).await);
    assert!(
        !project_dir.exists(),
        "the mirror, the CLI state directory and the shared directories go with the project"
    );
    assert!(
        !session_dir.exists(),
        "each former session's directory goes too"
    );

    app.get_as(&user, &project_path(id))
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn delete_refused_while_session_running() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let fixture = BareFixture::new();
    let id = credentialled_project(&app, &user, &fixture).await;

    let owned = seed_everything(&app, &user, id).await;
    let paths = DataPaths::from_config(&app.state.config);
    let project_dir = paths.project_dir(id);

    let live = session_in(&app, id, SessionState::Running).await;

    let refused = app.delete_as(&user, &project_path(id)).await;
    refused.assert_status(StatusCode::CONFLICT);
    refused.assert_json(&json!({ "status": 409, "error": "project has running sessions" }));
    assert!(project_dir.is_dir(), "a refused deletion removes nothing");
    assert!(paths.session_dir(live).is_dir());

    // The run has ended and its container is gone, so there is nothing left to
    // refuse for (`SPEC.md`, "Projects").
    let mut tx = app.state.pool.begin().await.expect("a transaction begins");
    SessionRepository::new(&app.state.pool)
        .set_state(&mut tx, live, SessionState::Done, &StateChange::plain())
        .await
        .expect("the session ends");
    tx.commit().await.expect("the transaction commits");

    app.delete_as(&user, &project_path(id))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    assert_every_table_empty(&rows_left(&app.state.pool, &owned).await);
    assert!(!project_dir.exists());
    assert!(!paths.session_dir(live).exists());
}

// ---- authentication ----

#[tokio::test]
async fn unauthenticated_requests_are_rejected() {
    let app = TestApp::spawn().await;
    let pid = Uuid::new_v4();
    let id = Uuid::new_v4();
    let project = project_path(pid);
    let dirs = format!("/api/projects/{pid}/shared-dirs");
    let profiles = format!("/api/projects/{pid}/profiles");

    // Every row of the three tables in `SPEC.md`: "Projects" (8), "Shared
    // directories" (4) and "Agent profiles" (5).
    let responses = [
        app.server.get("/api/projects").await,
        app.server.post("/api/projects").json(&json!({})).await,
        app.server.get(&project).await,
        app.server.put(&project).json(&json!({})).await,
        app.server.delete(&project).await,
        app.server
            .post(&format!("/api/projects/{pid}/retry-clone"))
            .await,
        app.server.post(&format!("/api/projects/{pid}/fetch")).await,
        app.server
            .get(&format!("/api/projects/{pid}/branches"))
            .await,
        app.server.get(&dirs).await,
        app.server.post(&dirs).json(&json!({})).await,
        app.server.post(&format!("{dirs}/target/clear")).await,
        app.server.delete(&format!("{dirs}/target")).await,
        app.server.get(&profiles).await,
        app.server.post(&profiles).json(&json!({})).await,
        app.server.get(&format!("{profiles}/{id}")).await,
        app.server
            .put(&format!("{profiles}/{id}"))
            .json(&json!({}))
            .await,
        app.server.delete(&format!("{profiles}/{id}")).await,
    ];

    assert_eq!(responses.len(), 17, "one request per documented route");
    for response in responses {
        response.assert_status(StatusCode::UNAUTHORIZED);
        response.assert_json(&json!({ "status": 401, "error": "authentication required" }));
    }
}
