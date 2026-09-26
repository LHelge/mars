//! The project route suites' shared arrangement and assertions: signing in,
//! creating a project over HTTP and waiting for its clone, a session row in a
//! chosen state, and the per-table row counts a deletion test compares before
//! and after (`tests/projects.rs`, `tests/projects_clone_job.rs`,
//! `tests/profiles.rs`, `tests/shared_dirs.rs`, `tests/project_lifecycle.rs`).
//!
//! Every remote a `ready` project needs is a [`BareFixture`] (git is never
//! mocked, `CLAUDE.md`, "Testing expectations"), and every wait goes through
//! [`clone_job::wait_for_clone`] with [`CLONE_TIMEOUT`]; nothing here sleeps
//! for a fixed time.
//!
//! Every credential, password and identity is an obviously fake stand-in
//! (rule 3). This module may `expect`: an arrangement that cannot be made has
//! nothing to return, and the panic names the step that failed.

use std::time::Duration;

use axum::http::StatusCode;
use axum_test::TestResponse;
use chrono::{DateTime, FixedOffset};
use mars_orchestrator::git::DataPaths;
use mars_orchestrator::models::{
    NewEvent, NewSession, ProfileKind, Project, ProjectStatus, SessionState, StateChange,
};
use mars_orchestrator::projects::clone_job;
use mars_orchestrator::repositories::{ProjectRepository, SessionRepository};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use super::git::BareFixture;
use super::{AuthenticatedUser, TestApp};

/// How long a clone of a local fixture repository may take before a test calls
/// it stuck. Generous: a loaded CI machine runs a dozen git processes for it.
pub const CLONE_TIMEOUT: Duration = Duration::from_secs(30);

/// Not a real remote: `.invalid` can never resolve (rule 3).
pub const TEST_REMOTE: &str = "https://example.invalid/org/repo.git";

/// A remote nothing listens on, so a clone or a fetch against it fails at once
/// rather than on a network timeout.
pub const UNREACHABLE_REMOTE: &str = "https://127.0.0.1:1/x.git";

/// Not a real credential: an obviously fake stand-in (rule 3).
pub const FAKE_CREDENTIAL: &str = "fake-git-credential-for-tests";

// ---- users and the documented refusals ----

/// An obviously fake password of the length `POST /api/test/users` requires
/// (rule 3).
pub fn password(name: &str) -> String {
    format!("fake-password-{name}")
}

/// A signed-in ordinary user to make requests as.
pub async fn signed_in(app: &TestApp, name: &str) -> AuthenticatedUser {
    app.create_user(name, &format!("{name}@example.test"), &password(name))
        .await
}

/// The documented 401 body (`SPEC.md`, "Authentication").
pub fn unauthorized() -> Value {
    json!({ "status": 401, "error": "authentication required" })
}

/// The documented body of the password-change gate (`SPEC.md`,
/// "Authentication").
pub fn password_change_required() -> Value {
    json!({ "status": 403, "error": "password change required" })
}

// ---- creating a project ----

/// `/api/projects/{id}`.
pub fn project_path(id: Uuid) -> String {
    format!("/api/projects/{id}")
}

/// The minimal create body: a name and a remote.
pub fn new_project(name: &str, remote_url: &str) -> Value {
    json!({ "name": name, "remote_url": remote_url })
}

/// The id of a resource the API answered with: a project, a profile, anything
/// that carries its `id` as a UUID string.
pub fn id_of(resource: &Value) -> Uuid {
    resource["id"]
        .as_str()
        .expect("the resource carries an id")
        .parse()
        .expect("the id is a uuid")
}

/// `POST /api/projects` with `body`, as `user`.
///
/// The raw response, for the tests whose subject is a *rejection*; [`created`]
/// is the success form.
pub async fn create(app: &TestApp, user: &AuthenticatedUser, body: &Value) -> TestResponse {
    app.post_as(user, "/api/projects").json(body).await
}

/// `POST /api/projects` for a project that must be created, asserting the
/// documented 201 and the `cloning` status every create starts in.
pub async fn created(app: &TestApp, user: &AuthenticatedUser, body: &Value) -> Value {
    let response = create(app, user, body).await;

    response.assert_status(StatusCode::CREATED);
    let project = response.json::<Value>();
    assert_eq!(project["status"], json!("cloning"), "{project}");

    project
}

/// A project to hang configuration on, created through its own endpoint so it
/// is seeded exactly as a real one is (the default states and the seeded
/// profiles of `SPEC.md`, "Role profile templates").
///
/// Its remote can never resolve, so its background clone fails quickly;
/// nothing that uses this helper waits for it or depends on its status.
pub async fn project(app: &TestApp, user: &AuthenticatedUser, name: &str) -> Uuid {
    id_of(&created(app, user, &new_project(name, TEST_REMOTE)).await)
}

/// Wait for the clone job and assert it ended in `ready`, naming the status
/// message if it did not.
pub async fn ready(app: &TestApp, id: Uuid) -> Project {
    let project = clone_job::wait_for_clone(&app.state, id, CLONE_TIMEOUT).await;
    assert_eq!(
        project.status,
        ProjectStatus::Ready,
        "{:?}",
        project.status_message
    );

    project
}

/// Create a project over HTTP against `fixture` and wait for its clone.
///
/// Returns the created project's id and the row the job left behind, asserting
/// that it really did become `ready`: every test that needs a project with a
/// repository on disk starts here.
pub async fn cloned_project(
    app: &TestApp,
    user: &AuthenticatedUser,
    name: &str,
    fixture: &BareFixture,
) -> (Uuid, Project) {
    let id = id_of(&created(app, user, &new_project(name, &fixture.url())).await);

    (id, ready(app, id).await)
}

/// A `ready` project named `mars` created with a credential, so it starts with
/// the project-scoped, orchestrator-only `GIT_CREDENTIAL` secret.
///
/// The mock credential provider is what the clone actually uses, so storing
/// one changes nothing about the clone; it is there because the deletion tests
/// assert that the secret goes with the project.
pub async fn credentialled_project(
    app: &TestApp,
    user: &AuthenticatedUser,
    fixture: &BareFixture,
) -> Uuid {
    let created = created(
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

// ---- reading a project back ----

/// A project's `last_fetched_at`, which every `ready` project has.
pub fn fetched_at(project: &Value) -> DateTime<FixedOffset> {
    let raw = project["last_fetched_at"]
        .as_str()
        .expect("a fetched project carries a timestamp");

    DateTime::parse_from_rfc3339(raw).expect("the timestamp is RFC 3339")
}

/// The ref the project repository's bare `HEAD` names, `refs/heads/...`.
pub fn head_of(app: &TestApp, id: Uuid) -> String {
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

// ---- sessions ----

/// A session of `project_id` on its default profile in `state`, with an event
/// and a populated work directory.
///
/// A session starts `creating`; anything further is reached through the
/// repository's own transitions (`creating → running → <state>`), so the row
/// is one the lifecycle could really have produced. The MCP token hash is an
/// obviously fake stand-in (rule 3).
pub async fn session_in(app: &TestApp, project_id: Uuid, state: SessionState) -> Uuid {
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
        format!("fake-mcp-token-hash-{}", Uuid::new_v4()),
    );

    let mut tx = app.state.pool.begin().await.expect("a transaction begins");
    let session = sessions
        .insert(&mut tx, &new_session)
        .await
        .expect("the session inserts");
    // Everything but `creating` is reached through `running`, which is the
    // only edge out of the state a session is inserted in.
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

// ---- what a deletion leaves behind ----

/// The ids a deleted project's rows are found by afterwards.
///
/// The tables reached from the project id alone can be counted directly; the
/// rest hang off a session, a profile, a task or a secret, all of which are
/// gone once the project is. Counting those through a join would report zero
/// whether or not the child rows survived, so the ids are captured while the
/// project is still there and the counts are taken against them.
pub struct Owned {
    pub project: Uuid,
    pub sessions: Vec<Uuid>,
    pub profiles: Vec<Uuid>,
    pub tasks: Vec<Uuid>,
    pub secrets: Vec<Uuid>,
}

/// How many rows every table a project owns still holds for it.
///
/// Runtime `query_scalar` rather than the macro: the table and the column are
/// what varies, and both are this function's own constants, never anything a
/// request carried.
pub async fn rows_left(pool: &PgPool, owned: &Owned) -> Vec<(&'static str, i64)> {
    let project = std::slice::from_ref(&owned.project);
    let tables: [(&'static str, &'static str, &[Uuid]); 14] = [
        ("projects", "id", project),
        ("agent_profiles", "project_id", project),
        ("project_shared_dirs", "project_id", project),
        ("sessions", "project_id", project),
        ("task_states", "project_id", project),
        ("tasks", "project_id", project),
        ("task_events", "project_id", project),
        ("secrets", "scope_id", project),
        ("events", "session_id", &owned.sessions),
        ("profile_states", "profile_id", &owned.profiles),
        ("task_dependencies", "task_id", &owned.tasks),
        ("task_comments", "task_id", &owned.tasks),
        ("task_handoffs", "task_id", &owned.tasks),
        ("secret_uses", "secret_id", &owned.secrets),
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

/// Every table [`rows_left`] counts holds something, which is what makes the
/// assertion after the deletion mean anything.
pub fn assert_every_table_seeded(counts: &[(&'static str, i64)]) {
    for (table, count) in counts {
        assert!(*count > 0, "{table} was not seeded");
    }
}

/// Not one row is left anywhere.
pub fn assert_every_table_empty(counts: &[(&'static str, i64)]) {
    for (table, count) in counts {
        assert_eq!(*count, 0, "{table} still holds rows of the deleted project");
    }
}
