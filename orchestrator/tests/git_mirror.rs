//! Project repository initialisation against real repositories
//! (`ARCHITECTURE.md`, "Git model", Project clone; ADR 0001, ADR 0017).
//!
//! Git is never mocked (`CLAUDE.md`, "Testing expectations"), so every case
//! here runs `init_project_repo` against a bare upstream in a `tempfile`
//! directory, reached over git's `file` transport. What the unit tests beside
//! the module cannot show is exactly what this asserts: the settings the
//! repository ends up with, which refs were seeded and which were not, and
//! that a second run changes nothing it must not.
//!
//! No database and no `TestApp`: nothing in this file touches Postgres, so it
//! needs no container engine either.
//!
//! The one credential here is an obviously fake stand-in for a PAT (rule 3);
//! a local upstream ignores the header it produces, which is the point — what
//! is asserted is that the temporary config carrying it is gone afterwards.

#![cfg(feature = "integration-tests")]

use std::path::Path;

use mars_orchestrator::git::testutil::{TestUpstream, run_git};
use mars_orchestrator::git::{
    DataPaths, GitCommand, GitCredential, GitError, GitRef, ProjectGitGuard, ProjectGitLocks,
    init_project_repo, refs, remove_project_repo,
};
use mars_orchestrator::models::RemoteUrl;
use tempfile::TempDir;
use uuid::Uuid;

/// Not a real PAT: an obviously fake stand-in (rule 3).
const FAKE_PAT: &str = "ghp_FAKE_TEST_TOKEN_0000000000";

/// The prefix `CredentialConfig` gives its temporary files.
const CONFIG_PREFIX: &str = "gitcfg-";

/// The two refspecs `ARCHITECTURE.md` names, in order.
const EXPECTED_REFSPECS: [&str; 2] = [
    "+refs/heads/*:refs/remotes/origin/*",
    "+refs/tags/*:refs/tags/*",
];

/// A `DATA_DIR` and the project git lock, for one test.
///
/// The guard is held for the whole test, which is what the production callers
/// do: initialisation runs under the project git lock and the helpers never
/// take it themselves (`ARCHITECTURE.md`, "Git model", Serialization).
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

    /// The project repository this data directory's project would use.
    fn repo(&self) -> std::path::PathBuf {
        self.paths.project_repo(self.guard.project_id())
    }

    /// The names of the files left under `DATA_DIR/tmp`, if any.
    fn tmp_entries(&self) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(self.paths.tmp()) else {
            return Vec::new();
        };

        entries
            .map(|entry| {
                entry
                    .expect("a readable directory entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect()
    }
}

/// Every value configured for `key`, or an empty list when the key is unset.
///
/// `git config --get-all` exits 1 for an unset key, which is an answer rather
/// than a failure, so this goes through [`GitCommand::run`].
async fn config_values(repo: &Path, key: &str) -> Vec<String> {
    let output = GitCommand::new()
        .args(["config", "--get-all", "--end-of-options", key])
        .cwd(repo)
        .run()
        .await
        .expect("git config runs");

    if output.status != 0 {
        return Vec::new();
    }

    output.stdout.lines().map(str::to_string).collect()
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

/// Where `HEAD` points.
async fn head_symref(repo: &Path) -> String {
    GitCommand::new()
        .args(["symbolic-ref", "--end-of-options", "HEAD"])
        .cwd(repo)
        .run_ok()
        .await
        .expect("HEAD is symbolic")
        .stdout
        .trim()
        .to_string()
}

/// An upstream with `main`, `feature/x` and an annotated tag on `main`.
async fn upstream_with_two_branches_and_a_tag() -> TestUpstream {
    let upstream = TestUpstream::create().await;
    upstream
        .commit_file("feature/x", "x.txt", "x\n", "feat: start x")
        .await;
    upstream.tag("v1.0.0", "main", true).await;
    upstream
}

#[tokio::test]
async fn a_fresh_repository_is_configured_seeded_and_pointed_at_the_discovered_default() {
    let upstream = upstream_with_two_branches_and_a_tag().await;
    let data = DataDir::create().await;
    let remote = RemoteUrl::local_for_tests(&upstream.path);

    let outcome = init_project_repo(&data.guard, &data.paths, &remote, None, None)
        .await
        .expect("the repository is initialised");

    // Discovery, not a guess: the fixture upstream's HEAD is `main`.
    assert_eq!(outcome.default_branch, "main");
    let mut seeded = outcome.seeded.clone();
    seeded.sort();
    assert_eq!(seeded, vec!["feature/x".to_string(), "main".to_string()]);

    let repo = data.repo();

    // Each integration head is its upstream counterpart, including the one
    // whose name has a slash in it.
    for branch in ["main", "feature/x"] {
        assert_eq!(
            commit_of(&repo, branch).await,
            commit_of(&repo, &format!("origin/{branch}")).await,
            "{branch} was not seeded at its upstream commit"
        );
    }

    assert_eq!(head_symref(&repo).await, "refs/heads/main");

    // The settings ADR 0001 depends on: objects a session clone borrows are
    // never collected.
    assert_eq!(config_values(&repo, "gc.auto").await, vec!["0".to_string()]);
    assert_eq!(
        config_values(&repo, "gc.pruneExpire").await,
        vec!["never".to_string()]
    );
    assert_eq!(
        config_values(&repo, "core.logAllRefUpdates").await,
        vec!["true".to_string()],
        "reflogs are what an operator recovers a bad integration from"
    );

    assert_eq!(
        config_values(&repo, "remote.origin.fetch").await,
        EXPECTED_REFSPECS
    );
    assert!(
        config_values(&repo, "remote.origin.mirror")
            .await
            .is_empty(),
        "the project repository is not a mirror (ADR 0017)"
    );
    assert_eq!(
        config_values(&repo, "remote.origin.url").await,
        vec![upstream.path.display().to_string()],
        "the stored URL is configured as it is, with nothing added"
    );

    // The tag came with the second refspec and stayed a tag.
    assert!(ref_exists(&repo, "refs/tags/v1.0.0").await);
    assert!(
        !ref_exists(&repo, "refs/heads/v1.0.0").await,
        "a tag was seeded as an integration head"
    );
}

#[tokio::test]
async fn a_requested_default_branch_is_used_instead_of_the_discovered_one() {
    let upstream = upstream_with_two_branches_and_a_tag().await;
    let data = DataDir::create().await;
    let remote = RemoteUrl::local_for_tests(&upstream.path);

    let outcome = init_project_repo(&data.guard, &data.paths, &remote, Some("feature/x"), None)
        .await
        .expect("the requested branch is accepted");

    assert_eq!(outcome.default_branch, "feature/x");
    assert_eq!(head_symref(&data.repo()).await, "refs/heads/feature/x");
}

#[tokio::test]
async fn a_default_branch_upstream_does_not_have_fails_and_leaves_the_repository_in_place() {
    let upstream = TestUpstream::create().await;
    let data = DataDir::create().await;
    let remote = RemoteUrl::local_for_tests(&upstream.path);

    let error = init_project_repo(&data.guard, &data.paths, &remote, Some("nope"), None)
        .await
        .expect_err("a branch upstream does not have cannot be the default");

    match error {
        GitError::UnknownRef(name) => assert_eq!(name, "nope"),
        other => panic!("expected an unknown ref, got {other:?}"),
    }

    // The fetch already ran, and `retry-clone` reruns this routine on the
    // repository that is still there.
    let repo = data.repo();
    assert!(repo.join("HEAD").exists(), "the repository was removed");
    assert!(ref_exists(&repo, "refs/remotes/origin/main").await);

    let outcome = init_project_repo(&data.guard, &data.paths, &remote, Some("main"), None)
        .await
        .expect("the retry succeeds on the same repository");
    assert_eq!(outcome.default_branch, "main");
    assert!(
        outcome.seeded.is_empty(),
        "the failed run had already seeded every head: {:?}",
        outcome.seeded
    );
    assert_eq!(head_symref(&repo).await, "refs/heads/main");
}

#[tokio::test]
async fn an_upstream_with_no_branches_has_no_default_to_discover() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    run_git(dir.path(), &["init", "--bare", "--quiet", "empty.git"]).await;

    let data = DataDir::create().await;
    let remote = RemoteUrl::local_for_tests(&dir.path().join("empty.git"));

    let error = init_project_repo(&data.guard, &data.paths, &remote, None, None)
        .await
        .expect_err("an empty upstream cannot be cloned into a ready project");

    // Either spelling is the same fact, and which one depends on whether the
    // empty upstream advertises a symbolic HEAD at all: nothing resolves to an
    // integration head, so the clone job reports `status: error`.
    assert!(
        matches!(error, GitError::UnknownRef(_)),
        "expected an unknown ref, got {error:?}"
    );
}

#[tokio::test]
async fn a_second_run_refreshes_upstream_tracking_and_never_moves_an_integration_head() {
    let upstream = TestUpstream::create().await;
    let data = DataDir::create().await;
    let remote = RemoteUrl::local_for_tests(&upstream.path);

    init_project_repo(&data.guard, &data.paths, &remote, None, None)
        .await
        .expect("the first run succeeds");

    let repo = data.repo();
    let seeded_main = commit_of(&repo, "main").await;

    // Upstream moves on and grows a branch the first run never saw.
    let advanced = upstream
        .commit_file("main", "NEWS.md", "news\n", "docs: add news")
        .await;
    upstream
        .commit_file("feature/y", "y.txt", "y\n", "feat: start y")
        .await;

    let outcome = init_project_repo(&data.guard, &data.paths, &remote, None, None)
        .await
        .expect("the second run succeeds");

    assert_eq!(
        outcome.seeded,
        vec!["feature/y".to_string()],
        "only the head that was missing is seeded"
    );
    assert_eq!(
        commit_of(&repo, "main").await,
        seeded_main,
        "an integration head was moved by a re-run (ADR 0017)"
    );
    assert_eq!(
        commit_of(&repo, "origin/main").await,
        advanced,
        "upstream tracking was not refreshed"
    );

    // Re-applying the config appends nothing.
    assert_eq!(
        config_values(&repo, "remote.origin.fetch").await,
        EXPECTED_REFSPECS
    );
    assert_eq!(config_values(&repo, "gc.auto").await, vec!["0".to_string()]);
}

#[tokio::test]
async fn a_credential_config_never_outlives_the_command_it_was_written_for() {
    let upstream = TestUpstream::create().await;
    let data = DataDir::create().await;
    let remote = RemoteUrl::local_for_tests(&upstream.path);
    let credential = GitCredential::from_pat(FAKE_PAT).expect("the fake PAT encodes");

    init_project_repo(&data.guard, &data.paths, &remote, None, Some(&credential))
        .await
        .expect("a local upstream ignores the header the credential adds");

    assert!(
        !data
            .tmp_entries()
            .iter()
            .any(|name| name.starts_with(CONFIG_PREFIX)),
        "a credential config was left in DATA_DIR/tmp: {:?}",
        data.tmp_entries()
    );

    // The same must hold when the command fails, which is the path a
    // destructor rather than a return value covers.
    let missing = RemoteUrl::local_for_tests(Path::new("/nonexistent/upstream.git"));
    let error = init_project_repo(&data.guard, &data.paths, &missing, None, Some(&credential))
        .await
        .expect_err("an upstream that is not there cannot be reached");

    assert!(
        matches!(error, GitError::Command { .. }),
        "expected a git failure, got {error:?}"
    );
    assert!(
        !data
            .tmp_entries()
            .iter()
            .any(|name| name.starts_with(CONFIG_PREFIX)),
        "a credential config survived a failed command: {:?}",
        data.tmp_entries()
    );
}

#[tokio::test]
async fn removing_the_repository_is_idempotent() {
    let upstream = TestUpstream::create().await;
    let data = DataDir::create().await;
    let remote = RemoteUrl::local_for_tests(&upstream.path);

    init_project_repo(&data.guard, &data.paths, &remote, None, None)
        .await
        .expect("the repository is initialised");
    assert!(data.repo().exists());

    remove_project_repo(&data.guard, &data.paths)
        .await
        .expect("the repository is removed");
    assert!(!data.repo().exists());

    remove_project_repo(&data.guard, &data.paths)
        .await
        .expect("removing a repository that is already gone is the wanted state");
}
