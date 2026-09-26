//! `/api/projects/{pid}/shared-dirs` through the real router (`SPEC.md`,
//! "Shared directories (`/api/projects/{pid}/shared-dirs`)").
//!
//! The four endpoints a project's shared directories are managed through. The
//! name and path rules are the model's and are asserted there
//! (`src/models/shared_dir.rs`); the constraints are the table's and are
//! asserted in `tests/repositories_shared_dirs.rs`; the filesystem helpers are
//! the layout's and are asserted in `src/projects/layout.rs`. What is asserted
//! here is the adapter and the one rule that exists nowhere else: clearing and
//! deleting are refused while a session of the project is `running` or
//! `creating`, and both leave the row and the directory exactly as they were
//! when they are (`ARCHITECTURE.md`, "Storage"; `README.md`, "Operating
//! notes").
//!
//! Also asserted: a create writes **no** directory — creation is the
//! launcher's, lazily, at the next launch — and every endpoint's 401 and its
//! 403 under the password-change gate.
//!
//! The project is created through `POST /api/projects` with a remote that can
//! never resolve, so the background clone job fails quickly; nothing here
//! depends on the project's status, and the directories the tests need are
//! created underneath it rather than waited for.
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use axum::http::StatusCode;
use axum_test::TestResponse;
use common::projects::{password_change_required, project, session_in, signed_in, unauthorized};
use common::{AuthenticatedUser, TestApp};
use mars_orchestrator::models::{SessionState, SharedDirError};
use serde_json::{Value, json};
use uuid::Uuid;

// ---- helpers ----
//
// Signing in, the documented refusals, the project and the sessions the
// refusal counts are `tests/common/projects.rs`'s, shared with the other
// project route suites.

/// `/api/projects/{pid}/shared-dirs`.
fn dirs_path(pid: Uuid) -> String {
    format!("/api/projects/{pid}/shared-dirs")
}

/// `/api/projects/{pid}/shared-dirs/{name}`.
fn dir_path(pid: Uuid, name: &str) -> String {
    format!("/api/projects/{pid}/shared-dirs/{name}")
}

/// `/api/projects/{pid}/shared-dirs/{name}/clear`.
fn clear_path(pid: Uuid, name: &str) -> String {
    format!("/api/projects/{pid}/shared-dirs/{name}/clear")
}

/// The create body: a name and a mount point.
fn new_dir(name: &str, container_path: &str) -> Value {
    json!({ "name": name, "container_path": container_path })
}

/// `POST /api/projects/{pid}/shared-dirs`, raw.
async fn create(app: &TestApp, user: &AuthenticatedUser, pid: Uuid, body: &Value) -> TestResponse {
    app.post_as(user, &dirs_path(pid)).json(body).await
}

/// `POST /api/projects/{pid}/shared-dirs` for an entry that must be created.
async fn created(app: &TestApp, user: &AuthenticatedUser, pid: Uuid, name: &str, path: &str) {
    create(app, user, pid, &new_dir(name, path))
        .await
        .assert_status(StatusCode::CREATED);
}

/// `GET /api/projects/{pid}/shared-dirs`, asserting 200.
async fn listed(app: &TestApp, user: &AuthenticatedUser, pid: Uuid) -> Vec<Value> {
    let response = app.get_as(user, &dirs_path(pid)).await;
    response.assert_status_ok();

    response.json::<Vec<Value>>()
}

/// `DATA_DIR/projects/<pid>/shared/<name>`, as the orchestrator sees it.
fn shared_dir(app: &TestApp, pid: Uuid, name: &str) -> PathBuf {
    app.state
        .config
        .project_layout(pid)
        .shared_dir(name)
        .expect("the test name is a shared directory name")
}

/// Create `shared/<name>` with a file and a nested file in it, as a launch and
/// a build would have left it.
async fn populate(app: &TestApp, pid: Uuid, name: &str) -> PathBuf {
    let directory = shared_dir(app, pid, name);

    tokio::fs::create_dir_all(directory.join("debug/deps"))
        .await
        .expect("the shared directory is created");
    tokio::fs::write(directory.join("artifact"), b"x")
        .await
        .expect("a file is written");
    tokio::fs::write(directory.join("debug/deps/lib.rlib"), b"x")
        .await
        .expect("a nested file is written");

    directory
}

/// Whether `directory` exists and has no entries.
async fn is_empty(directory: &Path) -> bool {
    let mut entries = tokio::fs::read_dir(directory)
        .await
        .expect("the directory exists");

    entries
        .next_entry()
        .await
        .expect("the directory reads")
        .is_none()
}

/// The 409 both destructive endpoints answer while a session is live.
fn running_sessions() -> Value {
    json!({ "status": 409, "error": "project has running sessions" })
}

// ---- authentication ----

#[tokio::test]
async fn every_endpoint_requires_a_token() {
    let app = TestApp::spawn().await;
    let pid = Uuid::new_v4();

    let responses = [
        app.server.get(&dirs_path(pid)).await,
        app.server
            .post(&dirs_path(pid))
            .json(&new_dir("target", "/session/work/target"))
            .await,
        app.server.post(&clear_path(pid, "target")).await,
        app.server.delete(&dir_path(pid, "target")).await,
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
    let pid = Uuid::new_v4();

    let responses = [
        app.get_as(&gated, &dirs_path(pid)).await,
        app.post_as(&gated, &dirs_path(pid))
            .json(&new_dir("target", "/session/work/target"))
            .await,
        app.post_as(&gated, &clear_path(pid, "target")).await,
        app.delete_as(&gated, &dir_path(pid, "target")).await,
    ];

    for response in responses {
        response.assert_status(StatusCode::FORBIDDEN);
        response.assert_json(&password_change_required());
    }
}

#[tokio::test]
async fn every_endpoint_answers_404_for_an_unknown_project() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = Uuid::new_v4();

    let responses = [
        app.get_as(&user, &dirs_path(pid)).await,
        app.post_as(&user, &dirs_path(pid))
            .json(&new_dir("target", "/session/work/target"))
            .await,
        app.post_as(&user, &clear_path(pid, "target")).await,
        app.delete_as(&user, &dir_path(pid, "target")).await,
    ];

    for response in responses {
        response.assert_status(StatusCode::NOT_FOUND);
    }
}

// ---- create and list ----

#[tokio::test]
async fn creating_a_shared_directory_answers_the_documented_shape() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;

    let response = create(&app, &user, pid, &new_dir("target", "/session/work/target")).await;

    response.assert_status(StatusCode::CREATED);
    let dir = response.json::<Value>();

    // Exactly the three documented fields: `project_id` is already in the URL
    // and never comes back. The set, because a parsed object carries no order.
    let object = dir.as_object().expect("a shared directory is an object");
    assert_eq!(
        object.keys().map(String::as_str).collect::<BTreeSet<_>>(),
        BTreeSet::from(["name", "container_path", "created_at"])
    );
    assert_eq!(dir["name"], json!("target"));
    assert_eq!(dir["container_path"], json!("/session/work/target"));
    assert!(
        dir["created_at"].as_str().is_some(),
        "created_at is a timestamp"
    );
}

#[tokio::test]
async fn creating_a_shared_directory_writes_no_directory() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;

    created(&app, &user, pid, "target", "/session/work/target").await;

    // The row is configuration; the directory appears at the next launch
    // (`docs/data-model.md`, `project_shared_dirs`).
    assert!(!shared_dir(&app, pid, "target").exists());
}

#[tokio::test]
async fn the_listing_is_ordered_by_name() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;

    assert!(listed(&app, &user, pid).await.is_empty());

    created(&app, &user, pid, "target", "/session/work/target").await;
    created(&app, &user, pid, "npm-cache", "/session/home/.npm").await;
    created(&app, &user, pid, "go-mod", "/session/home/go/pkg/mod").await;

    let dirs = listed(&app, &user, pid).await;
    let names: Vec<&str> = dirs
        .iter()
        .map(|dir| dir["name"].as_str().expect("a name"))
        .collect();
    assert_eq!(names, ["go-mod", "npm-cache", "target"]);
}

#[tokio::test]
async fn the_listing_is_scoped_to_its_project() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let mars = project(&app, &user, "mars").await;
    let phobos = project(&app, &user, "phobos").await;

    created(&app, &user, mars, "target", "/session/work/target").await;

    assert_eq!(listed(&app, &user, mars).await.len(), 1);
    assert!(listed(&app, &user, phobos).await.is_empty());
}

#[tokio::test]
async fn a_duplicate_name_and_a_duplicate_path_are_told_apart() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;

    created(&app, &user, pid, "target", "/session/work/target").await;

    let same_name = create(&app, &user, pid, &new_dir("target", "/session/work/other")).await;
    same_name.assert_status(StatusCode::CONFLICT);
    same_name.assert_json(&json!({
        "status": 409,
        "error": "shared directory name already used",
    }));

    let same_path = create(&app, &user, pid, &new_dir("other", "/session/work/target")).await;
    same_path.assert_status(StatusCode::CONFLICT);
    same_path.assert_json(&json!({
        "status": 409,
        "error": "container path already used",
    }));

    // Neither attempt wrote a row.
    assert_eq!(listed(&app, &user, pid).await.len(), 1);
}

#[tokio::test]
async fn the_same_name_and_path_may_be_used_in_another_project() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let mars = project(&app, &user, "mars").await;
    let phobos = project(&app, &user, "phobos").await;

    created(&app, &user, mars, "target", "/session/work/target").await;
    created(&app, &user, phobos, "target", "/session/work/target").await;
}

#[tokio::test]
async fn nested_container_paths_are_allowed() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;

    // The launcher mounts parents first (`ARCHITECTURE.md`, "Storage"), so the
    // rules do not have to refuse this.
    created(&app, &user, pid, "target", "/session/work/target").await;
    created(&app, &user, pid, "debug", "/session/work/target/debug").await;
}

#[tokio::test]
async fn an_invalid_name_is_the_models_bad_request() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;

    let too_long = "t".repeat(65);
    for name in ["Target", "-x", too_long.as_str(), "", "a/b", ".."] {
        let response = create(&app, &user, pid, &new_dir(name, "/session/work/target")).await;

        response.assert_status(StatusCode::BAD_REQUEST);
        response.assert_json(&json!({
            "status": 400,
            "error": SharedDirError::InvalidName.to_string(),
        }));
    }

    assert!(listed(&app, &user, pid).await.is_empty());
}

#[tokio::test]
async fn an_invalid_container_path_is_the_models_bad_request() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;

    // One per rule, and the message is the model's own (`src/models/
    // shared_dir.rs`), never one invented here.
    let cases = [
        ("relative", "must be absolute"),
        (
            "/session/work",
            "must not be, or contain, /session/work, /session/home, /session/log or /session/mcp.json",
        ),
        (
            "/session",
            "must not be, or contain, /session/work, /session/home, /session/log or /session/mcp.json",
        ),
        ("/data/x", "must not be /data or below it"),
        ("/a//b", "must not contain repeated slashes"),
        ("/a/../b", "must not contain . or .. segments"),
        ("/a/", "must not end in a slash"),
        ("", "must not be empty"),
    ];

    for (path, rule) in cases {
        let response = create(&app, &user, pid, &new_dir("target", path)).await;

        response.assert_status(StatusCode::BAD_REQUEST);
        response.assert_json(&json!({
            "status": 400,
            "error": SharedDirError::InvalidPath(rule).to_string(),
        }));
    }

    assert!(listed(&app, &user, pid).await.is_empty());
}

#[tokio::test]
async fn every_recommended_entry_is_accepted() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;

    // The table in `README.md`, "Operating notes": advice the product gives
    // that the endpoint refused would be a bug in one of the two.
    for (name, path) in [
        ("target", "/session/work/target"),
        ("cargo-registry", "/session/home/.cargo/registry"),
        ("npm-cache", "/session/home/.npm"),
        ("go-mod", "/session/home/go/pkg/mod"),
        ("go-build", "/session/home/.cache/go-build"),
        ("uv-cache", "/session/home/.cache/uv"),
        ("m2", "/session/home/.m2"),
        ("gradle", "/session/home/.gradle"),
    ] {
        created(&app, &user, pid, name, path).await;
    }

    assert_eq!(listed(&app, &user, pid).await.len(), 8);
}

// ---- clear ----

#[tokio::test]
async fn clearing_empties_the_directory_and_keeps_it() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;
    created(&app, &user, pid, "target", "/session/work/target").await;
    let directory = populate(&app, pid, "target").await;

    app.post_as(&user, &clear_path(pid, "target"))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    assert!(directory.is_dir(), "the directory itself must survive");
    assert!(is_empty(&directory).await, "the directory is not empty");
    // The row stays: only the contents went.
    assert_eq!(listed(&app, &user, pid).await.len(), 1);
}

#[tokio::test]
async fn clearing_a_directory_that_was_never_created_is_still_204() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;
    created(&app, &user, pid, "target", "/session/work/target").await;

    app.post_as(&user, &clear_path(pid, "target"))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    assert!(!shared_dir(&app, pid, "target").exists());
}

#[tokio::test]
async fn clearing_an_unknown_name_is_404() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;

    app.post_as(&user, &clear_path(pid, "absent"))
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn clearing_is_refused_while_a_session_is_live() {
    for state in [SessionState::Running, SessionState::Creating] {
        let app = TestApp::spawn().await;
        let user = signed_in(&app, "ada").await;
        let pid = project(&app, &user, "mars").await;
        created(&app, &user, pid, "target", "/session/work/target").await;
        let directory = populate(&app, pid, "target").await;
        session_in(&app, pid, state).await;

        let response = app.post_as(&user, &clear_path(pid, "target")).await;

        response.assert_status(StatusCode::CONFLICT);
        response.assert_json(&running_sessions());
        // Nothing was touched: a build may be holding these files open.
        assert!(directory.join("artifact").is_file(), "{state}");
        assert!(directory.join("debug/deps/lib.rlib").is_file(), "{state}");
    }
}

#[tokio::test]
async fn clearing_is_allowed_while_a_session_is_parked() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;
    created(&app, &user, pid, "target", "/session/work/target").await;
    let directory = populate(&app, pid, "target").await;
    session_in(&app, pid, SessionState::Parked).await;

    // A parked session has no container holding anything open (`SPEC.md`,
    // "Shared directories": the refusal is `running` and `creating` only).
    app.post_as(&user, &clear_path(pid, "target"))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    assert!(is_empty(&directory).await, "the directory is not empty");
}

#[tokio::test]
async fn a_name_that_is_not_a_name_never_reaches_the_filesystem() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;

    // A literal `..` is not among them and cannot be: a conforming client
    // resolves dot segments — encoded ones included — out of the URL before
    // the request is sent, so `shared-dirs/../clear` never arrives as a name
    // at all. That the *model* refuses one is asserted in
    // `src/routes/shared_dirs.rs` and again in `src/projects/layout.rs`; what
    // an HTTP request can still smuggle in is an encoded separator, and this
    // is that attempt. The rejection carries the model's message and no name.
    for name in ["a%2Fb", "Target", "-x", "%2Etarget"] {
        for response in [
            app.post_as(&user, &clear_path(pid, name)).await,
            app.delete_as(&user, &dir_path(pid, name)).await,
        ] {
            response.assert_status(StatusCode::BAD_REQUEST);
            response.assert_json(&json!({
                "status": 400,
                "error": SharedDirError::InvalidName.to_string(),
            }));
        }
    }
}

// ---- delete ----

#[tokio::test]
async fn deleting_removes_the_row_and_the_directory() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;
    created(&app, &user, pid, "target", "/session/work/target").await;
    let directory = populate(&app, pid, "target").await;

    app.delete_as(&user, &dir_path(pid, "target"))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    assert!(!directory.exists());
    assert!(listed(&app, &user, pid).await.is_empty());
    // The parent stays: other shared directories live there.
    assert!(app.state.config.project_layout(pid).shared_root().is_dir());
}

#[tokio::test]
async fn deleting_a_directory_that_was_never_created_is_still_204() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;
    created(&app, &user, pid, "target", "/session/work/target").await;

    app.delete_as(&user, &dir_path(pid, "target"))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    assert!(listed(&app, &user, pid).await.is_empty());
}

#[tokio::test]
async fn deleting_an_unknown_name_is_404() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "mars").await;
    created(&app, &user, pid, "target", "/session/work/target").await;

    app.delete_as(&user, &dir_path(pid, "absent"))
        .await
        .assert_status(StatusCode::NOT_FOUND);

    // A second delete of the same name is the same 404.
    app.delete_as(&user, &dir_path(pid, "target"))
        .await
        .assert_status(StatusCode::NO_CONTENT);
    app.delete_as(&user, &dir_path(pid, "target"))
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn deleting_is_refused_while_a_session_is_live() {
    for state in [SessionState::Running, SessionState::Creating] {
        let app = TestApp::spawn().await;
        let user = signed_in(&app, "ada").await;
        let pid = project(&app, &user, "mars").await;
        created(&app, &user, pid, "target", "/session/work/target").await;
        let directory = populate(&app, pid, "target").await;
        session_in(&app, pid, state).await;

        let response = app.delete_as(&user, &dir_path(pid, "target")).await;

        response.assert_status(StatusCode::CONFLICT);
        response.assert_json(&running_sessions());
        // The row and the directory are both exactly as they were.
        assert_eq!(listed(&app, &user, pid).await.len(), 1, "{state}");
        assert!(directory.join("artifact").is_file(), "{state}");
    }
}
