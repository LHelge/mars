//! The mirror fetch job: every `ready` mirror fetched as the system, with one
//! bad upstream costing only its own project (`ARCHITECTURE.md`, "Background
//! jobs"; "Git model", Project clone).
//!
//! `tests/git_fetch_project.rs` covers what one fetch does. What this suite
//! covers is the sweep around it: which projects are visited, that a project
//! which is not `ready` is left alone, that `last_fetched_at` moves only on
//! the rows that were fetched, that the credential is asked for as
//! [`GitActor::System`] every time — which is what makes the job's
//! `secret_uses` rows carry neither a session nor a user
//! (`docs/data-model.md`, `secret_uses`) — and that an unreachable upstream is
//! counted as a failure while the healthy project is still fetched.
//!
//! The upstreams are real bare repositories in `tempfile` directories reached
//! over git's `file` transport; each project row keeps the fake `https`
//! fixture `remote_url` (rule 3), because what a fetch follows is the mirror's
//! own `remote.origin.url`.
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use chrono::Utc;
use common::TestApp;
use mars_orchestrator::cron::JobReport;
use mars_orchestrator::git::testutil::TestUpstream;
use mars_orchestrator::git::{DataPaths, GitActor, GitRef, init_project_repo, refs};
use mars_orchestrator::models::{BranchName, NewProject, Project, ProjectStatus, RemoteUrl};
use mars_orchestrator::repositories::ProjectRepository;
use uuid::Uuid;

/// Not a real remote: the fixture value every project test stores (rule 3).
const TEST_REMOTE: &str = "https://git.example.com/fake/repo.git";

/// A `ready` project with a real repository on disk, pointed at `upstream`.
///
/// Inserted `cloning` and moved to `ready` the way the clone job does it,
/// because `projects` refuses `ready` without a `default_branch`
/// (`docs/data-model.md`, `projects`).
async fn seed_ready_project(app: &TestApp, name: &str, upstream: &TestUpstream) -> Project {
    let inserted = insert_project(app, name).await;

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

/// A project row in `cloning`, with no repository on disk: what the sweep must
/// walk past without touching.
async fn insert_project(app: &TestApp, name: &str) -> Project {
    let mut new_project = NewProject::new(name, TEST_REMOTE).expect("the test project is valid");
    new_project.default_branch = Some(BranchName::parse("main").expect("main is a branch name"));

    let mut tx = app.pool.begin().await.expect("a transaction begins");
    let inserted = ProjectRepository::new(&app.pool)
        .insert(&mut tx, &new_project)
        .await
        .expect("the project inserts");
    tx.commit().await.expect("the transaction commits");

    inserted
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
async fn the_sweep_fetches_every_ready_mirror_as_the_system() {
    let app = TestApp::spawn().await;

    let first_upstream = TestUpstream::create().await;
    let second_upstream = TestUpstream::create().await;
    let first = seed_ready_project(&app, "ready-one", &first_upstream).await;
    let second = seed_ready_project(&app, "ready-two", &second_upstream).await;

    // Neither of these has a repository on disk, and neither may be reached.
    let cloning = insert_project(&app, "still-cloning").await;
    let failed = insert_project(&app, "clone-failed").await;
    set_status(&app, failed.id, ProjectStatus::Error).await;

    let seeded_main = commit_of(&app, first.id, "main").await;

    // Upstream moves on after the initialisation fetch, so a stale mirror is
    // visibly stale.
    let first_advanced = first_upstream
        .commit_file("main", "NEWS.md", "news\n", "docs: add news")
        .await;
    let second_advanced = second_upstream
        .commit_file("main", "NEWS.md", "news\n", "docs: add news")
        .await;

    let report = app
        .cron()
        .mirror_fetch(Utc::now())
        .await
        .expect("the sweep runs");

    assert_eq!(
        report,
        JobReport {
            items: 2,
            skipped: 0,
            failures: 0
        },
        "only the two ready projects are work"
    );

    assert_eq!(
        commit_of(&app, first.id, "origin/main").await,
        first_advanced
    );
    assert_eq!(
        commit_of(&app, second.id, "origin/main").await,
        second_advanced
    );
    assert_eq!(
        commit_of(&app, first.id, "main").await,
        seeded_main,
        "the sweep moved Mars's integration head (ADR 0017)"
    );

    assert!(reload(&app, first.id).await.last_fetched_at.is_some());
    assert!(reload(&app, second.id).await.last_fetched_at.is_some());
    assert!(
        reload(&app, cloning.id).await.last_fetched_at.is_none(),
        "a cloning project was fetched"
    );
    assert!(
        reload(&app, failed.id).await.last_fetched_at.is_none(),
        "a project in error was fetched"
    );

    // The actor is what decides that the job's `secret_uses` rows carry
    // neither `session_id` nor `user_id` (`docs/data-model.md`).
    let mut requested = app.mock_git().requested();
    requested.sort_by_key(|(id, _)| *id);
    let mut expected = vec![(first.id, GitActor::System), (second.id, GitActor::System)];
    expected.sort_by_key(|(id, _)| *id);
    assert_eq!(
        requested, expected,
        "the credential was asked for once per ready project, as the system"
    );
}

#[tokio::test]
async fn an_unreachable_upstream_costs_only_its_own_project() {
    let app = TestApp::spawn().await;

    let healthy_upstream = TestUpstream::create().await;
    let doomed_upstream = TestUpstream::create().await;
    let healthy = seed_ready_project(&app, "healthy", &healthy_upstream).await;
    let doomed = seed_ready_project(&app, "doomed", &doomed_upstream).await;

    let first = app
        .cron()
        .mirror_fetch(Utc::now())
        .await
        .expect("the first sweep runs");
    assert_eq!(first.items, 2);

    let healthy_first = reload(&app, healthy.id)
        .await
        .last_fetched_at
        .expect("the healthy project was fetched");
    let doomed_first = reload(&app, doomed.id)
        .await
        .last_fetched_at
        .expect("the doomed project was fetched");

    // The upstream disappears, as a network failure or a revoked credential
    // makes it.
    std::fs::remove_dir_all(&doomed_upstream.path).expect("the fixture upstream is removed");
    let advanced = healthy_upstream
        .commit_file("main", "MORE.md", "more\n", "docs: add more")
        .await;

    let second = app
        .cron()
        .mirror_fetch(Utc::now())
        .await
        .expect("one bad upstream does not fail the sweep");

    assert_eq!(
        second,
        JobReport {
            items: 1,
            skipped: 0,
            failures: 1
        },
        "the failure was counted and the other project still fetched"
    );
    assert_eq!(
        commit_of(&app, healthy.id, "origin/main").await,
        advanced,
        "the healthy project was not fetched after its neighbour failed"
    );
    assert!(
        reload(&app, healthy.id)
            .await
            .last_fetched_at
            .expect("still set")
            > healthy_first,
        "the healthy project's fetch was not recorded"
    );
    assert_eq!(
        reload(&app, doomed.id).await.last_fetched_at,
        Some(doomed_first),
        "a failed fetch recorded itself"
    );
}

#[tokio::test]
async fn a_project_that_left_ready_between_listing_and_fetching_is_skipped() {
    let app = TestApp::spawn().await;

    let first_upstream = TestUpstream::create().await;
    let second_upstream = TestUpstream::create().await;
    let one = seed_ready_project(&app, "sweep-one", &first_upstream).await;
    let two = seed_ready_project(&app, "sweep-two", &second_upstream).await;

    // The sweep visits by id, so the project whose lock is held is whichever
    // sorts first and the one moved out of `ready` is the other.
    let (blocked, retried) = if one.id < two.id {
        (one.id, two.id)
    } else {
        (two.id, one.id)
    };

    // The window a retry-clone races with, made deterministic: the sweep lists
    // both projects, then blocks on the first one's git lock, and only then is
    // the second moved out of `ready`, which is the state `fetch_project`
    // finds when it re-reads the row.
    let guard = app.state.git_locks.lock(blocked).await;
    let cron = app.cron();
    let sweep = tokio::spawn(async move { cron.mirror_fetch(Utc::now()).await });

    // Long enough for one indexed listing query against a local Postgres.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    set_status(&app, retried, ProjectStatus::Error).await;
    drop(guard);

    let report = sweep
        .await
        .expect("the sweep task did not panic")
        .expect("the sweep runs");

    assert_eq!(
        report,
        JobReport {
            items: 1,
            skipped: 1,
            failures: 0
        },
        "a project that left `ready` after the listing is skipped, not failed"
    );

    // The next tick no longer lists it at all.
    set_status(&app, blocked, ProjectStatus::Error).await;
    assert_eq!(
        app.cron()
            .mirror_fetch(Utc::now())
            .await
            .expect("the sweep runs"),
        JobReport::default()
    );
}

#[tokio::test]
async fn no_projects_is_an_empty_report() {
    let app = TestApp::spawn().await;

    let report = app
        .cron()
        .mirror_fetch(Utc::now())
        .await
        .expect("the sweep runs");

    assert_eq!(report, JobReport::default());
    assert!(app.mock_git().requested().is_empty());
}
