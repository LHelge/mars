//! `fetch_project`: the readiness gate, the credential actor, the
//! `last_fetched_at` write and the `max_age` skip (`ARCHITECTURE.md`, "Git
//! model", Project clone; "Launch sequence"; "Background jobs";
//! `docs/data-model.md`, `projects`).
//!
//! `tests/git_fetch.rs` covers what the fetch does to refs against a real
//! upstream and needs no database. What needs one is everything around it:
//! that a project which is not `ready` is refused before anything else
//! happens, that the credential is asked for with the actor the caller named —
//! `System` for the cron job, a user for `POST /projects/{id}/fetch` — and
//! that a fetch which ran moves `projects.last_fetched_at` while one skipped
//! for being recent leaves it exactly as it was.
//!
//! The upstream is a real bare repository in a `tempfile` directory reached
//! over git's `file` transport; the project row's `remote_url` stays the fake
//! `https` fixture value, because what the fetch follows is the mirror's own
//! `remote.origin.url`.
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use std::time::Duration;

use common::TestApp;
use mars_orchestrator::git::mock::MockGitCredentialProvider;
use mars_orchestrator::git::testutil::TestUpstream;
use mars_orchestrator::git::{DataPaths, GitActor, GitRef, fetch_project, init_project_repo, refs};
use mars_orchestrator::models::{BranchName, NewProject, Project, ProjectStatus, RemoteUrl};
use mars_orchestrator::prelude::*;
use mars_orchestrator::repositories::ProjectRepository;
use uuid::Uuid;

/// Not a real remote: the fixture value every project test stores (rule 3).
const TEST_REMOTE: &str = "https://git.example.com/fake/repo.git";

/// The freshness window a fresh session launch passes
/// (`ARCHITECTURE.md`, "Launch sequence").
const LAUNCH_MAX_AGE: Duration = Duration::from_secs(30);

/// A `ready` project with a real repository on disk, pointed at `upstream`.
///
/// The row is inserted `cloning` and moved to `ready` the way the clone job
/// does it, because `projects` refuses `ready` without a `default_branch`
/// (`docs/data-model.md`, `projects`).
async fn seed_ready_project(app: &TestApp, name: &str, upstream: &TestUpstream) -> Project {
    let mut new_project = NewProject::new(name, TEST_REMOTE).expect("the test project is valid");
    new_project.default_branch = Some(BranchName::parse("main").expect("main is a branch name"));

    let projects = ProjectRepository::new(&app.pool);

    let mut tx = app.pool.begin().await.expect("a transaction begins");
    let inserted = projects
        .insert(&mut tx, &new_project)
        .await
        .expect("the project inserts");
    tx.commit().await.expect("the transaction commits");

    // The repository the fetch will run in, created exactly as the clone job
    // creates it and under the very lock table `fetch_project` waits on.
    let paths = DataPaths::from_config(&app.state.config);
    {
        let guard = app.state.git_locks.lock(inserted.id).await;
        init_project_repo(
            &guard,
            &paths,
            &RemoteUrl::local_for_tests(&upstream.path),
            Some("main"),
            None,
        )
        .await
        .expect("the project repository is initialised");
    }

    set_status(app, inserted.id, ProjectStatus::Ready).await
}

/// Move a project to `status`, returning the stored row.
async fn set_status(app: &TestApp, id: Uuid, status: ProjectStatus) -> Project {
    let mut tx = app.pool.begin().await.expect("a transaction begins");
    let updated = ProjectRepository::new(&app.pool)
        .set_status(&mut tx, id, status, None)
        .await
        .expect("the status is set")
        .expect("the project exists");
    tx.commit().await.expect("the transaction commits");

    updated
}

/// The project row as it is now.
async fn reload(app: &TestApp, id: Uuid) -> Project {
    ProjectRepository::new(&app.pool)
        .find(id)
        .await
        .expect("the lookup runs")
        .expect("the project exists")
}

/// The commit a ref in the project repository points at, by its API name.
async fn commit_of(app: &TestApp, project_id: Uuid, name: &str) -> String {
    let repo = DataPaths::from_config(&app.state.config).project_repo(project_id);
    let git_ref = GitRef::parse(name).expect("a parsable ref name");

    refs::resolve(&repo, &git_ref)
        .await
        .unwrap_or_else(|err| panic!("{name} resolves: {err}"))
        .commit
}

#[tokio::test]
async fn the_cron_job_s_fetch_records_the_time_and_asks_as_the_system() {
    let app = TestApp::spawn().await;
    let upstream = TestUpstream::create().await;
    let project = seed_ready_project(&app, "cron-fetch", &upstream).await;

    assert!(
        project.last_fetched_at.is_none(),
        "initialisation does not record a fetch; fetch_project does"
    );

    // Upstream moves on between the initialisation fetch and this one.
    let advanced = upstream
        .commit_file("main", "NEWS.md", "news\n", "docs: add news")
        .await;

    let outcome = fetch_project(&app.state, project.id, &GitActor::System, None)
        .await
        .expect("the fetch succeeds");

    assert!(outcome.fetched, "no max_age means always fetch");
    assert_eq!(
        commit_of(&app, project.id, "origin/main").await,
        advanced,
        "upstream tracking was not refreshed"
    );

    let stored = reload(&app, project.id).await;
    assert_eq!(
        stored.last_fetched_at,
        Some(outcome.at),
        "the outcome's timestamp is the one that was written"
    );

    // `secret_uses` records neither a session nor a user for the mirror-fetch
    // job (`docs/data-model.md`, `secret_uses`), which is what the actor
    // decides.
    assert_eq!(
        app.mock_git().requested(),
        vec![(project.id, GitActor::System)],
        "the credential was asked for once, as the system"
    );
}

#[tokio::test]
async fn a_recent_fetch_is_skipped_and_leaves_the_timestamp_alone() {
    let app = TestApp::spawn().await;
    let upstream = TestUpstream::create().await;
    let project = seed_ready_project(&app, "skip-fetch", &upstream).await;

    let first = fetch_project(&app.state, project.id, &GitActor::System, None)
        .await
        .expect("the first fetch succeeds");
    assert!(first.fetched);

    // What a fresh launch passes: a fetch this recent is not worth repeating.
    let second = fetch_project(
        &app.state,
        project.id,
        &GitActor::User(Uuid::new_v4()),
        Some(LAUNCH_MAX_AGE),
    )
    .await
    .expect("the skipped call still succeeds");

    assert!(!second.fetched, "a fetch seconds old was repeated");
    assert_eq!(
        second.at, first.at,
        "the skip reports when the repository was actually fetched"
    );
    assert_eq!(
        reload(&app, project.id).await.last_fetched_at,
        Some(first.at)
    );

    assert_eq!(
        app.mock_git().requested(),
        vec![(project.id, GitActor::System)],
        "a skipped fetch must not read the credential at all"
    );
}

#[tokio::test]
async fn a_max_age_a_project_has_never_met_still_fetches() {
    let app = TestApp::spawn().await;
    let upstream = TestUpstream::create().await;
    let project = seed_ready_project(&app, "never-fetched", &upstream).await;
    let ada = app
        .insert_user("ada", "ada@example.com", false, false)
        .await;

    // `last_fetched_at` is null, so there is nothing to be fresh.
    let outcome = fetch_project(
        &app.state,
        project.id,
        &GitActor::User(ada.id),
        Some(LAUNCH_MAX_AGE),
    )
    .await
    .expect("the fetch succeeds");

    assert!(
        outcome.fetched,
        "a project that has never been fetched must"
    );
    assert_eq!(
        app.mock_git().requested(),
        vec![(project.id, GitActor::User(ada.id))],
        "a launch asks as the launching user"
    );
}

#[tokio::test]
async fn a_project_that_is_not_ready_is_refused_before_anything_else() {
    let app = TestApp::spawn().await;
    let upstream = TestUpstream::create().await;
    let project = seed_ready_project(&app, "not-ready", &upstream).await;

    for status in [ProjectStatus::Cloning, ProjectStatus::Error] {
        set_status(&app, project.id, status).await;

        let error = fetch_project(&app.state, project.id, &GitActor::System, None)
            .await
            .expect_err("a project that is not ready cannot be fetched");

        assert!(
            matches!(error, Error::Conflict(ref message) if message == "project is not ready"),
            "expected the documented conflict for {status:?}, got {error:?}"
        );
    }

    assert!(
        app.mock_git().requested().is_empty(),
        "the refusal happened before the credential was read"
    );
    assert!(reload(&app, project.id).await.last_fetched_at.is_none());
}

#[tokio::test]
async fn an_unknown_project_is_not_found() {
    let app = TestApp::spawn().await;

    let error = fetch_project(&app.state, Uuid::new_v4(), &GitActor::System, None)
        .await
        .expect_err("there is no such project");

    assert!(
        matches!(error, Error::NotFound),
        "expected a not-found, got {error:?}"
    );
}

#[tokio::test]
async fn a_failed_fetch_leaves_the_timestamp_unchanged() {
    let app = TestApp::spawn().await;
    let upstream = TestUpstream::create().await;
    let project = seed_ready_project(&app, "unreachable", &upstream).await;

    let first = fetch_project(&app.state, project.id, &GitActor::System, None)
        .await
        .expect("the first fetch succeeds");

    // The upstream becomes unreachable, as a network failure or a revoked
    // credential makes it.
    std::fs::remove_dir_all(&upstream.path).expect("the fixture upstream is removed");

    let error = fetch_project(&app.state, project.id, &GitActor::System, None)
        .await
        .expect_err("an upstream that is not there cannot be fetched");

    assert!(
        error.status().is_server_error(),
        "an unreachable upstream is an internal fault, got {error:?}"
    );
    assert_eq!(
        reload(&app, project.id).await.last_fetched_at,
        Some(first.at),
        "a failed fetch recorded itself"
    );
}

#[tokio::test]
async fn a_configured_credential_reaches_the_fetch_and_leaves_nothing_behind() {
    let app = TestApp::spawn().await;
    let upstream = TestUpstream::create().await;
    let project = seed_ready_project(&app, "credentialed", &upstream).await;

    // An obviously fake credential (rule 3); a local upstream ignores the
    // header it adds, so what this asserts is that the temporary config the
    // header travels in is written and then gone.
    app.mock_git().set_credential(
        project.id,
        Some(MockGitCredentialProvider::fake_credential()),
    );

    let outcome = fetch_project(&app.state, project.id, &GitActor::System, None)
        .await
        .expect("a local upstream ignores the credential header");
    assert!(outcome.fetched);

    let tmp = DataPaths::from_config(&app.state.config).tmp();
    let leftovers: Vec<String> = std::fs::read_dir(&tmp)
        .map(|entries| {
            entries
                .map(|entry| {
                    entry
                        .expect("a readable directory entry")
                        .file_name()
                        .to_string_lossy()
                        .into_owned()
                })
                .collect()
        })
        .unwrap_or_default();

    assert!(
        !leftovers.iter().any(|name| name.starts_with("gitcfg-")),
        "a credential config was left in DATA_DIR/tmp: {leftovers:?}"
    );
}

#[tokio::test]
async fn a_fetch_never_moves_the_integration_head_of_a_ready_project() {
    let app = TestApp::spawn().await;
    let upstream = TestUpstream::create().await;
    let project = seed_ready_project(&app, "ownership", &upstream).await;

    let seeded_main = commit_of(&app, project.id, "main").await;
    let advanced = upstream
        .commit_file("main", "NEWS.md", "news\n", "docs: add news")
        .await;

    fetch_project(&app.state, project.id, &GitActor::System, None)
        .await
        .expect("the fetch succeeds");

    assert_eq!(
        commit_of(&app, project.id, "main").await,
        seeded_main,
        "the fetch moved Mars's integration head (ADR 0017)"
    );
    assert_eq!(commit_of(&app, project.id, "origin/main").await, advanced);
}
