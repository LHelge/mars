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
    assert_eq!(project["has_credential"], json!(false));
    assert!(project["created_at"].is_string());

    // Exactly the ten documented fields, and none of the three the row adds.
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
            "created_at",
            "default_branch",
            "has_credential",
            "id",
            "last_fetched_at",
            "max_attempts",
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
