//! `/api/projects` through the real router (`SPEC.md`, "Projects
//! (`/api/projects`)").
//!
//! The endpoints of the projects table itself: the list, the create, the read,
//! the edit, the clone retry, the on-demand mirror fetch and the branch
//! listing. What the creation transaction writes is
//! asserted in `tests/projects_create.rs` and what the background job does in
//! `tests/projects_clone_job.rs`; what is asserted here is the adapter — the
//! paths, the JWT requirement and the password-change gate on each of them, the
//! status of every success and of every documented failure, and the exact
//! response shape.
//!
//! Git is never mocked (`CLAUDE.md`, "Testing expectations"), so the tests that
//! need a `ready` project point it at a real bare repository in a `tempfile`
//! directory through the `file://` form [`RemoteUrl`] accepts under the
//! `integration-tests` feature, and wait for the real clone job through
//! [`clone_job::wait_for_clone`]. Nothing here sleeps for a fixed time.
//!
//! Every credential is an obviously fake stand-in (rule 3), and no response is
//! ever allowed to carry one back.
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use axum::http::StatusCode;
use axum_test::TestResponse;
use chrono::{DateTime, FixedOffset};
use common::{AuthenticatedUser, TestApp};
use mars_orchestrator::git::testutil::{TestUpstream, run_git};
use mars_orchestrator::git::{DataPaths, GitActor};
use mars_orchestrator::models::{Project, ProjectStatus};
use mars_orchestrator::projects::{NewProjectRequest, clone_job, create_project};
use serde_json::{Value, json};
use uuid::Uuid;

/// How long a clone of a two-commit local repository may take before the test
/// calls it stuck. Generous: a loaded CI machine runs a dozen git processes for
/// it.
const CLONE_TIMEOUT: Duration = Duration::from_secs(30);

/// Not a real remote: `.invalid` can never resolve (rule 3).
const TEST_REMOTE: &str = "https://example.invalid/org/repo.git";

/// A remote nothing listens on, so the clone job fails at once. The tests that
/// need a project in `error` use it.
const UNREACHABLE_REMOTE: &str = "https://127.0.0.1:1/x.git";

/// Not a real credential: an obviously fake stand-in (rule 3).
const FAKE_CREDENTIAL: &str = "fake-git-credential-for-tests";

/// The fixed name a project's git credential is stored under.
const GIT_CREDENTIAL: &str = "GIT_CREDENTIAL";

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

/// The documented body of the password-change gate (`SPEC.md`,
/// "Authentication").
fn password_change_required() -> Value {
    json!({ "status": 403, "error": "password change required" })
}

/// `POST /api/projects` with `body`, as `user`.
///
/// The raw response, because half of what these tests assert is the status and
/// the message of a *rejection*; [`created`] is the success form.
async fn create(app: &TestApp, user: &AuthenticatedUser, body: &Value) -> TestResponse {
    app.post_as(user, "/api/projects").json(body).await
}

/// `POST /api/projects` for a project that must be created, asserting 201.
///
/// The arrangement step of every test that is about what happens *after* a
/// create.
async fn created(app: &TestApp, user: &AuthenticatedUser, body: &Value) -> Value {
    let response = create(app, user, body).await;

    response.assert_status(StatusCode::CREATED);
    response.json::<Value>()
}

/// The minimal create body: a name and a remote.
fn new_project(name: &str, remote_url: &str) -> Value {
    json!({ "name": name, "remote_url": remote_url })
}

/// The id of a project the API answered with.
fn id_of(project: &Value) -> Uuid {
    project["id"]
        .as_str()
        .expect("a project carries an id")
        .parse()
        .expect("the id is a uuid")
}

/// `/api/projects/{id}`.
fn project_path(id: Uuid) -> String {
    format!("/api/projects/{id}")
}

/// `/api/projects/{id}/retry-clone`.
fn retry_path(id: Uuid) -> String {
    format!("/api/projects/{id}/retry-clone")
}

/// The `file://` URL of a local bare repository, which is the remote form
/// `RemoteUrl::parse` accepts under the `integration-tests` feature.
fn file_url(path: &Path) -> String {
    format!("file://{}", path.display())
}

/// A bare upstream with one commit on `main` at exactly `path`.
///
/// [`TestUpstream`] owns its own temporary directory, and the concurrency test
/// below needs a remote that does *not* exist when the project is created and
/// appears afterwards, so the path has to be one the test chose.
async fn bare_upstream_at(path: &Path) {
    let parent = path.parent().expect("the upstream path has a parent");
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .expect("the upstream directory is named");

    run_git(
        parent,
        &["init", "--bare", "--quiet", "--initial-branch=main", name],
    )
    .await;
    run_git(parent, &["clone", "--quiet", name, "work"]).await;

    let work = parent.join("work");
    run_git(&work, &["symbolic-ref", "HEAD", "refs/heads/main"]).await;
    std::fs::write(work.join("README.md"), "# fixture\n").expect("the fixture file is written");
    run_git(&work, &["add", "--", "README.md"]).await;
    run_git(&work, &["commit", "--quiet", "-m", "chore: add a readme"]).await;
    run_git(
        &work,
        &["push", "--quiet", "origin", "HEAD:refs/heads/main"],
    )
    .await;

    std::fs::remove_dir_all(&work).expect("the throwaway work clone is removed");
}

/// A bare upstream with `main` and one extra branch per name in `extra`.
///
/// [`TestUpstream::create`] gives `main` with two commits and a symbolic `HEAD`
/// naming it; each extra branch is one more commit pushed to the same bare
/// repository, which is what makes it an integration head after the clone.
async fn upstream_with(extra: &[&str]) -> TestUpstream {
    let upstream = TestUpstream::create().await;

    for (index, branch) in extra.iter().enumerate() {
        upstream
            .commit_file(
                branch,
                &format!("extra-{index}.txt"),
                "an extra branch\n",
                "feat: another branch",
            )
            .await;
    }

    upstream
}

/// Create a project over HTTP against `upstream` and wait for its clone.
///
/// Returns the created project's id and the row the job left behind, asserting
/// that it really did become `ready`: every test that needs a project with a
/// repository on disk starts here.
async fn cloned_project(
    app: &TestApp,
    user: &AuthenticatedUser,
    name: &str,
    upstream: &TestUpstream,
) -> (Uuid, Project) {
    let created = created(app, user, &new_project(name, &file_url(&upstream.path))).await;
    let id = id_of(&created);

    let project = clone_job::wait_for_clone(&app.state, id, CLONE_TIMEOUT).await;
    assert_eq!(
        project.status,
        ProjectStatus::Ready,
        "{:?}",
        project.status_message
    );

    (id, project)
}

/// Create a project whose clone is bound to fail, and wait for it to.
///
/// The precondition of `retry-clone`: the remote is a port nothing listens on,
/// so the job reaches `error` in one failed connection rather than a timeout.
async fn failed_project(app: &TestApp, user: &AuthenticatedUser, name: &str) -> Uuid {
    let created = created(app, user, &new_project(name, UNREACHABLE_REMOTE)).await;
    let id = id_of(&created);

    let project = clone_job::wait_for_clone(&app.state, id, CLONE_TIMEOUT).await;
    assert_eq!(project.status, ProjectStatus::Error);

    id
}

/// The branch the project repository's symbolic `HEAD` names.
async fn head_branch(app: &TestApp, id: Uuid) -> String {
    let repo = DataPaths::from_config(&app.state.config).project_repo(id);

    run_git(&repo, &["symbolic-ref", "--end-of-options", "HEAD"])
        .await
        .trim()
        .to_string()
}

// ---- authentication ----

#[tokio::test]
async fn every_endpoint_requires_a_token() {
    let app = TestApp::spawn().await;
    let id = Uuid::new_v4();

    let responses = [
        app.server.get("/api/projects").await,
        app.server
            .post("/api/projects")
            .json(&new_project("mars", TEST_REMOTE))
            .await,
        app.server.get(&project_path(id)).await,
        app.server.put(&project_path(id)).json(&json!({})).await,
        app.server.post(&retry_path(id)).await,
    ];

    for response in responses {
        response.assert_status(StatusCode::UNAUTHORIZED);
        response.assert_json(&unauthorized());
    }
}

#[tokio::test]
async fn every_endpoint_is_refused_while_a_password_change_is_pending() {
    let app = TestApp::spawn().await;
    let gated = app.create_gated_user("gated", "gated@example.test").await;
    let id = Uuid::new_v4();

    let responses = [
        app.get_as(&gated, "/api/projects").await,
        app.post_as(&gated, "/api/projects")
            .json(&new_project("mars", TEST_REMOTE))
            .await,
        app.get_as(&gated, &project_path(id)).await,
        app.put_as(&gated, &project_path(id)).json(&json!({})).await,
        app.post_as(&gated, &retry_path(id)).await,
    ];

    for response in responses {
        response.assert_status(StatusCode::FORBIDDEN);
        response.assert_json(&password_change_required());
    }
}

// ---- create ----

#[tokio::test]
async fn creating_a_project_answers_the_documented_shape_in_cloning() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;

    let response = create(&app, &user, &new_project("mars", TEST_REMOTE)).await;

    response.assert_status(StatusCode::CREATED);
    let project = response.json::<Value>();

    assert_eq!(project["name"], json!("mars"));
    assert_eq!(project["remote_url"], json!(TEST_REMOTE));
    assert_eq!(project["status"], json!("cloning"));
    assert_eq!(project["status_message"], Value::Null);
    assert_eq!(project["default_branch"], Value::Null);
    assert_eq!(project["last_fetched_at"], Value::Null);
    assert_eq!(project["max_attempts"], json!(3));
    assert_eq!(project["max_rounds"], json!(5));
    // Automation is off and uncapped until somebody says otherwise (ADR 0042).
    assert_eq!(project["max_concurrent_sessions"], Value::Null);
    assert_eq!(project["automation_paused"], json!(false));
    assert_eq!(project["has_credential"], json!(false));
    assert!(project["created_at"].is_string());

    // Exactly the thirteen documented fields, and none of the three the row
    // adds.
    let mut keys: Vec<&str> = project
        .as_object()
        .expect("a project is an object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "automation_paused",
            "created_at",
            "default_branch",
            "has_credential",
            "id",
            "last_fetched_at",
            "max_attempts",
            "max_concurrent_sessions",
            "max_rounds",
            "name",
            "remote_url",
            "status",
            "status_message",
        ]
    );
}

#[tokio::test]
async fn a_supplied_credential_is_stored_and_never_returned() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;

    let response = create(
        &app,
        &user,
        &json!({
            "name": "mars",
            "remote_url": TEST_REMOTE,
            "credential": FAKE_CREDENTIAL,
        }),
    )
    .await;

    response.assert_status(StatusCode::CREATED);
    let project = response.json::<Value>();
    assert_eq!(project["has_credential"], json!(true));
    assert!(
        !response.text().contains(FAKE_CREDENTIAL),
        "the credential came back in the response"
    );

    // It is the project-scoped, orchestrator-only `GIT_CREDENTIAL` secret, and
    // the secrets endpoint shows its metadata and no value (`SPEC.md`,
    // "Secrets").
    let id = id_of(&project);
    let listed = app
        .get_as(&user, &format!("/api/secrets?scope=project&scope_id={id}"))
        .await;
    listed.assert_status_ok();

    let secrets = listed.json::<Value>();
    let secrets = secrets.as_array().expect("the listing is an array");
    assert_eq!(secrets.len(), 1, "{secrets:?}");
    assert_eq!(secrets[0]["name"], json!(GIT_CREDENTIAL));
    assert_eq!(secrets[0]["scope"], json!("project"));
    assert_eq!(secrets[0]["scope_id"], json!(id));
    assert_eq!(secrets[0]["orchestrator_only"], json!(true));
    assert_eq!(secrets[0]["value"], Value::Null, "a value was returned");
    assert!(!listed.text().contains(FAKE_CREDENTIAL));
}

#[tokio::test]
async fn a_credential_on_a_public_repository_is_simply_stored() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;

    let project = created(
        &app,
        &user,
        &json!({
            "name": "mars",
            "remote_url": TEST_REMOTE,
            "credential": FAKE_CREDENTIAL,
        }),
    )
    .await;

    assert_eq!(project["status"], json!("cloning"));
    assert_eq!(project["has_credential"], json!(true));
}

#[tokio::test]
async fn a_supplied_default_branch_is_stored_as_given() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;

    let project = created(
        &app,
        &user,
        &json!({
            "name": "mars",
            "remote_url": TEST_REMOTE,
            "default_branch": "release/1.2",
        }),
    )
    .await;

    assert_eq!(project["default_branch"], json!("release/1.2"));
    assert_eq!(project["status"], json!("cloning"));
}

#[tokio::test]
async fn every_invalid_create_field_is_a_bad_request() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;

    let cases = [
        json!({ "name": "   ", "remote_url": TEST_REMOTE }),
        json!({ "name": "m".repeat(101), "remote_url": TEST_REMOTE }),
        json!({ "name": "mars", "remote_url": "http://example.invalid/org/repo.git" }),
        json!({
            "name": "mars",
            "remote_url": "https://user:not-a-real-token@example.invalid/org/repo.git",
        }),
        json!({
            "name": "mars",
            "remote_url": TEST_REMOTE,
            "default_branch": "refs/heads/main",
        }),
        json!({ "name": "mars", "remote_url": TEST_REMOTE, "credential": "   " }),
    ];

    for body in cases {
        let response = create(&app, &user, &body).await;

        response.assert_status(StatusCode::BAD_REQUEST);
        assert_eq!(response.json::<Value>()["status"], json!(400), "{body}");
        assert!(
            !response.text().contains("not-a-real-token"),
            "a rejection echoed the credential: {}",
            response.text()
        );
    }

    // Nothing was created by any of them.
    let listed = app.get_as(&user, "/api/projects").await;
    listed.assert_status_ok();
    assert_eq!(
        listed.json::<Value>().as_array().map(Vec::len),
        Some(0),
        "a rejected create left a project behind"
    );
}

#[tokio::test]
async fn a_duplicate_project_name_is_a_conflict() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;

    created(&app, &user, &new_project("mars", UNREACHABLE_REMOTE)).await;

    let response = create(&app, &user, &new_project("mars", UNREACHABLE_REMOTE)).await;

    response.assert_status(StatusCode::CONFLICT);
    response.assert_json(&json!({ "status": 409, "error": "project name already taken" }));
}

// ---- list and read ----

#[tokio::test]
async fn the_list_holds_every_project_oldest_first() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;

    // Named so that alphabetical order and creation order differ.
    let first = id_of(&created(&app, &user, &new_project("phobos", UNREACHABLE_REMOTE)).await);
    let second = id_of(&created(&app, &user, &new_project("deimos", UNREACHABLE_REMOTE)).await);

    let response = app.get_as(&user, "/api/projects").await;

    response.assert_status_ok();
    let listed = response.json::<Value>();
    let listed = listed.as_array().expect("the listing is an array");

    assert_eq!(
        listed.iter().map(id_of).collect::<Vec<_>>(),
        vec![first, second]
    );
    assert!(listed.iter().all(|project| project["id"].is_string()));
}

#[tokio::test]
async fn reading_a_project_answers_it_and_an_unknown_id_is_404() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;

    let created = created(&app, &user, &new_project("mars", UNREACHABLE_REMOTE)).await;
    let id = id_of(&created);

    let response = app.get_as(&user, &project_path(id)).await;
    response.assert_status_ok();
    assert_eq!(response.json::<Value>()["name"], json!("mars"));

    let missing = app.get_as(&user, &project_path(Uuid::new_v4())).await;
    missing.assert_status(StatusCode::NOT_FOUND);
    missing.assert_json(&json!({ "status": 404, "error": "not found" }));
}

#[tokio::test]
async fn a_path_segment_that_is_not_a_uuid_is_a_bad_request() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;

    let response = app.get_as(&user, "/api/projects/not-a-uuid").await;

    response.assert_status(StatusCode::BAD_REQUEST);
    assert_eq!(response.json::<Value>()["status"], json!(400));
}

// ---- update ----

#[tokio::test]
async fn updating_the_name_and_the_attempt_budget_answers_the_stored_row() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let id = id_of(&created(&app, &user, &new_project("mars", UNREACHABLE_REMOTE)).await);

    let response = app
        .put_as(&user, &project_path(id))
        .json(&json!({ "name": "mars-2", "max_attempts": 7 }))
        .await;

    response.assert_status_ok();
    let updated = response.json::<Value>();
    assert_eq!(updated["name"], json!("mars-2"));
    assert_eq!(updated["max_attempts"], json!(7));

    // Committed, not just answered.
    let read = app.get_as(&user, &project_path(id)).await;
    read.assert_status_ok();
    assert_eq!(read.json::<Value>()["name"], json!("mars-2"));
}

#[tokio::test]
async fn an_empty_update_body_answers_the_current_row_unchanged() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let created = created(&app, &user, &new_project("mars", UNREACHABLE_REMOTE)).await;
    let id = id_of(&created);

    let response = app.put_as(&user, &project_path(id)).json(&json!({})).await;

    response.assert_status_ok();
    let answered = response.json::<Value>();
    assert_eq!(answered["id"], created["id"]);
    assert_eq!(answered["name"], created["name"]);
    assert_eq!(answered["max_attempts"], created["max_attempts"]);
    assert_eq!(answered["created_at"], created["created_at"]);
}

#[tokio::test]
async fn an_attempt_budget_outside_the_documented_range_is_a_bad_request() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let id = id_of(&created(&app, &user, &new_project("mars", UNREACHABLE_REMOTE)).await);

    for attempts in [0, 21] {
        let response = app
            .put_as(&user, &project_path(id))
            .json(&json!({ "max_attempts": attempts }))
            .await;

        response.assert_status(StatusCode::BAD_REQUEST);
        response.assert_json(&json!({
            "status": 400,
            "error": "max attempts must be between 1 and 20",
        }));
    }
}

#[tokio::test]
async fn updating_the_round_limit_answers_and_stores_it() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let id = id_of(&created(&app, &user, &new_project("mars", UNREACHABLE_REMOTE)).await);

    for rounds in [1, 50, 12] {
        let response = app
            .put_as(&user, &project_path(id))
            .json(&json!({ "max_rounds": rounds }))
            .await;

        response.assert_status_ok();
        assert_eq!(response.json::<Value>()["max_rounds"], json!(rounds));
    }

    // Committed, not just answered; and an update that does not name it
    // leaves it alone.
    app.put_as(&user, &project_path(id))
        .json(&json!({ "name": "mars-2" }))
        .await
        .assert_status_ok();
    let read = app.get_as(&user, &project_path(id)).await;
    read.assert_status_ok();
    assert_eq!(read.json::<Value>()["max_rounds"], json!(12));
}

#[tokio::test]
async fn a_round_limit_outside_the_documented_range_is_a_bad_request() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let id = id_of(&created(&app, &user, &new_project("mars", UNREACHABLE_REMOTE)).await);

    for rounds in [0, -1, 51] {
        let response = app
            .put_as(&user, &project_path(id))
            .json(&json!({ "max_rounds": rounds }))
            .await;

        response.assert_status(StatusCode::BAD_REQUEST);
        response.assert_json(&json!({
            "status": 400,
            "error": "max_rounds must be between 1 and 50",
        }));
    }

    // Nothing was written by the refusals.
    let read = app.get_as(&user, &project_path(id)).await;
    assert_eq!(read.json::<Value>()["max_rounds"], json!(5));
}

#[tokio::test]
async fn renaming_a_project_to_a_name_that_is_taken_is_a_conflict() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    created(&app, &user, &new_project("mars", UNREACHABLE_REMOTE)).await;
    let id = id_of(&created(&app, &user, &new_project("phobos", UNREACHABLE_REMOTE)).await);

    let response = app
        .put_as(&user, &project_path(id))
        .json(&json!({ "name": "mars" }))
        .await;

    response.assert_status(StatusCode::CONFLICT);
    response.assert_json(&json!({ "status": 409, "error": "project name already taken" }));
}

#[tokio::test]
async fn updating_an_unknown_project_is_404() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;

    let response = app
        .put_as(&user, &project_path(Uuid::new_v4()))
        .json(&json!({ "name": "mars" }))
        .await;

    response.assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn changing_the_default_branch_of_a_ready_project_moves_head() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let upstream = upstream_with(&["release/2.0"]).await;
    let (id, cloned) = cloned_project(&app, &user, "mars", &upstream).await;

    assert_eq!(cloned.default_branch.as_deref(), Some("main"));
    assert_eq!(head_branch(&app, id).await, "refs/heads/main");

    let response = app
        .put_as(&user, &project_path(id))
        .json(&json!({ "default_branch": "release/2.0" }))
        .await;

    response.assert_status_ok();
    assert_eq!(
        response.json::<Value>()["default_branch"],
        json!("release/2.0")
    );
    assert_eq!(head_branch(&app, id).await, "refs/heads/release/2.0");
}

#[tokio::test]
async fn a_default_branch_that_is_not_an_integration_head_is_a_bad_request() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let upstream = upstream_with(&[]).await;
    let (id, _) = cloned_project(&app, &user, "mars", &upstream).await;

    let response = app
        .put_as(&user, &project_path(id))
        .json(&json!({ "default_branch": "nope" }))
        .await;

    response.assert_status(StatusCode::BAD_REQUEST);
    response.assert_json(&json!({
        "status": 400,
        "error": "default_branch \"nope\" is not an integration head of this project",
    }));

    // Neither the row nor `HEAD` moved.
    let read = app.get_as(&user, &project_path(id)).await;
    assert_eq!(read.json::<Value>()["default_branch"], json!("main"));
    assert_eq!(head_branch(&app, id).await, "refs/heads/main");
}

#[tokio::test]
async fn setting_the_default_branch_a_ready_project_already_has_changes_nothing() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let upstream = upstream_with(&[]).await;
    let (id, _) = cloned_project(&app, &user, "mars", &upstream).await;

    let response = app
        .put_as(&user, &project_path(id))
        .json(&json!({ "default_branch": "main", "max_attempts": 5 }))
        .await;

    response.assert_status_ok();
    let updated = response.json::<Value>();
    assert_eq!(updated["default_branch"], json!("main"));
    assert_eq!(updated["max_attempts"], json!(5));
    assert_eq!(head_branch(&app, id).await, "refs/heads/main");
}

#[tokio::test]
async fn a_default_branch_on_a_project_without_a_repository_is_stored_unchecked() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    // Still `cloning`: nothing on disk to check the name against, and the clone
    // job is what validates it (`SPEC.md`, "Projects").
    let id = id_of(&created(&app, &user, &new_project("mars", UNREACHABLE_REMOTE)).await);

    let response = app
        .put_as(&user, &project_path(id))
        .json(&json!({ "default_branch": "release/9.9" }))
        .await;

    response.assert_status_ok();
    assert_eq!(
        response.json::<Value>()["default_branch"],
        json!("release/9.9")
    );
}

#[tokio::test]
async fn a_default_branch_move_whose_row_write_conflicts_leaves_head_alone() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let upstream = upstream_with(&["release/2.0"]).await;
    let (id, _) = cloned_project(&app, &user, "mars", &upstream).await;
    // The name the update below collides with.
    created(&app, &user, &new_project("phobos", UNREACHABLE_REMOTE)).await;

    let response = app
        .put_as(&user, &project_path(id))
        .json(&json!({ "name": "phobos", "default_branch": "release/2.0" }))
        .await;

    response.assert_status(StatusCode::CONFLICT);
    response.assert_json(&json!({ "status": 409, "error": "project name already taken" }));

    // The row write failed, so the repository must not have moved either.
    assert_eq!(head_branch(&app, id).await, "refs/heads/main");
    let read = app.get_as(&user, &project_path(id)).await.json::<Value>();
    assert_eq!(read["name"], json!("mars"));
    assert_eq!(read["default_branch"], json!("main"));
}

// ---- retry-clone ----

#[tokio::test]
async fn retrying_a_failed_clone_answers_cloning_again() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let id = failed_project(&app, &user, "mars").await;

    let response = app.post_as(&user, &retry_path(id)).await;

    response.assert_status_ok();
    let retried = response.json::<Value>();
    assert_eq!(retried["status"], json!("cloning"));
    assert_eq!(retried["status_message"], Value::Null);

    // The job really ran again, and failed again for the same reason.
    let project = clone_job::wait_for_clone(&app.state, id, CLONE_TIMEOUT).await;
    assert_eq!(project.status, ProjectStatus::Error);
    assert!(project.status_message.is_some());
}

#[tokio::test]
async fn retrying_a_project_that_is_not_in_error_is_a_conflict() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let upstream = upstream_with(&[]).await;
    let (id, _) = cloned_project(&app, &user, "mars", &upstream).await;

    let response = app.post_as(&user, &retry_path(id)).await;

    response.assert_status(StatusCode::CONFLICT);
    response.assert_json(&json!({ "status": 409, "error": "project is not in error state" }));
}

#[tokio::test]
async fn only_one_of_two_concurrent_retries_starts_a_clone() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let dir = tempfile::tempdir().expect("a temporary directory");
    let upstream = dir.path().join("upstream.git");

    // The remote does not exist yet, which is the ordinary mistyped-URL case:
    // the first clone fails and leaves the project in `error`.
    let id = id_of(&created(&app, &user, &new_project("mars", &file_url(&upstream))).await);
    let failed = clone_job::wait_for_clone(&app.state, id, CLONE_TIMEOUT).await;
    assert_eq!(failed.status, ProjectStatus::Error);

    // Now it exists, so whichever retry wins moves the project to `ready` and
    // never back to `error`: the loser's guarded `UPDATE` finds `cloning` or
    // `ready` and matches no row, whichever order the two requests interleave
    // in. That is what makes this assertion about the guard rather than about
    // timing.
    bare_upstream_at(&upstream).await;

    let (first, second) = tokio::join!(
        app.post_as(&user, &retry_path(id)),
        app.post_as(&user, &retry_path(id))
    );

    let mut statuses = [first.status_code(), second.status_code()];
    statuses.sort_unstable();
    assert_eq!(
        statuses,
        [StatusCode::OK, StatusCode::CONFLICT],
        "exactly one retry may start a clone"
    );

    let cloned = clone_job::wait_for_clone(&app.state, id, CLONE_TIMEOUT).await;
    assert_eq!(
        cloned.status,
        ProjectStatus::Ready,
        "{:?}",
        cloned.status_message
    );
}

#[tokio::test]
async fn retrying_an_unknown_project_is_404() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;

    let response = app.post_as(&user, &retry_path(Uuid::new_v4())).await;

    response.assert_status(StatusCode::NOT_FOUND);
    response.assert_json(&json!({ "status": 404, "error": "not found" }));
}

// --- POST /projects/{id}/fetch and GET /projects/{id}/branches

/// `/api/projects/{id}/fetch`.
fn fetch_path(id: Uuid) -> String {
    format!("/api/projects/{id}/fetch")
}

/// `/api/projects/{id}/branches`.
fn branches_path(id: Uuid) -> String {
    format!("/api/projects/{id}/branches")
}

/// A project that stays `cloning`, because no job was ever started for it.
///
/// `POST /api/projects` spawns the clone immediately and the row leaves
/// `cloning` within milliseconds, which is no basis for asserting what the two
/// endpoints answer *while* a project is cloning. The creation transaction on
/// its own leaves exactly that row and no repository on disk.
async fn cloning_project(app: &TestApp, user: &AuthenticatedUser, name: &str) -> Uuid {
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

/// The project's bare repository on disk.
fn repo_of(app: &TestApp, id: Uuid) -> PathBuf {
    DataPaths::from_config(&app.state.config).project_repo(id)
}

/// Write `full_name` in the project repository at `commit`.
///
/// How a session ref, a tag or a hand-off ref gets into a mirror here: the
/// epics that create them for real are not in this router yet, and what these
/// tests assert is which namespaces the listing reports.
async fn write_ref(app: &TestApp, id: Uuid, full_name: &str, commit: &str) {
    run_git(
        &repo_of(app, id),
        &["update-ref", "--end-of-options", full_name, commit],
    )
    .await;
}

/// The object id `rev` names in the project repository.
///
/// No `--end-of-options`: `rev-parse` echoes options it does not understand,
/// and every `rev` here is a fully qualified name this test wrote itself.
async fn commit_of(app: &TestApp, id: Uuid, rev: &str) -> String {
    run_git(&repo_of(app, id), &["rev-parse", rev])
        .await
        .trim()
        .to_string()
}

/// A project's `last_fetched_at`, which every project in these tests has.
fn fetched_at(project: &Value) -> DateTime<FixedOffset> {
    let raw = project["last_fetched_at"]
        .as_str()
        .expect("a fetched project carries a timestamp");

    DateTime::parse_from_rfc3339(raw).expect("the timestamp is RFC 3339")
}

/// The listing as `(name, kind)` pairs, in the order it came back in.
fn listed(branches: &Value) -> Vec<(String, String)> {
    branches
        .as_array()
        .expect("the listing is an array")
        .iter()
        .map(|branch| {
            (
                branch["name"].as_str().expect("a branch has a name").into(),
                branch["kind"].as_str().expect("a branch has a kind").into(),
            )
        })
        .collect()
}

#[tokio::test]
async fn the_fetch_and_the_branch_listing_require_a_token() {
    let app = TestApp::spawn().await;
    let id = Uuid::new_v4();

    let responses = [
        app.server.post(&fetch_path(id)).await,
        app.server.get(&branches_path(id)).await,
    ];

    for response in responses {
        response.assert_status(StatusCode::UNAUTHORIZED);
        response.assert_json(&unauthorized());
    }
}

#[tokio::test]
async fn the_fetch_and_the_branch_listing_are_refused_while_a_password_change_is_pending() {
    let app = TestApp::spawn().await;
    let gated = app.create_gated_user("gated", "gated@example.test").await;
    let id = Uuid::new_v4();

    let responses = [
        app.post_as(&gated, &fetch_path(id)).await,
        app.get_as(&gated, &branches_path(id)).await,
    ];

    for response in responses {
        response.assert_status(StatusCode::FORBIDDEN);
        response.assert_json(&password_change_required());
    }
}

#[tokio::test]
async fn fetching_a_ready_project_advances_its_fetch_time_as_the_requesting_user() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let upstream = upstream_with(&[]).await;
    let (id, cloned) = cloned_project(&app, &user, "mars", &upstream).await;

    let before = cloned
        .last_fetched_at
        .expect("the clone that just finished is a fetch");

    let response = app.post_as(&user, &fetch_path(id)).await;

    response.assert_status_ok();
    let project = response.json::<Value>();
    assert_eq!(project["id"], json!(id));
    assert_eq!(project["status"], json!("ready"));
    assert!(
        fetched_at(&project) > before,
        "an explicit fetch must record itself: {before} -> {project}"
    );

    // The credential was asked for on behalf of the user who asked for the
    // fetch, which is what `secret_uses` records (`docs/data-model.md`:
    // `purpose = 'git'`, `user_id` for REST, no session). Asserted through the
    // provider the router holds, which is the mock; the row itself is
    // `tests/projects_clone_job.rs`'s, which swaps the real provider in.
    assert_eq!(
        app.mock_git().requested().last(),
        Some(&(id, GitActor::User(user.user.id))),
        "the fetch asked as somebody else"
    );
}

#[tokio::test]
async fn a_fetch_moves_the_upstream_refs_and_leaves_the_integration_heads_alone() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let upstream = upstream_with(&[]).await;
    let (id, _) = cloned_project(&app, &user, "mars", &upstream).await;

    let head_before = commit_of(&app, id, "refs/heads/main").await;

    // Upstream advances `main` and grows a branch with a slash in its name,
    // which only a prefix ref pattern reaches.
    let moved = upstream
        .commit_file("main", "next.txt", "next\n", "feat: move main on")
        .await;
    upstream
        .commit_file("feature/x", "x.txt", "x\n", "feat: start x")
        .await;

    app.post_as(&user, &fetch_path(id)).await.assert_status_ok();

    let response = app.get_as(&user, &branches_path(id)).await;
    response.assert_status_ok();
    let branches = response.json::<Value>();

    // The fetch moved the tracking ref and left Mars's head where it was
    // (`ARCHITECTURE.md`, "Git model", Ref ownership).
    assert_eq!(commit_of(&app, id, "refs/heads/main").await, head_before);
    assert_eq!(
        commit_of(&app, id, "refs/remotes/origin/main").await,
        moved,
        "the upstream-tracking ref did not move"
    );

    assert_eq!(
        listed(&branches),
        vec![
            ("main".to_string(), "head".to_string()),
            ("origin/feature/x".to_string(), "upstream".to_string()),
            ("origin/main".to_string(), "upstream".to_string()),
        ],
        "a branch upstream grew after the clone is tracked, not seeded as a head"
    );
}

#[tokio::test]
async fn the_branch_listing_is_the_three_kinds_and_leaves_the_other_refs_out() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let upstream = upstream_with(&["feature/x"]).await;
    let (id, _) = cloned_project(&app, &user, "mars", &upstream).await;

    // A session ref, as a fetch-back would leave it, and the two kinds of ref
    // that live in the repository without being branches.
    let session_id = Uuid::new_v4();
    let tip = commit_of(&app, id, "refs/heads/main").await;
    write_ref(&app, id, &format!("refs/sessions/{session_id}"), &tip).await;
    write_ref(&app, id, "refs/tags/v1", &tip).await;
    write_ref(&app, id, &format!("refs/handoffs/{}", Uuid::new_v4()), &tip).await;

    let response = app.get_as(&user, &branches_path(id)).await;

    response.assert_status_ok();
    let branches = response.json::<Value>();
    let session_ref = format!("refs/sessions/{session_id}");

    assert_eq!(
        listed(&branches),
        vec![
            ("feature/x".to_string(), "head".to_string()),
            ("main".to_string(), "head".to_string()),
            ("origin/feature/x".to_string(), "upstream".to_string()),
            ("origin/main".to_string(), "upstream".to_string()),
            (session_ref, "session".to_string()),
        ],
        "heads, then upstream, then sessions, each by name; no tag and no hand-off"
    );

    let entries = branches.as_array().expect("the listing is an array");
    let session = entries.last().expect("the session ref is listed");
    assert_eq!(session["session_id"], json!(session_id));
    assert_eq!(session["commit"], json!(tip));

    // `session_id` is omitted rather than null for everything else, and every
    // entry carries a full object id.
    for entry in &entries[..entries.len() - 1] {
        assert!(
            entry.get("session_id").is_none(),
            "a head or upstream ref carried a session id: {entry}"
        );
    }
    assert!(
        entries
            .iter()
            .all(|entry| entry["commit"].as_str().is_some_and(|id| id.len() == 40)),
        "{branches}"
    );
}

#[tokio::test]
async fn fetching_a_project_that_is_still_cloning_is_a_conflict() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let id = cloning_project(&app, &user, "mars").await;

    let response = app.post_as(&user, &fetch_path(id)).await;

    response.assert_status(StatusCode::CONFLICT);
    response.assert_json(&json!({ "status": 409, "error": "project is not ready" }));
}

#[tokio::test]
async fn listing_the_branches_of_a_project_that_is_still_cloning_is_a_conflict() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let id = cloning_project(&app, &user, "mars").await;

    let response = app.get_as(&user, &branches_path(id)).await;

    response.assert_status(StatusCode::CONFLICT);
    response.assert_json(&json!({ "status": 409, "error": "project is not ready" }));
}

#[tokio::test]
async fn listing_the_branches_of_a_failed_project_is_a_conflict() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let id = failed_project(&app, &user, "mars").await;

    let response = app.get_as(&user, &branches_path(id)).await;

    response.assert_status(StatusCode::CONFLICT);
    response.assert_json(&json!({ "status": 409, "error": "project is not ready" }));
}

#[tokio::test]
async fn a_fetch_whose_remote_is_gone_answers_the_git_status_and_changes_nothing() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let upstream = upstream_with(&[]).await;
    let (id, cloned) = cloned_project(&app, &user, "mars", &upstream).await;

    // The remote directory disappears under the mirror, which git reports as a
    // failed command: an internal error rather than a state the caller named
    // (`GitError::status`).
    std::fs::remove_dir_all(&upstream.path).expect("the upstream is removed");

    let response = app.post_as(&user, &fetch_path(id)).await;

    response.assert_status(StatusCode::INTERNAL_SERVER_ERROR);

    // Nothing was recorded: the project is still `ready`, still has no status
    // message, and was last fetched by the clone.
    let read = app.get_as(&user, &project_path(id)).await;
    read.assert_status_ok();
    let project = read.json::<Value>();
    assert_eq!(project["status"], json!("ready"));
    assert_eq!(project["status_message"], Value::Null);
    assert_eq!(
        fetched_at(&project),
        cloned.last_fetched_at.expect("the clone recorded a fetch")
    );
}

#[tokio::test]
async fn fetching_and_listing_an_unknown_project_is_404() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let id = Uuid::new_v4();

    let responses = [
        app.post_as(&user, &fetch_path(id)).await,
        app.get_as(&user, &branches_path(id)).await,
    ];

    for response in responses {
        response.assert_status(StatusCode::NOT_FOUND);
        response.assert_json(&json!({ "status": 404, "error": "not found" }));
    }
}

// --- DELETE /projects/{id}

// The deletion tests' own imports and helpers, kept together down here rather
// than in the shared blocks at the top of the file.
use mars_orchestrator::events::TaskActor;
use mars_orchestrator::models::{
    NewEvent, NewSession, NewSharedDir, NewTask, NewTaskComment, NewTaskHandoff, ProfileKind,
    SecretUsePurpose, SessionState, StateChange, TaskDependencyKind,
};
use mars_orchestrator::projects::session_dir;
use mars_orchestrator::repositories::{
    ProjectRepository, SecretRepository, SessionRepository, TaskRepository,
};
use mars_orchestrator::tracker::TrackerMutation;
use sqlx::PgPool;

/// An obviously fake but well-formed SHA-1 object id (rule 3).
const FAKE_COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

/// An obviously fake second project secret, and the name it is stored under
/// (rule 3).
const EXTRA_SECRET_NAME: &str = "FAKE_DEPLOY_TOKEN";
const EXTRA_SECRET_VALUE: &str = "fake-deploy-token-for-tests";

/// The ids a deleted project's rows are found by afterwards.
///
/// Seven tables are reached from the project id alone; the rest hang off a
/// session, a profile, a task or a secret, all of which are gone once the
/// project is. Counting those through a join would report zero whether or not
/// the child rows survived, so the ids are captured while the project is still
/// there and the counts are taken against them.
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

/// Every table named above holds something, which is what makes the assertion
/// after the deletion mean anything.
fn assert_every_table_seeded(counts: &[(&'static str, i64)]) {
    for (table, count) in counts {
        assert!(*count > 0, "{table} was not seeded");
    }
}

/// Not one row is left anywhere.
fn assert_every_table_empty(counts: &[(&'static str, i64)]) {
    for (table, count) in counts {
        assert_eq!(*count, 0, "{table} still holds rows of the deleted project");
    }
}

/// A `ready` project created with a credential, so it starts with the
/// project-scoped `GIT_CREDENTIAL` secret.
///
/// The mock credential provider is what the clone actually uses, so storing one
/// changes nothing about the clone; it is here because deleting the secret is
/// part of what these tests assert.
async fn credentialled_project(
    app: &TestApp,
    user: &AuthenticatedUser,
    upstream: &TestUpstream,
) -> Uuid {
    let created = created(
        app,
        user,
        &json!({
            "name": "mars",
            "remote_url": file_url(&upstream.path),
            "credential": FAKE_CREDENTIAL,
        }),
    )
    .await;
    assert_eq!(created["has_credential"], json!(true));

    let id = id_of(&created);
    let project = clone_job::wait_for_clone(&app.state, id, CLONE_TIMEOUT).await;
    assert_eq!(
        project.status,
        ProjectStatus::Ready,
        "{:?}",
        project.status_message
    );

    id
}

/// A session of this project in `state`, with an event and its directory.
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

    tokio::fs::create_dir_all(session_dir(&app.state.config.data_dir, session.id))
        .await
        .expect("the session directory is created");

    session.id
}

/// Everything a project can own, written through the repositories that own it.
///
/// A shared-directory row and its directory, two tasks with a dependency, a
/// comment and a hand-off between them, a project-wide task event, a `parked`
/// session with an event and a directory, and a second project secret with an
/// audit row of its own.
async fn seed_everything(app: &TestApp, user: &AuthenticatedUser, project_id: Uuid) -> Owned {
    let pool = &app.state.pool;
    let projects = ProjectRepository::new(pool);
    let tasks = TaskRepository::new(pool);

    let layout = app.state.config.project_layout(project_id);
    let mut tx = pool.begin().await.expect("a transaction begins");
    projects
        .insert_shared_dir(
            &mut tx,
            project_id,
            &NewSharedDir::new("target", "/session/work/target").expect("the shared dir parses"),
        )
        .await
        .expect("the shared directory inserts");
    tx.commit().await.expect("the transaction commits");
    layout
        .ensure_shared_dir("target")
        .await
        .expect("the shared directory is created");

    let session_id = session_in(app, project_id, SessionState::Parked).await;

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

    let comment = NewTaskComment::from_user(blocker.id, user.user.id, "implemented and pushed");
    tasks
        .insert_comment(mutation.conn(), project_id, &comment)
        .await
        .expect("the comment inserts");
    let mut handoff = NewTaskHandoff::new(
        blocker.id,
        format!("session/{session_id}"),
        FAKE_COMMIT,
        comment.id,
    );
    handoff.source_session_id = Some(session_id);
    handoff.created_by_session_id = Some(session_id);
    tasks
        .insert_handoff(mutation.conn(), project_id, &handoff)
        .await
        .expect("the hand-off inserts");

    mutation
        .emit_states_changed(&[])
        .expect("the board event is emitted");
    mutation.commit().await.expect("the mutation commits");

    let response = app
        .post_as(user, "/api/secrets")
        .json(&json!({
            "scope": "project",
            "scope_id": project_id,
            "name": EXTRA_SECRET_NAME,
            "value": EXTRA_SECRET_VALUE,
        }))
        .await;
    response.assert_status(StatusCode::CREATED);

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

    Owned {
        project: project_id,
        sessions: vec![session_id],
        // Every profile the project was seeded with, not just the default
        // one, so the deletion assertion covers all four (`SPEC.md`, "Role
        // profile templates").
        profiles: projects
            .list_profiles(project_id)
            .await
            .expect("the profiles read")
            .into_iter()
            .map(|profile| profile.id)
            .collect(),
        tasks: vec![blocker.id, blocked.id],
        secrets,
    }
}

#[tokio::test]
async fn deleting_a_project_removes_every_row_and_every_directory_it_owned() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let upstream = upstream_with(&[]).await;
    let id = credentialled_project(&app, &user, &upstream).await;

    let owned = seed_everything(&app, &user, id).await;
    assert_every_table_seeded(&rows_left(&app.state.pool, &owned).await);

    let paths = DataPaths::from_config(&app.state.config);
    let project_dir = paths.project_dir(id);
    let session_directory = paths.session_dir(owned.sessions[0]);
    assert!(project_dir.is_dir());
    assert!(paths.project_repo(id).is_dir());
    assert!(session_directory.is_dir());

    let response = app.delete_as(&user, &project_path(id)).await;
    response.assert_status(StatusCode::NO_CONTENT);

    assert_every_table_empty(&rows_left(&app.state.pool, &owned).await);
    assert!(
        !project_dir.exists(),
        "the mirror, the CLI state directory and the shared directories go with the project"
    );
    assert!(
        !session_directory.exists(),
        "each former session's directory goes too"
    );

    // A second deletion has nothing left to delete (`SPEC.md`, "Projects").
    app.delete_as(&user, &project_path(id))
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_live_session_refuses_the_deletion_until_it_has_ended() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let upstream = upstream_with(&[]).await;
    let id = credentialled_project(&app, &user, &upstream).await;

    let owned = seed_everything(&app, &user, id).await;
    let paths = DataPaths::from_config(&app.state.config);
    let project_dir = paths.project_dir(id);

    // A session starts `creating` and becomes `running`; both are live, and a
    // deletion attempted in either state changes nothing at all.
    let live = session_in(&app, id, SessionState::Creating).await;
    for state in [SessionState::Creating, SessionState::Running] {
        if state == SessionState::Running {
            let mut tx = app.state.pool.begin().await.expect("a transaction begins");
            SessionRepository::new(&app.state.pool)
                .set_state(&mut tx, live, state, &StateChange::plain())
                .await
                .expect("the session starts running");
            tx.commit().await.expect("the transaction commits");
        }

        let response = app.delete_as(&user, &project_path(id)).await;
        response.assert_status(StatusCode::CONFLICT);
        response.assert_json(&json!({
            "status": 409,
            "error": "project has running sessions"
        }));

        assert_every_table_seeded(&rows_left(&app.state.pool, &owned).await);
        assert!(project_dir.is_dir(), "a refused deletion removes nothing");
    }

    // `parked` never counted, and `done` does not either: the run has ended
    // and its container is gone, so there is nothing left to refuse for.
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
    assert!(
        !paths.session_dir(live).exists(),
        "every session's directory goes, not only the ones seeded first"
    );
}

#[tokio::test]
async fn deleting_a_project_whose_directory_was_never_created_is_still_204() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;

    // Straight through the creation transaction rather than the route, which
    // spawns the clone job at once: this is a project whose job has not
    // started, the only way to catch one that is `cloning` with nothing on
    // disk.
    let project = create_project(
        &app.state,
        NewProjectRequest {
            name: "mars".to_string(),
            remote_url: TEST_REMOTE.to_string(),
            default_branch: None,
            credential: None,
        },
        user.user.id,
    )
    .await
    .expect("the project is created");
    assert_eq!(project.status, ProjectStatus::Cloning);

    let project_dir = DataPaths::from_config(&app.state.config).project_dir(project.id);
    assert!(!project_dir.exists(), "the clone job never ran");

    app.delete_as(&user, &project_path(project.id))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    assert!(
        ProjectRepository::new(&app.state.pool)
            .find(project.id)
            .await
            .expect("the project reads")
            .is_none()
    );
}

#[tokio::test]
async fn deleting_an_unknown_project_is_404() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;

    let response = app.delete_as(&user, &project_path(Uuid::new_v4())).await;

    response.assert_status(StatusCode::NOT_FOUND);
    response.assert_json(&json!({ "status": 404, "error": "not found" }));
}

#[tokio::test]
async fn deleting_a_project_needs_a_token_and_an_ungated_user() {
    let app = TestApp::spawn().await;
    let gated = app.create_gated_user("gated", "gated@example.test").await;
    let id = Uuid::new_v4();

    let anonymous = app.server.delete(&project_path(id)).await;
    anonymous.assert_status(StatusCode::UNAUTHORIZED);
    anonymous.assert_json(&unauthorized());

    let refused = app.delete_as(&gated, &project_path(id)).await;
    refused.assert_status(StatusCode::FORBIDDEN);
    refused.assert_json(&password_change_required());
}

// ---- the unattended-launch settings (`ARCHITECTURE.md`, "Unattended
// launches"; ADR 0042) ----

#[tokio::test]
async fn the_project_cap_and_the_pause_switch_are_set_and_cleared_by_put() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let id = id_of(&created(&app, &user, &new_project("mars", UNREACHABLE_REMOTE)).await);

    let set = app
        .put_as(&user, &project_path(id))
        .json(&json!({ "max_concurrent_sessions": 3, "automation_paused": true }))
        .await;
    set.assert_status_ok();
    let updated = set.json::<Value>();
    assert_eq!(updated["max_concurrent_sessions"], json!(3));
    assert_eq!(updated["automation_paused"], json!(true));

    // Committed, not just answered.
    let read = app.get_as(&user, &project_path(id)).await;
    read.assert_status_ok();
    assert_eq!(read.json::<Value>()["max_concurrent_sessions"], json!(3));

    // An update that names neither leaves both alone, like every other field.
    let elsewhere = app
        .put_as(&user, &project_path(id))
        .json(&json!({ "name": "mars-2" }))
        .await;
    elsewhere.assert_status_ok();
    let kept = elsewhere.json::<Value>();
    assert_eq!(kept["max_concurrent_sessions"], json!(3));
    assert_eq!(kept["automation_paused"], json!(true));

    // An explicit null removes the cap, which no other nullable field of this
    // body can express (`SPEC.md`, "Projects").
    let cleared = app
        .put_as(&user, &project_path(id))
        .json(&json!({ "max_concurrent_sessions": null, "automation_paused": false }))
        .await;
    cleared.assert_status_ok();
    let uncapped = cleared.json::<Value>();
    assert_eq!(uncapped["max_concurrent_sessions"], Value::Null);
    assert_eq!(uncapped["automation_paused"], json!(false));
}

#[tokio::test]
async fn a_project_cap_below_one_is_a_bad_request() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let id = id_of(&created(&app, &user, &new_project("mars", UNREACHABLE_REMOTE)).await);

    for cap in [0, -1] {
        let response = app
            .put_as(&user, &project_path(id))
            .json(&json!({ "max_concurrent_sessions": cap }))
            .await;

        response.assert_status(StatusCode::BAD_REQUEST);
        response.assert_json(&json!({
            "status": 400,
            "error": "max_concurrent_sessions must be at least 1 when set",
        }));
    }

    // And nothing was written by either attempt.
    let read = app.get_as(&user, &project_path(id)).await;
    read.assert_status_ok();
    assert_eq!(read.json::<Value>()["max_concurrent_sessions"], Value::Null);
}

#[tokio::test]
async fn neither_setting_can_be_sent_to_the_create_endpoint() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;

    // `deny_unknown_fields`: they are `PUT`'s, like `max_attempts`.
    for extra in ["max_concurrent_sessions", "automation_paused"] {
        let mut body = new_project("mars", UNREACHABLE_REMOTE);
        body[extra] = json!(1);

        let response = create(&app, &user, &body).await;
        response.assert_status(StatusCode::BAD_REQUEST);
    }
}
