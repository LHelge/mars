//! The recurring mirror fetch and the project branch listing, against real
//! repositories (`ARCHITECTURE.md`, "Git model", Project clone and Ref
//! ownership; `SPEC.md`, "Projects"; ADR 0017).
//!
//! Git is never mocked (`CLAUDE.md`, "Testing expectations"), and the property
//! this file exists for cannot be asserted any other way: that a fetch
//! refreshes and prunes upstream-tracking refs and tags while leaving every
//! ref Mars owns exactly where it was, *including* an integration head that
//! carries a merge upstream has never seen. That is the one guarantee ADR 0017
//! separated the namespaces for, so it is tested against a real upstream over
//! git's `file` transport rather than against a mock that could only ever
//! restate the implementation.
//!
//! No database and no `TestApp`: nothing here touches Postgres, so it needs no
//! container engine. `fetch_project`, which does need both, is
//! `tests/git_fetch_project.rs`.

#![cfg(feature = "integration-tests")]

use std::path::{Path, PathBuf};

use mars_orchestrator::git::testutil::{TestUpstream, run_git};
use mars_orchestrator::git::{
    DataPaths, GitError, GitRef, ProjectGitGuard, ProjectGitLocks, fetch_upstream,
    init_project_repo, list_branches, refs,
};
use mars_orchestrator::models::{BranchKind, RemoteUrl};
use tempfile::TempDir;
use uuid::Uuid;

/// A `DATA_DIR`, one project id and that project's git lock, held for the
/// whole test as the production callers hold it (`ARCHITECTURE.md`, "Git
/// model", Serialization).
struct DataDir {
    _dir: TempDir,
    paths: DataPaths,
    guard: ProjectGitGuard,
}

impl DataDir {
    async fn create() -> Self {
        let dir = tempfile::tempdir().expect("a temporary data directory");
        let paths = DataPaths::new(dir.path());
        let guard = ProjectGitLocks::new().lock(Uuid::new_v4()).await;

        Self {
            _dir: dir,
            paths,
            guard,
        }
    }

    fn project_id(&self) -> Uuid {
        self.guard.project_id()
    }

    fn repo(&self) -> PathBuf {
        self.paths.project_repo(self.project_id())
    }

    /// An initialised project repository pointed at `upstream`.
    async fn initialised(upstream: &TestUpstream) -> Self {
        let data = Self::create().await;
        init_project_repo(
            &data.guard,
            &data.paths,
            &RemoteUrl::local_for_tests(&upstream.path),
            None,
            None,
        )
        .await
        .expect("the repository is initialised");

        data
    }

    /// The fetch under test, unauthenticated: a local upstream needs no
    /// credential, and the credential path is covered by `tests/git_mirror.rs`.
    async fn fetch(&self) {
        fetch_upstream(&self.guard, &self.paths, None)
            .await
            .expect("the fetch succeeds against a local upstream");
    }
}

/// The commit a ref points at, by its API name (`main`, `origin/main`).
async fn commit_of(repo: &Path, name: &str) -> String {
    let git_ref = GitRef::parse(name).expect("a parsable ref name");
    refs::resolve(repo, &git_ref)
        .await
        .unwrap_or_else(|err| panic!("{name} resolves: {err}"))
        .commit
}

/// Does `full_name` exist in `repo`?
async fn ref_exists(repo: &Path, full_name: &str) -> bool {
    !refs::list(repo, &[full_name])
        .await
        .expect("for-each-ref runs")
        .is_empty()
}

/// Commit directly onto an integration head in the project repository,
/// standing in for a merge Mars made locally and has not pushed.
///
/// `commit-tree` rather than a clone and a push: the repository is bare, the
/// tree is deliberately unchanged, and what the test needs is a commit on
/// `refs/heads/<branch>` that upstream has never heard of.
async fn commit_on_integration_head(repo: &Path, branch: &str, message: &str) -> String {
    let full = format!("refs/heads/{branch}");
    // `--verify`, because a plain `rev-parse` echoes the options it did not
    // consume alongside the object id.
    let tree = run_git(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{full}^{{tree}}"),
        ],
    )
    .await
    .trim()
    .to_string();

    let commit = run_git(repo, &["commit-tree", "-p", &full, "-m", message, &tree])
        .await
        .trim()
        .to_string();

    refs::update(repo, &full, &commit, None)
        .await
        .expect("the integration head is moved");

    commit
}

#[tokio::test]
async fn a_fetch_refreshes_upstream_tracking_and_tags_without_moving_an_integration_head() {
    let upstream = TestUpstream::create().await;
    let data = DataDir::initialised(&upstream).await;
    let repo = data.repo();

    let seeded_main = commit_of(&repo, "main").await;

    // Upstream advances, grows a branch and gains a tag, all after the
    // initialisation fetch.
    let advanced = upstream
        .commit_file("main", "NEWS.md", "news\n", "docs: add news")
        .await;
    upstream
        .commit_file("feature/x", "x.txt", "x\n", "feat: start x")
        .await;
    upstream.tag("v1.0.0", "main", true).await;

    data.fetch().await;

    assert_eq!(
        commit_of(&repo, "origin/main").await,
        advanced,
        "upstream tracking was not refreshed"
    );
    assert!(
        ref_exists(&repo, "refs/remotes/origin/feature/x").await,
        "a branch added upstream after initialisation was not fetched"
    );
    assert!(
        ref_exists(&repo, "refs/tags/v1.0.0").await,
        "the second refspec did not bring the tag"
    );

    assert_eq!(
        commit_of(&repo, "main").await,
        seeded_main,
        "the fetch moved an integration head (ADR 0017)"
    );
    assert!(
        !ref_exists(&repo, "refs/heads/feature/x").await,
        "a fetch seeded an integration head; only initialisation seeds"
    );
}

#[tokio::test]
async fn pruning_removes_the_upstream_ref_and_keeps_the_integration_head() {
    let upstream = TestUpstream::create().await;
    upstream
        .commit_file("feature/x", "x.txt", "x\n", "feat: start x")
        .await;
    upstream.tag("v1.0.0", "main", true).await;

    // Initialisation seeds `refs/heads/feature/x` beside its tracking ref.
    let data = DataDir::initialised(&upstream).await;
    let repo = data.repo();
    let seeded_feature = commit_of(&repo, "feature/x").await;

    upstream.delete_branch("feature/x").await;
    upstream.delete_tag("v1.0.0").await;

    data.fetch().await;

    assert!(
        !ref_exists(&repo, "refs/remotes/origin/feature/x").await,
        "--prune left the tracking ref of a deleted upstream branch behind"
    );
    assert!(
        !ref_exists(&repo, "refs/tags/v1.0.0").await,
        "--prune left a tag upstream no longer has"
    );

    assert_eq!(
        commit_of(&repo, "feature/x").await,
        seeded_feature,
        "pruning upstream tracking deleted a Mars integration head (ADR 0017)"
    );
}

#[tokio::test]
async fn a_fetch_preserves_an_unpushed_integration_merge() {
    let upstream = TestUpstream::create().await;
    let data = DataDir::initialised(&upstream).await;
    let repo = data.repo();

    // What ADR 0017 exists for: a merge Mars made locally and has not pushed.
    let merged = commit_on_integration_head(&repo, "main", "merge: integrate session work").await;

    let advanced = upstream
        .commit_file("main", "NEWS.md", "news\n", "docs: add news")
        .await;

    data.fetch().await;

    assert_eq!(
        commit_of(&repo, "main").await,
        merged,
        "a scheduled fetch replaced an unpushed local merge (ADR 0017)"
    );
    assert_eq!(
        commit_of(&repo, "origin/main").await,
        advanced,
        "upstream tracking did not follow upstream"
    );
}

#[tokio::test]
async fn an_unreachable_upstream_fails_without_touching_any_ref() {
    let upstream = TestUpstream::create().await;
    let data = DataDir::initialised(&upstream).await;
    let repo = data.repo();

    let before = commit_of(&repo, "origin/main").await;

    // The upstream directory goes away underneath the project repository, as
    // an unreachable remote does.
    std::fs::remove_dir_all(&upstream.path).expect("the fixture upstream is removed");

    let error = fetch_upstream(&data.guard, &data.paths, None)
        .await
        .expect_err("an upstream that is not there cannot be fetched");

    assert!(
        matches!(error, GitError::Command { .. }),
        "expected a git command failure, got {error:?}"
    );
    assert_eq!(
        commit_of(&repo, "origin/main").await,
        before,
        "a failed fetch changed a ref"
    );
    assert!(ref_exists(&repo, "refs/heads/main").await);
}

#[tokio::test]
async fn the_branch_listing_is_the_three_kinds_in_the_documented_order() {
    let upstream = TestUpstream::create().await;
    upstream
        .commit_file("feature/x", "x.txt", "x\n", "feat: start x")
        .await;
    upstream.tag("v1.0.0", "main", true).await;

    let data = DataDir::initialised(&upstream).await;
    let repo = data.repo();

    // A session ref, as fetch-back would leave it.
    let session_id = Uuid::new_v4();
    let session_commit = commit_of(&repo, "main").await;
    refs::update(
        &repo,
        &format!("refs/sessions/{session_id}"),
        &session_commit,
        None,
    )
    .await
    .expect("the session ref is written");

    let branches = list_branches(&data.paths, data.project_id())
        .await
        .expect("the listing runs");

    let listed: Vec<(&str, BranchKind)> = branches
        .iter()
        .map(|branch| (branch.name.as_str(), branch.kind))
        .collect();
    let expected_session = session_id.to_string();
    assert_eq!(
        listed,
        vec![
            ("feature/x", BranchKind::Head),
            ("main", BranchKind::Head),
            ("origin/feature/x", BranchKind::Upstream),
            ("origin/main", BranchKind::Upstream),
            (expected_session.as_str(), BranchKind::Session),
        ],
        "heads, then upstream, then sessions, each alphabetically"
    );

    // The session ref is the only one carrying an id, and it is its own.
    let session = branches.last().expect("the listing is not empty");
    assert_eq!(session.session_id, Some(session_id));
    assert_eq!(session.commit, session_commit);
    assert!(
        branches
            .iter()
            .take(branches.len() - 1)
            .all(|branch| branch.session_id.is_none()),
        "a head or upstream ref carried a session id"
    );

    // A tag is not a branch, and neither is `origin/HEAD`.
    assert!(
        ref_exists(&repo, "refs/tags/v1.0.0").await,
        "the fixture tag is not in the repository at all"
    );
    assert!(
        !branches.iter().any(|branch| branch.name == "v1.0.0"),
        "a tag was listed as a branch: {branches:?}"
    );
    assert!(
        !branches.iter().any(|branch| branch.name == "origin/HEAD"),
        "origin/HEAD was listed as a branch: {branches:?}"
    );
}

#[tokio::test]
async fn a_repository_with_nothing_but_heads_lists_only_heads() {
    let upstream = TestUpstream::create().await;
    let data = DataDir::initialised(&upstream).await;

    let branches = list_branches(&data.paths, data.project_id())
        .await
        .expect("the listing runs");

    assert_eq!(
        branches
            .iter()
            .filter(|branch| branch.kind == BranchKind::Session)
            .count(),
        0,
        "a project with no sessions has no session refs"
    );
    assert_eq!(
        branches
            .iter()
            .map(|branch| branch.name.as_str())
            .collect::<Vec<_>>(),
        vec!["main", "origin/main"]
    );
}
