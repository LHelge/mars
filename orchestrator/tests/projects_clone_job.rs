//! The background clone job, against real repositories and a real database
//! (`ARCHITECTURE.md`, "Git model", Project clone; `SPEC.md`, "Projects").
//!
//! Git is never mocked (`CLAUDE.md`, "Testing expectations"), so every project
//! here points at a [`BareFixture`], a bare repository in a `tempfile`
//! directory that can also be empty, removed and recreated, over the
//! `file://` form [`RemoteUrl`] accepts under the `integration-tests` feature.
//! There is no `POST /projects` route yet, so the rows are inserted through
//! [`ProjectRepository`] exactly as that route will and the job is called
//! directly.
//!
//! Nothing here sleeps for a fixed time: every wait goes through
//! [`clone_job::wait_for_clone`] or [`wait_for_path`], both of which give up
//! with a message rather than hanging.
//!
//! The one credential is an obviously fake stand-in for a PAT (rule 3); the
//! local upstream ignores the header it produces, and what the test that uses
//! it asserts is the `secret_uses` row the lookup leaves behind.

#![cfg(feature = "integration-tests")]

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use common::TestApp;
use common::git::BareFixture;
use common::projects::{CLONE_TIMEOUT, UNREACHABLE_REMOTE};
use mars_orchestrator::git::{
    CommitIdentity, DataPaths, GitActor, GitCommand, PatCredentialProvider,
};
use mars_orchestrator::models::{BranchName, NewProject, Project, ProjectStatus};
use mars_orchestrator::prelude::*;
use mars_orchestrator::projects::clone_job;
use mars_orchestrator::repositories::ProjectRepository;
use mars_orchestrator::secrets::set_project_git_credential;
use uuid::Uuid;
use zeroize::Zeroizing;

/// Not a real PAT: an obviously fake stand-in (rule 3).
const FAKE_PAT: &str = "ghp_FAKE_TEST_TOKEN_0000000000";

/// A file that is put inside `repo.git` to prove the next run replaced the
/// directory instead of reusing it.
const STALE_MARKER: &str = "mars-stale-marker";

/// A project row in `cloning`, as `POST /projects` leaves one.
async fn insert_project(
    app: &TestApp,
    remote_url: &str,
    default_branch: Option<&str>,
    created_by: Option<Uuid>,
) -> Project {
    let mut project = NewProject::new("fixture", remote_url).expect("the fixture project is valid");
    project.default_branch = default_branch
        .map(|name| BranchName::parse(name).expect("the fixture branch name is valid"));
    project.created_by = created_by;

    let mut tx = app.pool.begin().await.expect("a transaction begins");
    let inserted = ProjectRepository::new(&app.pool)
        .insert(&mut tx, &project)
        .await
        .expect("the project inserts");
    tx.commit().await.expect("the transaction commits");

    assert_eq!(inserted.status, ProjectStatus::Cloning);
    inserted
}

/// `DATA_DIR/projects/<id>/repo.git`.
fn repo_git(app: &TestApp, project_id: Uuid) -> PathBuf {
    DataPaths::new(app.data_dir.path()).project_repo(project_id)
}

/// Does `full_name` exist in the project repository?
async fn ref_exists(repo: &Path, full_name: &str) -> bool {
    let output = GitCommand::new()
        .args(["show-ref", "--verify", "--quiet", "--end-of-options"])
        .arg(full_name)
        .cwd(repo)
        .run()
        .await
        .expect("git show-ref runs");

    output.status == 0
}

/// One configured value, or an empty string when the key is unset.
async fn config_value(repo: &Path, key: &str) -> String {
    let output = GitCommand::new()
        .args(["config", "--get", "--end-of-options", key])
        .cwd(repo)
        .run()
        .await
        .expect("git config runs");

    output.stdout.trim().to_string()
}

/// Wait until `path` exists, or give up with a message.
///
/// Used to observe that the job has passed a step, so the test can act in the
/// window after it; never to wait for the job as a whole, which is what
/// [`clone_job::wait_for_clone`] is for.
async fn wait_for_path(path: &Path, timeout: Duration) {
    let deadline = tokio::time::Instant::now() + timeout;

    while !path.exists() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "{} did not appear within {timeout:?}",
            path.display()
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn a_project_with_no_default_branch_discovers_it_and_becomes_ready() {
    let app = TestApp::spawn().await;
    let upstream = BareFixture::new();

    let user = app
        .insert_user("cloner", "cloner@example.invalid", false, false)
        .await;
    let remote = upstream.url();
    let project = insert_project(&app, &remote, None, Some(user.id)).await;

    clone_job::spawn(app.state.clone(), project.id, Some(user.id))
        .await
        .expect("the clone job does not panic");

    let cloned = clone_job::wait_for_clone(&app.state, project.id, CLONE_TIMEOUT).await;
    assert_eq!(
        cloned.status,
        ProjectStatus::Ready,
        "{:?}",
        cloned.status_message
    );
    assert_eq!(cloned.default_branch.as_deref(), Some("main"));
    assert_eq!(cloned.status_message, None);
    assert!(
        cloned.last_fetched_at.is_some(),
        "the clone that just ran is a fetch"
    );

    // The repository, its refs and the settings `ARCHITECTURE.md` fixes.
    let repo = repo_git(&app, project.id);
    assert!(
        ref_exists(&repo, "refs/heads/main").await,
        "no integration head"
    );
    assert!(
        ref_exists(&repo, "refs/remotes/origin/main").await,
        "no upstream-tracking ref"
    );
    assert_eq!(config_value(&repo, "gc.auto").await, "0");

    // The rest of the project's directory, which the job creates itself.
    let layout = app.state.config.project_layout(project.id);
    assert!(layout.claude_dir().is_dir(), "no CLI state directory");
    assert!(layout.shared_root().is_dir(), "no shared directory root");

    // The credential was asked for on behalf of the user who requested it,
    // which is what `secret_uses` records (`docs/data-model.md`).
    assert_eq!(
        app.mock_git().requested(),
        vec![(project.id, GitActor::User(user.id))]
    );
}

#[tokio::test]
async fn a_default_branch_the_remote_has_is_used_as_it_was_given() {
    let app = TestApp::spawn().await;
    let upstream = BareFixture::new();
    // A second branch, not the one upstream's HEAD names.
    upstream.add_branch("release/2.0");

    let remote = upstream.url();
    let project = insert_project(&app, &remote, Some("release/2.0"), None).await;

    clone_job::run(&app.state, project.id, None).await;

    let cloned = clone_job::wait_for_clone(&app.state, project.id, CLONE_TIMEOUT).await;
    assert_eq!(
        cloned.status,
        ProjectStatus::Ready,
        "{:?}",
        cloned.status_message
    );
    assert_eq!(cloned.default_branch.as_deref(), Some("release/2.0"));

    // Nobody asked: a job with no requesting user is the orchestrator's own.
    assert_eq!(
        app.mock_git().requested(),
        vec![(project.id, GitActor::System)]
    );
}

#[tokio::test]
async fn a_default_branch_the_remote_lacks_fails_with_the_documented_message() {
    let app = TestApp::spawn().await;
    let upstream = BareFixture::new();

    let remote = upstream.url();
    let project = insert_project(&app, &remote, Some("nope"), None).await;

    clone_job::run(&app.state, project.id, None).await;

    let cloned = clone_job::wait_for_clone(&app.state, project.id, CLONE_TIMEOUT).await;
    assert_eq!(cloned.status, ProjectStatus::Error);
    assert_eq!(
        cloned.status_message.as_deref(),
        Some("default branch \"nope\" not found on remote")
    );
    assert_eq!(
        cloned.default_branch.as_deref(),
        Some("nope"),
        "a failed clone does not rewrite what the user asked for"
    );
}

#[tokio::test]
async fn an_unreachable_remote_fails_with_git_s_own_words_and_no_credential() {
    let app = TestApp::spawn().await;
    let project = insert_project(&app, UNREACHABLE_REMOTE, None, None).await;

    clone_job::run(&app.state, project.id, None).await;

    let cloned = clone_job::wait_for_clone(&app.state, project.id, CLONE_TIMEOUT).await;
    assert_eq!(cloned.status, ProjectStatus::Error);

    let message = cloned.status_message.expect("a failure explains itself");
    assert!(!message.trim().is_empty(), "an empty reason helps nobody");
    assert!(
        !message.contains('@'),
        "a status message must not carry userinfo: {message}"
    );
    assert!(
        !message.contains('\n'),
        "a status message is one line: {message}"
    );
    assert!(
        message.chars().count() <= 1000,
        "a status message is bounded: {} characters",
        message.chars().count()
    );
}

#[tokio::test]
async fn an_empty_remote_says_it_has_no_branches() {
    let app = TestApp::spawn().await;
    let upstream = BareFixture::empty();

    let remote = upstream.url();
    let project = insert_project(&app, &remote, None, None).await;

    clone_job::run(&app.state, project.id, None).await;

    let cloned = clone_job::wait_for_clone(&app.state, project.id, CLONE_TIMEOUT).await;
    assert_eq!(cloned.status, ProjectStatus::Error);
    assert_eq!(
        cloned.status_message.as_deref(),
        Some("remote has no branches")
    );
}

#[tokio::test]
async fn a_retry_after_the_remote_appears_re_initialises_the_repository() {
    let app = TestApp::spawn().await;
    let upstream = BareFixture::new();
    upstream.remove();

    // The remote is not there yet, which is the ordinary mistyped-URL case.
    let remote = upstream.url();
    let project = insert_project(&app, &remote, None, None).await;

    clone_job::run(&app.state, project.id, None).await;
    let failed = clone_job::wait_for_clone(&app.state, project.id, CLONE_TIMEOUT).await;
    assert_eq!(failed.status, ProjectStatus::Error);
    assert!(failed.status_message.is_some());

    // The repository of the failed attempt is left in place, and a file put
    // inside it now proves the retry replaced it rather than reused it.
    let repo = repo_git(&app, project.id);
    assert!(repo.is_dir(), "the failed attempt's repository was removed");
    std::fs::write(repo.join(STALE_MARKER), "stale\n").expect("the marker is written");

    upstream.recreate();

    let mut tx = app.pool.begin().await.expect("a transaction begins");
    ProjectRepository::new(&app.pool)
        .mark_cloning_from_error(&mut tx, project.id)
        .await
        .expect("the retry transition runs")
        .expect("the project is in error");
    tx.commit().await.expect("the transaction commits");

    clone_job::run(&app.state, project.id, None).await;

    let cloned = clone_job::wait_for_clone(&app.state, project.id, CLONE_TIMEOUT).await;
    assert_eq!(
        cloned.status,
        ProjectStatus::Ready,
        "{:?}",
        cloned.status_message
    );
    assert_eq!(cloned.default_branch.as_deref(), Some("main"));
    assert_eq!(
        cloned.status_message, None,
        "the stale reason was not cleared"
    );
    assert!(
        !repo.join(STALE_MARKER).exists(),
        "the retry reused the interrupted attempt's repository"
    );
    assert!(ref_exists(&repo, "refs/heads/main").await);
}

#[tokio::test]
async fn a_project_deleted_while_it_clones_takes_its_directory_with_it() {
    let app = TestApp::spawn().await;
    let upstream = BareFixture::new();

    let remote = upstream.url();
    let project = insert_project(&app, &remote, None, None).await;
    let layout = app.state.config.project_layout(project.id);

    // The delete is prepared but not committed, so the job still reads the row
    // and does its work — and its own guarded `UPDATE` then waits on this row
    // lock instead of racing it. Whether the commit below lands before or
    // after that statement, it matches no row.
    let mut tx = app.pool.begin().await.expect("a transaction begins");
    sqlx::query("DELETE FROM projects WHERE id = $1")
        .bind(project.id)
        .execute(&mut *tx)
        .await
        .expect("the project row is deleted");

    let job = clone_job::spawn(app.state.clone(), project.id, None);

    // The CLI state directory appears right after the job has read the row, so
    // committing now cannot make the job stop before it created anything.
    wait_for_path(&layout.claude_dir(), CLONE_TIMEOUT).await;
    tx.commit().await.expect("the deletion commits");

    job.await.expect("the clone job does not panic");

    assert!(
        ProjectRepository::new(&app.pool)
            .find(project.id)
            .await
            .expect("the projects table is readable")
            .is_none(),
        "the row came back"
    );
    assert!(
        !layout.root().exists(),
        "the deleted project's directory was left behind: {}",
        layout.root().display()
    );
}

#[tokio::test]
async fn the_credential_lookup_is_recorded_as_a_git_use_by_the_requesting_user() {
    let app = TestApp::spawn().await;
    let upstream = BareFixture::new();

    let user = app
        .insert_user("owner", "owner@example.invalid", false, false)
        .await;
    let remote = upstream.url();
    let project = insert_project(&app, &remote, None, Some(user.id)).await;

    set_project_git_credential(
        &app.pool,
        &app.state.keyring,
        project.id,
        Zeroizing::new(FAKE_PAT.to_string()),
        Some(user.id),
    )
    .await
    .expect("the fake credential is stored");

    // The real provider rather than the mock: what is asserted here is the
    // `secret_uses` row the lookup writes, which only it does.
    let mut state = app.state.clone();
    state.git_credentials = Arc::new(PatCredentialProvider::new(
        app.pool.clone(),
        app.state.keyring.clone(),
        CommitIdentity {
            name: "Mars Bot".to_string(),
            email: "bot@example.invalid".to_string(),
        },
    ));

    clone_job::run(&state, project.id, Some(user.id)).await;

    let cloned = clone_job::wait_for_clone(&state, project.id, CLONE_TIMEOUT).await;
    assert_eq!(
        cloned.status,
        ProjectStatus::Ready,
        "{:?}",
        cloned.status_message
    );

    let uses: Vec<(String, Option<Uuid>, Option<Uuid>)> =
        sqlx::query_as("SELECT purpose, user_id, session_id FROM secret_uses ORDER BY id")
            .fetch_all(&app.pool)
            .await
            .expect("the secret_uses table is readable");

    assert_eq!(
        uses,
        vec![("git".to_string(), Some(user.id), None)],
        "the clone's credential use is a git use by the requesting user"
    );
}
