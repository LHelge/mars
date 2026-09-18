//! Rebasing a branch onto another against real repositories
//! (`ARCHITECTURE.md`, "Git model", Merge, rebase, push and Commit identity;
//! `SPEC.md`, "Git" and the MCP `rebase` tool; ADR 0007, ADR 0019).
//!
//! Git is never mocked (`CLAUDE.md`, "Testing expectations"), so every case
//! here builds an upstream, a project repository and real session work clones
//! in `tempfile` directories and rebases what an agent actually committed.
//! What only a real repository shows is what this asserts: that the rewritten
//! ref carries new commits whose authors survived and whose committer is the
//! bot, that the session's checkout follows only when a `reset --hard` would
//! discard nothing, that a conflict leaves both the ref and the checkout
//! exactly where they were, and that no temporary clone or `refs/tmp/*`
//! outlives the operation.
//!
//! No database and no `TestApp`: nothing in this file touches Postgres, so it
//! needs no container engine either.

#![cfg(feature = "integration-tests")]

use std::path::{Path, PathBuf};

use mars_orchestrator::git::testutil::{
    TEST_AUTHOR_EMAIL, TEST_AUTHOR_NAME, TestUpstream, run_git,
};
use mars_orchestrator::git::{
    CommitIdentity, DataPaths, GitActor, GitError, GitRef, ProjectGitGuard, ProjectGitLocks,
    RebaseOutcome, ResolvedRef, WorkTreeOutcome, create_work_clone, fetch_back, fetch_upstream,
    init_project_repo, merge, rebase, refs, resolve_base, session_branch,
};
use mars_orchestrator::models::RemoteUrl;
use tempfile::TempDir;
use uuid::Uuid;

/// The identity a launching user would supply. Obviously fake (rule 3);
/// `.invalid` never resolves.
fn launching_user() -> CommitIdentity {
    CommitIdentity {
        name: "Ada Launcher".to_string(),
        email: "ada@example.invalid".to_string(),
    }
}

/// The bot identity `GitCredentialProvider::commit_identity` would answer, and
/// the one every replayed commit here must be *committed* by while keeping its
/// author.
fn bot() -> CommitIdentity {
    CommitIdentity {
        name: "Mars Bot".to_string(),
        email: "mars-bot@example.invalid".to_string(),
    }
}

/// A `DATA_DIR`, one project git lock held for the whole test and an
/// initialised project repository — the state a rebase request finds
/// (`ARCHITECTURE.md`, "Git model", Serialization).
struct Project {
    _data: TempDir,
    upstream: TestUpstream,
    paths: DataPaths,
    guard: ProjectGitGuard,
}

impl Project {
    /// An upstream with `main`, and a project repository initialised from it.
    async fn create() -> Self {
        let upstream = TestUpstream::create().await;
        let data = tempfile::tempdir().expect("a temporary data directory");
        let paths = DataPaths::new(data.path());
        let guard = ProjectGitLocks::new().lock(Uuid::new_v4()).await;
        let remote = RemoteUrl::local_for_tests(&upstream.path);

        init_project_repo(&guard, &paths, &remote, None, None)
            .await
            .expect("the project repository is initialised");

        Self {
            _data: data,
            upstream,
            paths,
            guard,
        }
    }

    /// The project repository every operation here acts on.
    fn repo(&self) -> PathBuf {
        self.paths.project_repo(self.guard.project_id())
    }

    /// The session's checkout, which only the reconciliation step touches.
    fn work(&self, session_id: Uuid) -> PathBuf {
        self.paths.session_work(session_id)
    }

    /// Launch a session from the project default and return its id.
    async fn launch(&self) -> Uuid {
        let session_id = Uuid::new_v4();
        let base = resolve_base(&self.guard, &self.paths, None, "main")
            .await
            .expect("the base resolves");

        create_work_clone(
            &self.guard,
            &self.paths,
            session_id,
            &base,
            &launching_user(),
        )
        .await
        .expect("the work clone is created");

        session_id
    }

    /// Commit `content` to `path` in the session's checkout, as the agent
    /// would inside its container.
    async fn commit(&self, session_id: Uuid, path: &str, content: &str, message: &str) -> String {
        let work = self.work(session_id);
        let file = work.join(path);
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent).expect("the parent directory is created");
        }
        std::fs::write(&file, content).expect("the file is written");

        run_git(&work, &["add", "--", path]).await;
        run_git(&work, &["commit", "--quiet", "-m", message]).await;
        run_git(&work, &["rev-parse", "HEAD"])
            .await
            .trim()
            .to_string()
    }

    /// Fetch the session branch back into the project repository, as the
    /// service does before every rebase (`SPEC.md`, "Git").
    async fn sync(&self, session_id: Uuid) -> String {
        fetch_back(&self.guard, &self.paths, session_id)
            .await
            .expect("the session branch is fetched back")
    }

    /// Advance upstream's `main` and fetch it, which moves `origin/main` and
    /// leaves the integration head alone (ADR 0017).
    async fn advance_upstream(&self, path: &str, content: &str, message: &str) -> String {
        let commit = self
            .upstream
            .commit_file("main", path, content, message)
            .await;
        fetch_upstream(&self.guard, &self.paths, None)
            .await
            .expect("the upstream fetch succeeds");
        commit
    }

    /// Resolve one ref in the project repository, as a route does under the
    /// lock before calling the primitive.
    async fn resolve(&self, name: &str) -> ResolvedRef {
        let git_ref = GitRef::parse(name).expect("a parsable ref name");
        refs::resolve(&self.repo(), &git_ref)
            .await
            .unwrap_or_else(|err| panic!("{name} resolves: {err}"))
    }

    /// Rebase `branch` onto `onto` by API name, with the bot identity.
    async fn rebase(
        &self,
        branch: &str,
        onto: &str,
    ) -> std::result::Result<RebaseOutcome, GitError> {
        let branch = self.resolve(branch).await;
        let onto = self.resolve(onto).await;

        rebase(&self.guard, &self.paths, &branch, &onto, &bot()).await
    }

    /// The commit an API-named ref points at in the project repository.
    async fn commit_of(&self, name: &str) -> String {
        self.resolve(name).await.commit
    }

    /// Assert the operation left no temporary clone and wrote no `refs/tmp/*`
    /// into the project repository.
    async fn assert_no_leftovers(&self) {
        let entries: Vec<PathBuf> = match std::fs::read_dir(self.paths.tmp()) {
            Ok(entries) => entries
                .map(|entry| entry.expect("a readable directory entry").path())
                .collect(),
            Err(_) => Vec::new(),
        };

        assert_eq!(
            entries,
            Vec::<PathBuf>::new(),
            "a temporary clone was left in DATA_DIR/tmp"
        );
        assert_eq!(
            run_git(
                &self.repo(),
                &["for-each-ref", "--format=%(refname)", "refs/tmp/"],
            )
            .await
            .trim(),
            "",
            "the temporary refs reached the project repository"
        );
    }
}

/// One commit's author, committer and subject.
async fn commit_details(repo: &Path, commit: &str) -> (String, String, String, String, String) {
    let shown = run_git(
        repo,
        &["show", "--quiet", "--format=%an%n%ae%n%cn%n%ce%n%s", commit],
    )
    .await;

    let mut lines = shown.lines();
    let author_name = lines.next().unwrap_or_default().to_string();
    let author_email = lines.next().unwrap_or_default().to_string();
    let committer_name = lines.next().unwrap_or_default().to_string();
    let committer_email = lines.next().unwrap_or_default().to_string();
    let subject = lines.next().unwrap_or_default().to_string();

    (
        author_name,
        author_email,
        committer_name,
        committer_email,
        subject,
    )
}

/// The commits in `tip` that are not in `base`, oldest first.
async fn commits_between(repo: &Path, base: &str, tip: &str) -> Vec<String> {
    run_git(repo, &["rev-list", "--reverse", &format!("{base}..{tip}")])
        .await
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

/// Is `ancestor` reachable from `descendant`? `merge-base` answers with the
/// ancestor itself when it is one, which needs no non-zero exit code.
async fn is_ancestor(repo: &Path, ancestor: &str, descendant: &str) -> bool {
    run_git(repo, &["merge-base", ancestor, descendant])
        .await
        .trim()
        == ancestor
}

/// The commit the session's checkout has checked out.
async fn work_head(work: &Path) -> String {
    run_git(work, &["rev-parse", "HEAD"])
        .await
        .trim()
        .to_string()
}

#[tokio::test]
async fn a_session_branch_is_replayed_onto_fetched_upstream_and_a_clean_checkout_follows() {
    let project = Project::create().await;
    let session_id = project.launch().await;

    project
        .commit(session_id, "first.txt", "first\n", "feat: the first commit")
        .await;
    project
        .commit(
            session_id,
            "second.txt",
            "second\n",
            "feat: the second commit",
        )
        .await;
    let before = project.sync(session_id).await;

    let upstream_commit = project
        .advance_upstream("upstream.txt", "upstream\n", "feat: upstream moved")
        .await;

    let outcome = project
        .rebase(&session_id.to_string(), "origin/main")
        .await
        .expect("the session branch rebases onto the fetched upstream");

    assert_eq!(outcome.work_tree, WorkTreeOutcome::Updated);
    assert_ne!(outcome.commit, before, "nothing was rewritten");

    let repo = project.repo();
    assert_eq!(
        project
            .commit_of(&format!("refs/sessions/{session_id}"))
            .await,
        outcome.commit,
        "the session ref was not written back"
    );
    assert!(
        is_ancestor(&repo, &upstream_commit, &outcome.commit).await,
        "the upstream commit is not in the rebased history"
    );

    // Both commits survived, in order, with their authors — and Mars as the
    // party that re-committed them (Commit identity).
    let replayed = commits_between(&repo, &upstream_commit, &outcome.commit).await;
    assert_eq!(replayed.len(), 2, "{replayed:?}");
    let bot = bot();
    for (commit, expected_subject) in replayed
        .iter()
        .zip(["feat: the first commit", "feat: the second commit"])
    {
        let (author_name, author_email, committer_name, committer_email, subject) =
            commit_details(&repo, commit).await;

        assert_eq!(subject, expected_subject);
        assert_eq!(author_name, TEST_AUTHOR_NAME);
        assert_eq!(author_email, TEST_AUTHOR_EMAIL);
        assert_eq!(committer_name, bot.name);
        assert_eq!(committer_email, bot.email);
    }

    // The checkout was clean and on the session branch, so it followed.
    let work = project.work(session_id);
    assert_eq!(work_head(&work).await, outcome.commit);
    assert_eq!(
        run_git(&work, &["symbolic-ref", "HEAD"]).await.trim(),
        format!("refs/heads/{}", session_branch(session_id)),
        "reconciliation moved the checkout off the session branch"
    );
    assert!(
        work.join("upstream.txt").exists(),
        "the reset did not apply"
    );

    project.assert_no_leftovers().await;
}

#[tokio::test]
async fn a_dirty_checkout_is_reported_rather_than_reset() {
    let project = Project::create().await;
    let session_id = project.launch().await;

    project
        .commit(session_id, "agent.txt", "work\n", "feat: the agent's work")
        .await;
    let before = project.sync(session_id).await;
    project
        .advance_upstream("upstream.txt", "upstream\n", "feat: upstream moved")
        .await;

    // Uncommitted work in a tracked file: exactly what `reset --hard` would
    // throw away.
    let work = project.work(session_id);
    std::fs::write(work.join("agent.txt"), "work in progress\n")
        .expect("the uncommitted change is written");

    let outcome = project
        .rebase(&session_id.to_string(), "origin/main")
        .await
        .expect("a dirty checkout does not fail the rebase");

    assert_eq!(outcome.work_tree, WorkTreeOutcome::ReconciliationRequired);

    // The mirror write-back stands; only the checkout is behind.
    assert_eq!(
        project
            .commit_of(&format!("refs/sessions/{session_id}"))
            .await,
        outcome.commit
    );
    assert_ne!(outcome.commit, before);
    assert_eq!(
        work_head(&work).await,
        before,
        "the checkout was moved despite being dirty"
    );
    assert_eq!(
        std::fs::read_to_string(work.join("agent.txt")).expect("the file is readable"),
        "work in progress\n",
        "uncommitted work was discarded"
    );

    project.assert_no_leftovers().await;
}

#[tokio::test]
async fn an_untracked_file_does_not_stop_the_checkout_following() {
    let project = Project::create().await;
    let session_id = project.launch().await;

    project
        .commit(session_id, "agent.txt", "work\n", "feat: the agent's work")
        .await;
    project.sync(session_id).await;
    project
        .advance_upstream("upstream.txt", "upstream\n", "feat: upstream moved")
        .await;

    // An untracked build artefact survives `reset --hard` untouched, so it is
    // not work the reset would discard and must not block reconciliation.
    let work = project.work(session_id);
    std::fs::write(work.join("scratch.log"), "noise\n").expect("the artefact is written");

    let outcome = project
        .rebase(&session_id.to_string(), "origin/main")
        .await
        .expect("an untracked file is not a dirty work tree");

    assert_eq!(outcome.work_tree, WorkTreeOutcome::Updated);
    assert_eq!(work_head(&work).await, outcome.commit);
    assert!(
        work.join("scratch.log").exists(),
        "the reset removed an untracked file"
    );
}

#[tokio::test]
async fn a_checkout_on_another_branch_is_reported_rather_than_reset() {
    let project = Project::create().await;
    let session_id = project.launch().await;

    project
        .commit(session_id, "agent.txt", "work\n", "feat: the agent's work")
        .await;
    let before = project.sync(session_id).await;
    project
        .advance_upstream("upstream.txt", "upstream\n", "feat: upstream moved")
        .await;

    // The agent checked something else out; a reset here would move a branch
    // nobody asked about (or none at all).
    let work = project.work(session_id);
    run_git(&work, &["checkout", "--quiet", "--detach"]).await;

    let outcome = project
        .rebase(&session_id.to_string(), "origin/main")
        .await
        .expect("a detached checkout does not fail the rebase");

    assert_eq!(outcome.work_tree, WorkTreeOutcome::ReconciliationRequired);
    assert_eq!(work_head(&work).await, before, "the checkout was moved");
    assert_eq!(
        project
            .commit_of(&format!("refs/sessions/{session_id}"))
            .await,
        outcome.commit
    );
}

#[tokio::test]
async fn a_session_with_no_checkout_left_reports_nothing_to_reconcile() {
    let project = Project::create().await;
    let session_id = project.launch().await;

    project
        .commit(session_id, "agent.txt", "work\n", "feat: the agent's work")
        .await;
    project.sync(session_id).await;
    project
        .advance_upstream("upstream.txt", "upstream\n", "feat: upstream moved")
        .await;

    // The session was deleted, or its container never came back; the ref in
    // the mirror is still rebasable.
    std::fs::remove_dir_all(project.work(session_id)).expect("the work directory is removed");

    let outcome = project
        .rebase(&session_id.to_string(), "origin/main")
        .await
        .expect("a missing checkout does not fail the rebase");

    assert_eq!(outcome.work_tree, WorkTreeOutcome::NotApplicable);
    assert_eq!(
        project
            .commit_of(&format!("refs/sessions/{session_id}"))
            .await,
        outcome.commit
    );
    project.assert_no_leftovers().await;
}

#[tokio::test]
async fn an_integration_head_is_linearised_with_no_checkout_to_update() {
    let project = Project::create().await;
    let session_id = project.launch().await;

    project
        .commit(session_id, "agent.txt", "work\n", "feat: the agent's work")
        .await;
    let session_commit = project.sync(session_id).await;

    // The documented explicit way to linearise before a push: `main` carries
    // work of its own and upstream has moved underneath it.
    let source = project.resolve(&session_id.to_string()).await;
    let target = project.resolve("main").await;
    merge(
        &project.guard,
        &project.paths,
        &source,
        &target,
        None,
        &bot(),
        &GitActor::System,
    )
    .await
    .expect("the session merges into main");
    assert_eq!(project.commit_of("main").await, session_commit);

    let upstream_commit = project
        .advance_upstream("upstream.txt", "upstream\n", "feat: upstream moved")
        .await;

    let outcome = project
        .rebase("main", "origin/main")
        .await
        .expect("an integration head is rebasable");

    assert_eq!(outcome.work_tree, WorkTreeOutcome::NotApplicable);
    assert_eq!(project.commit_of("main").await, outcome.commit);
    assert!(is_ancestor(&project.repo(), &upstream_commit, &outcome.commit).await);
    assert_eq!(
        commits_between(&project.repo(), &upstream_commit, &outcome.commit)
            .await
            .len(),
        1,
        "the linearised head should carry exactly the one commit it had"
    );

    // The session's own ref is not a rebase target here and must not move.
    assert_eq!(
        project
            .commit_of(&format!("refs/sessions/{session_id}"))
            .await,
        session_commit
    );
    project.assert_no_leftovers().await;
}

#[tokio::test]
async fn a_conflict_reports_its_paths_and_leaves_the_ref_and_the_checkout_alone() {
    let project = Project::create().await;
    let session_id = project.launch().await;

    project
        .commit(
            session_id,
            "README.md",
            "# the session's line\n",
            "docs: the session's readme",
        )
        .await;
    let before = project.sync(session_id).await;

    project
        .advance_upstream(
            "README.md",
            "# upstream's line\n",
            "docs: upstream's readme",
        )
        .await;

    let error = project
        .rebase(&session_id.to_string(), "origin/main")
        .await
        .expect_err("the replayed commit conflicts");

    let GitError::Conflict { ref paths } = error else {
        panic!("expected a conflict, got {error:?}");
    };
    assert_eq!(paths, &vec!["README.md".to_string()]);
    assert_eq!(error.status(), axum::http::StatusCode::UNPROCESSABLE_ENTITY);

    assert_eq!(
        project
            .commit_of(&format!("refs/sessions/{session_id}"))
            .await,
        before,
        "a conflicting rebase rewrote the session ref"
    );

    // The checkout is never touched on this path: no fetch, no reset, and
    // nothing half-rebased in it.
    let work = project.work(session_id);
    assert_eq!(work_head(&work).await, before);
    assert_eq!(
        run_git(&work, &["status", "--porcelain"]).await.trim(),
        "",
        "the conflicting rebase disturbed the checkout"
    );

    project.assert_no_leftovers().await;
}

#[tokio::test]
async fn rebasing_something_already_based_on_onto_changes_nothing() {
    let project = Project::create().await;
    let session_id = project.launch().await;

    project
        .commit(session_id, "agent.txt", "work\n", "feat: the agent's work")
        .await;
    let before = project.sync(session_id).await;

    // `main` is where this session branched from and has not moved.
    let outcome = project
        .rebase(&session_id.to_string(), "main")
        .await
        .expect("an already-based branch rebases to itself");

    assert_eq!(
        outcome.commit, before,
        "an up-to-date rebase rewrote history"
    );
    assert_eq!(outcome.work_tree, WorkTreeOutcome::Updated);
    assert_eq!(
        project
            .commit_of(&format!("refs/sessions/{session_id}"))
            .await,
        before
    );
    assert_eq!(work_head(&project.work(session_id)).await, before);
    project.assert_no_leftovers().await;
}

#[tokio::test]
async fn a_branch_with_nothing_of_its_own_moves_to_onto() {
    let project = Project::create().await;
    let session_id = project.launch().await;
    let base = project.sync(session_id).await;

    let upstream_commit = project
        .advance_upstream("upstream.txt", "upstream\n", "feat: upstream moved")
        .await;
    assert_ne!(base, upstream_commit);

    // Git semantics: with nothing to replay, `work` fast-forwards to `onto`
    // and that is what is written back.
    let outcome = project
        .rebase(&session_id.to_string(), "origin/main")
        .await
        .expect("a branch that is only behind rebases");

    assert_eq!(outcome.commit, upstream_commit);
    assert_eq!(outcome.work_tree, WorkTreeOutcome::Updated);
    assert_eq!(
        project
            .commit_of(&format!("refs/sessions/{session_id}"))
            .await,
        upstream_commit
    );
    assert_eq!(work_head(&project.work(session_id)).await, upstream_commit);
}

#[tokio::test]
async fn an_upstream_ref_may_be_onto_and_never_the_branch_being_rewritten() {
    let project = Project::create().await;
    let session_id = project.launch().await;
    project
        .commit(session_id, "agent.txt", "work\n", "feat: the agent's work")
        .await;
    let session_commit = project.sync(session_id).await;
    let before = project.commit_of("origin/main").await;

    // An upstream-tracking ref is upstream's, so it is never rewritten
    // (ADR 0017; `SPEC.md`, "Git": 400).
    let error = project
        .rebase("origin/main", "main")
        .await
        .expect_err("an upstream ref may not be rebased");
    assert!(matches!(error, GitError::InvalidRef(_)), "{error:?}");
    assert_eq!(error.status(), axum::http::StatusCode::BAD_REQUEST);

    // A session ref is not a base, so it cannot be `onto` either.
    let error = project
        .rebase("main", &session_id.to_string())
        .await
        .expect_err("a session ref is not a rebase base");
    assert!(matches!(error, GitError::InvalidRef(_)), "{error:?}");
    assert_eq!(error.status(), axum::http::StatusCode::BAD_REQUEST);

    assert_eq!(project.commit_of("origin/main").await, before);
    assert_eq!(
        project
            .commit_of(&format!("refs/sessions/{session_id}"))
            .await,
        session_commit
    );
    project.assert_no_leftovers().await;
}
