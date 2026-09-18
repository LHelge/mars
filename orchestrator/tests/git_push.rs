//! Pushing one ref upstream, against real repositories (`ARCHITECTURE.md`,
//! "Git model", Merge, rebase, push and Credentials; `SPEC.md`, "Git" and the
//! MCP `push` tool; ADR 0002, ADR 0007, ADR 0017).
//!
//! Git is never mocked (`CLAUDE.md`, "Testing expectations"), and the
//! properties this file exists for are exactly the ones only a real remote
//! shows: that a push moves the one upstream branch it was given and no other
//! ref; that git updates `refs/remotes/origin/<branch>` itself afterwards
//! because the configured fetch refspec matches, without this code writing it;
//! that a diverged upstream produces a rejection which leaves *every* local
//! ref where it was, so a merge that was already made survives it; and that an
//! explicit `force` then publishes the same commit.
//!
//! There is no "merge into `main`" primitive to build the fixture on yet, so
//! "after a merge" is staged the way a merge would leave the repository: a
//! session commits in its work clone, `fetch_back` brings it into
//! `refs/sessions/<sid>`, and `refs::update` moves `refs/heads/main` onto that
//! commit. What is under test is the push, not how the integration head got
//! where it is.
//!
//! No database and no `TestApp`: nothing here touches Postgres, so it needs no
//! container engine.

#![cfg(feature = "integration-tests")]

use std::path::{Path, PathBuf};

use mars_orchestrator::git::testutil::{TestUpstream, run_git, test_identity};
use mars_orchestrator::git::{
    ComparePage, DataPaths, GitCommand, GitCredential, GitError, GitRef, ProjectGitGuard,
    ProjectGitLocks, ResolvedRef, create_work_clone, fetch_back, init_project_repo, push, refs,
    resolve_base, session_branch,
};
use mars_orchestrator::models::RemoteUrl;
use tempfile::TempDir;
use uuid::Uuid;

/// Not a real PAT: an obviously fake stand-in (`CLAUDE.md` rule 3).
const FAKE_PAT: &str = "ghp_FAKE_TEST_TOKEN_0000000000";

/// The prefix `CredentialConfig` gives its temporary files.
const CONFIG_PREFIX: &str = "gitcfg-";

/// A GitHub remote the compare link is built from. `acme/widgets` is not a
/// repository anyone has to own for this to be the right URL.
const GITHUB_REMOTE: &str = "https://github.com/acme/widgets.git";

/// An upstream, a `DATA_DIR`, the project git lock and an initialised project
/// repository pointed at that upstream.
///
/// The guard is held for the whole test, as the production caller holds it
/// across resolution and the push (`ARCHITECTURE.md`, "Git model",
/// Serialization).
struct Project {
    _data: TempDir,
    upstream: TestUpstream,
    paths: DataPaths,
    guard: ProjectGitGuard,
}

impl Project {
    async fn create() -> Self {
        let upstream = TestUpstream::create().await;
        let data = tempfile::tempdir().expect("a temporary data directory");
        let paths = DataPaths::new(data.path());
        let guard = ProjectGitLocks::new().lock(Uuid::new_v4()).await;

        init_project_repo(
            &guard,
            &paths,
            &RemoteUrl::local_for_tests(&upstream.path),
            None,
            None,
        )
        .await
        .expect("the project repository is initialised");

        Self {
            _data: data,
            upstream,
            paths,
            guard,
        }
    }

    /// The bare project repository every push runs in.
    fn repo(&self) -> PathBuf {
        self.paths.project_repo(self.guard.project_id())
    }

    /// Everything a session would leave behind: a work clone off `main`, one
    /// commit in it and that commit fetched back into `refs/sessions/<sid>`.
    /// Returns the session id and the commit.
    async fn session_with_a_commit(&self, file: &str) -> (Uuid, String) {
        let session_id = Uuid::new_v4();
        let base = resolve_base(&self.guard, &self.paths, None, "main")
            .await
            .expect("the default base resolves");

        create_work_clone(
            &self.guard,
            &self.paths,
            session_id,
            &base,
            &test_identity(),
        )
        .await
        .expect("the work clone is created");

        let work = self.paths.session_work(session_id);
        std::fs::write(work.join(file), "agent output\n").expect("the file is written");
        run_git(&work, &["add", "--", file]).await;
        run_git(&work, &["commit", "--quiet", "-m", "feat: agent work"]).await;

        let commit = fetch_back(&self.guard, &self.paths, session_id)
            .await
            .expect("the session branch is fetched back");

        (session_id, commit)
    }

    /// Put `refs/heads/main` at `commit`, which is where a merge into the
    /// integration head would have left it.
    async fn integrate_into_main(&self, commit: &str) {
        refs::update(&self.repo(), "refs/heads/main", commit, None)
            .await
            .expect("the integration head is moved");
    }

    /// Resolve one ref in the project repository, as the service does before
    /// calling [`push`].
    async fn resolve(&self, name: &str) -> ResolvedRef {
        let git_ref = GitRef::parse(name).expect("a parsable ref name");
        refs::resolve(&self.repo(), &git_ref)
            .await
            .unwrap_or_else(|err| panic!("{name} resolves: {err}"))
    }

    /// The commit a fully qualified ref points at in the project repository,
    /// or `None` when it is not there.
    async fn mirror_ref(&self, full_name: &str) -> Option<String> {
        rev_parse(&self.repo(), full_name).await
    }

    /// The same, in the upstream bare repository.
    async fn upstream_ref(&self, full_name: &str) -> Option<String> {
        rev_parse(&self.upstream.path, full_name).await
    }

    /// The file names currently in `DATA_DIR/tmp/`.
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

    /// Point `origin` at a path that is not there, so the next push cannot
    /// reach a remote at all.
    async fn break_the_remote(&self) {
        run_git(
            &self.repo(),
            &[
                "config",
                "--replace-all",
                "--end-of-options",
                "remote.origin.url",
                "/nonexistent/upstream.git",
            ],
        )
        .await;
    }
}

/// `git rev-parse --verify <full_name>`, as `None` when the ref is absent.
///
/// A missing ref is an answer here rather than a failure, so this goes through
/// [`GitCommand::run`].
async fn rev_parse(repo: &Path, full_name: &str) -> Option<String> {
    let output = GitCommand::new()
        .args(["rev-parse", "--verify", "--end-of-options", full_name])
        .cwd(repo)
        .run()
        .await
        .expect("git rev-parse runs");

    (output.status == 0).then(|| output.stdout.trim().to_string())
}

#[tokio::test]
async fn pushing_an_integration_head_moves_upstream_and_the_tracking_ref() {
    let project = Project::create().await;
    let (_session, commit) = project.session_with_a_commit("agent.txt").await;
    project.integrate_into_main(&commit).await;

    let outcome = push(
        &project.guard,
        &project.paths,
        &project.resolve("main").await,
        None,
        false,
        None,
        Some(ComparePage {
            remote_url: GITHUB_REMOTE,
            default_branch: "main",
        }),
    )
    .await
    .expect("the integration head is published");

    assert_eq!(outcome.remote_branch, "main");
    assert_eq!(outcome.commit, commit);
    assert_eq!(
        outcome.compare_url.as_deref(),
        Some("https://github.com/acme/widgets/compare/main...main?expand=1")
    );

    assert_eq!(
        project.upstream_ref("refs/heads/main").await.as_deref(),
        Some(commit.as_str()),
        "upstream main did not move to the pushed commit"
    );
    // The acceptance criterion this file is here for: git maintains upstream
    // tracking itself, because the configured fetch refspec matches. Nothing
    // in `push` writes a ref.
    assert_eq!(
        project
            .mirror_ref("refs/remotes/origin/main")
            .await
            .as_deref(),
        Some(commit.as_str()),
        "git did not update the tracking ref after the push"
    );
}

#[tokio::test]
async fn a_session_ref_is_published_as_the_session_branch() {
    let project = Project::create().await;
    let (session_id, commit) = project.session_with_a_commit("agent.txt").await;
    let before_main = project
        .upstream_ref("refs/heads/main")
        .await
        .expect("upstream has main");

    let outcome = push(
        &project.guard,
        &project.paths,
        &project.resolve(&refs::session_ref(session_id)).await,
        None,
        false,
        None,
        None,
    )
    .await
    .expect("the session ref is published");

    // `SPEC.md`, "MCP tool contracts": session refs are pushed as
    // `refs/heads/session/<id>` by default.
    assert_eq!(outcome.remote_branch, session_branch(session_id));
    assert_eq!(outcome.commit, commit);
    assert_eq!(outcome.compare_url, None, "no compare page was asked for");

    let published = format!("refs/heads/{}", session_branch(session_id));
    assert_eq!(
        project.upstream_ref(&published).await.as_deref(),
        Some(commit.as_str())
    );
    assert_eq!(
        project
            .mirror_ref(&format!(
                "refs/remotes/origin/{}",
                session_branch(session_id)
            ))
            .await
            .as_deref(),
        Some(commit.as_str())
    );
    // Only the selected ref: `main` upstream is exactly where it was
    // (`SPEC.md`, "Git": a push updates only the selected upstream branch).
    assert_eq!(
        project.upstream_ref("refs/heads/main").await.as_deref(),
        Some(before_main.as_str())
    );
}

#[tokio::test]
async fn a_requested_remote_branch_is_where_the_ref_lands() {
    let project = Project::create().await;
    let (session_id, commit) = project.session_with_a_commit("agent.txt").await;

    let outcome = push(
        &project.guard,
        &project.paths,
        &project.resolve(&refs::session_ref(session_id)).await,
        Some("feature/x"),
        false,
        None,
        Some(ComparePage {
            remote_url: GITHUB_REMOTE,
            default_branch: "main",
        }),
    )
    .await
    .expect("the session ref is published under the requested name");

    assert_eq!(outcome.remote_branch, "feature/x");
    assert_eq!(
        outcome.compare_url.as_deref(),
        Some("https://github.com/acme/widgets/compare/main...feature/x?expand=1")
    );
    assert_eq!(
        project
            .upstream_ref("refs/heads/feature/x")
            .await
            .as_deref(),
        Some(commit.as_str())
    );
    assert_eq!(
        project
            .upstream_ref(&format!("refs/heads/{}", session_branch(session_id)))
            .await,
        None,
        "the default name was published as well as the requested one"
    );
}

#[tokio::test]
async fn a_diverged_upstream_is_a_conflict_that_changes_nothing_and_force_then_publishes() {
    let project = Project::create().await;
    let (_session, commit) = project.session_with_a_commit("agent.txt").await;
    project.integrate_into_main(&commit).await;

    // Upstream moves on with work the project repository has never fetched:
    // exactly what makes the next ordinary push a non-fast-forward.
    let upstream_commit = project
        .upstream
        .commit_file("main", "other.txt", "elsewhere\n", "feat: upstream work")
        .await;
    let tracking_before = project
        .mirror_ref("refs/remotes/origin/main")
        .await
        .expect("the tracking ref exists");
    assert_ne!(tracking_before, upstream_commit);

    let error = push(
        &project.guard,
        &project.paths,
        &project.resolve("main").await,
        None,
        false,
        None,
        None,
    )
    .await
    .expect_err("a diverged upstream rejects the push");

    assert!(
        matches!(&error, GitError::NonFastForward { remote_branch } if remote_branch == "main"),
        "expected a non-fast-forward for main, got {error:?}"
    );

    // The whole point of the conflict: the local merge is still there and
    // upstream tracking has not silently moved (ADR 0017).
    assert_eq!(
        project.mirror_ref("refs/heads/main").await.as_deref(),
        Some(commit.as_str()),
        "the rejected push moved the integration head"
    );
    assert_eq!(
        project
            .mirror_ref("refs/remotes/origin/main")
            .await
            .as_deref(),
        Some(tracking_before.as_str()),
        "the rejected push moved the tracking ref"
    );
    assert_eq!(
        project.upstream_ref("refs/heads/main").await.as_deref(),
        Some(upstream_commit.as_str()),
        "the rejected push changed upstream"
    );

    let outcome = push(
        &project.guard,
        &project.paths,
        &project.resolve("main").await,
        None,
        true,
        None,
        None,
    )
    .await
    .expect("an explicitly forced push is allowed to discard upstream's commit");

    assert_eq!(outcome.commit, commit);
    assert_eq!(
        project.upstream_ref("refs/heads/main").await.as_deref(),
        Some(commit.as_str())
    );
    assert_eq!(
        project
            .mirror_ref("refs/remotes/origin/main")
            .await
            .as_deref(),
        Some(commit.as_str())
    );
}

#[tokio::test]
async fn pushing_a_commit_that_is_already_upstream_succeeds_with_the_same_commit() {
    let project = Project::create().await;
    let (session_id, commit) = project.session_with_a_commit("agent.txt").await;
    let source = project.resolve(&refs::session_ref(session_id)).await;

    let first = push(
        &project.guard,
        &project.paths,
        &source,
        None,
        false,
        None,
        None,
    )
    .await
    .expect("the first push publishes the branch");

    // git answers `=` / `[up to date]` and exits 0; that is a success with the
    // same commit, not a rejection.
    let second = push(
        &project.guard,
        &project.paths,
        &source,
        None,
        false,
        None,
        None,
    )
    .await
    .expect("pushing what is already there is not a failure");

    assert_eq!(first, second);
    assert_eq!(second.commit, commit);
}

#[tokio::test]
async fn only_an_integration_head_or_a_session_ref_may_be_pushed() {
    let project = Project::create().await;
    let (_session, commit) = project.session_with_a_commit("agent.txt").await;
    project.integrate_into_main(&commit).await;

    let upstream_ref = project.resolve("origin/main").await;
    let error = push(
        &project.guard,
        &project.paths,
        &upstream_ref,
        None,
        false,
        None,
        None,
    )
    .await
    .expect_err("an upstream-tracking ref is not a push source");

    assert!(
        matches!(&error, GitError::InvalidRef(named) if named == "origin/main"),
        "expected an invalid ref, got {error:?}"
    );
    assert_eq!(
        project.tmp_entries(),
        Vec::<String>::new(),
        "validation ran a command it should have refused before"
    );
}

#[tokio::test]
async fn a_remote_branch_that_is_not_a_branch_name_is_refused() {
    let project = Project::create().await;
    let (_session, commit) = project.session_with_a_commit("agent.txt").await;
    project.integrate_into_main(&commit).await;
    let source = project.resolve("main").await;
    let upstream_before = project.upstream_ref("refs/heads/main").await;

    // A fully qualified spelling is refused outright: the target namespace is
    // not the caller's to choose (`SPEC.md`, "Git": 400).
    for requested in ["refs/heads/x", "", "-force", "a..b", "with space"] {
        let error = push(
            &project.guard,
            &project.paths,
            &source,
            Some(requested),
            false,
            None,
            None,
        )
        .await
        .expect_err("a name that is not a usable branch name is refused");

        assert!(
            matches!(&error, GitError::InvalidRef(named) if named == requested),
            "{requested:?} produced {error:?}"
        );
    }

    assert_eq!(
        project.upstream_ref("refs/heads/main").await,
        upstream_before,
        "a refused name still reached the remote"
    );
}

#[tokio::test]
async fn a_credential_config_never_outlives_a_push() {
    let project = Project::create().await;
    let (session_id, _commit) = project.session_with_a_commit("agent.txt").await;
    let source = project.resolve(&refs::session_ref(session_id)).await;
    let credential = GitCredential::from_pat(FAKE_PAT).expect("the fake PAT encodes");

    push(
        &project.guard,
        &project.paths,
        &source,
        None,
        false,
        Some(&credential),
        None,
    )
    .await
    .expect("a local upstream ignores the header the credential adds");

    assert!(
        !project
            .tmp_entries()
            .iter()
            .any(|name| name.starts_with(CONFIG_PREFIX)),
        "a credential config was left in DATA_DIR/tmp: {:?}",
        project.tmp_entries()
    );

    // And on the failure path, which is the one a destructor rather than a
    // return value covers.
    project.break_the_remote().await;
    let error = push(
        &project.guard,
        &project.paths,
        &source,
        None,
        false,
        Some(&credential),
        None,
    )
    .await
    .expect_err("an upstream that is not there cannot be reached");

    assert!(
        matches!(error, GitError::Command { .. }),
        "an unreachable remote is an internal fault, got {error:?}"
    );
    assert!(
        !project
            .tmp_entries()
            .iter()
            .any(|name| name.starts_with(CONFIG_PREFIX)),
        "a credential config survived a failed push: {:?}",
        project.tmp_entries()
    );
    // Rule 3: the argv and git's stderr are both safe to log, because the
    // credential only ever existed in the temporary config.
    assert!(
        !format!("{error:?}").contains(FAKE_PAT),
        "the failure quoted the credential"
    );
}
