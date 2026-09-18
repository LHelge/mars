//! The read-only diff and the session-branch listing against real
//! repositories (`ARCHITECTURE.md`, "Git model", Diff; `SPEC.md`, "Git":
//! `Diff`, `SessionBranch`).
//!
//! Git is never mocked (`CLAUDE.md`, "Testing expectations"), and what a real
//! repository shows that the parser unit tests beside `git::diff` cannot is
//! the part that depends on git's own behaviour: that the merge base is the
//! branch point rather than the base branch's tip, that a moving integration
//! head does not leak other people's commits into a session's patch, that
//! `--numstat` really does print `-` for a binary file, and that ahead/behind
//! is counted in the direction the API documents.
//!
//! No database and no `TestApp`: nothing in this file touches Postgres, so it
//! needs no container engine either.

#![cfg(feature = "integration-tests")]

use std::path::{Path, PathBuf};
use std::time::Duration;

use mars_orchestrator::git::testutil::{TestUpstream, run_git};
use mars_orchestrator::git::{
    CommitIdentity, DataPaths, GitError, GitRef, MAX_PATCH_BYTES, ProjectGitGuard, ProjectGitLocks,
    ResolvedRef, create_work_clone, diff, fetch_back, fetch_upstream, init_project_repo, refs,
    resolve_base, session_branches,
};
use mars_orchestrator::models::{Diff, DiffStatus, RemoteUrl};
use tempfile::TempDir;
use uuid::Uuid;

/// The project's default branch throughout, as `TestUpstream` creates it.
const DEFAULT_BRANCH: &str = "main";

/// A tag with no integration head of the same name, so the fallback
/// `refs::resolve` makes for a bare tag cannot quietly stand in for a missing
/// default branch.
const LONELY_TAG: &str = "release-1";

/// Long enough for git's whole-second committer dates to differ.
///
/// `for-each-ref` reports `%(committerdate:iso-strict)`, which has no
/// sub-second field, so two branches committed in the same second would tie
/// and the documented ordering could not be asserted.
const A_DIFFERENT_SECOND: Duration = Duration::from_millis(1_100);

/// The identity a launching user would supply. Obviously fake (rule 3);
/// `.invalid` never resolves.
fn launching_user() -> CommitIdentity {
    CommitIdentity {
        name: "Ada Launcher".to_string(),
        email: "ada@example.invalid".to_string(),
    }
}

/// A `DATA_DIR`, the project git lock and an initialised project repository,
/// with an upstream that can still be advanced.
///
/// The guard is held for the whole test. The two functions under test take
/// none — they are the read-only pair that runs outside the lock
/// (`ARCHITECTURE.md`, "Git model", Serialization) — but launching a session
/// and fetching upstream do.
struct Project {
    _data: TempDir,
    upstream: TestUpstream,
    paths: DataPaths,
    guard: ProjectGitGuard,
}

impl Project {
    /// An upstream with `main`, a file for a session to delete and a tag with
    /// no branch behind it, and a project repository initialised from it.
    async fn create() -> Self {
        let upstream = TestUpstream::create().await;
        upstream
            .commit_file(
                DEFAULT_BRANCH,
                "old.txt",
                "gone soon\n",
                "chore: add a file for a session to delete",
            )
            .await;
        upstream.tag(LONELY_TAG, DEFAULT_BRANCH, false).await;

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

    fn project_id(&self) -> Uuid {
        self.guard.project_id()
    }

    /// The project repository the two functions under test read.
    fn repo(&self) -> PathBuf {
        self.paths.project_repo(self.project_id())
    }

    /// The work directory of one session.
    fn work(&self, session_id: Uuid) -> PathBuf {
        self.paths.session_work(session_id)
    }

    /// Launch a session from the default branch and return its id.
    async fn start_session(&self) -> Uuid {
        let session_id = Uuid::new_v4();
        let base = resolve_base(&self.guard, &self.paths, None, DEFAULT_BRANCH)
            .await
            .expect("the default branch resolves");

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

    /// Publish a session's branch into the project repository, as the service
    /// does before resolving a session `head`.
    async fn sync(&self, session_id: Uuid) -> String {
        fetch_back(&self.guard, &self.paths, session_id)
            .await
            .expect("the session branch is fetched back")
    }

    /// Move `refs/heads/main` on by one upstream commit.
    ///
    /// A fetch only advances `refs/remotes/origin/main`: the integration head
    /// is Mars's own and nothing but an explicit write moves it (ADR 0017).
    async fn advance_default_branch(&self, path: &str, content: &str, message: &str) -> String {
        let commit = self
            .upstream
            .commit_file(DEFAULT_BRANCH, path, content, message)
            .await;

        fetch_upstream(&self.guard, &self.paths, None)
            .await
            .expect("the upstream is fetched");
        refs::update(
            &self.repo(),
            &format!("refs/heads/{DEFAULT_BRANCH}"),
            &commit,
            None,
        )
        .await
        .expect("the integration head is advanced");

        commit
    }

    /// Resolve an API ref name against the project repository.
    async fn resolved(&self, name: &str) -> ResolvedRef {
        let git_ref = GitRef::parse(name).expect("a parsable ref name");
        refs::resolve(&self.repo(), &git_ref)
            .await
            .unwrap_or_else(|err| panic!("{name} resolves: {err}"))
    }

    /// The diff of one session against the default branch, with the
    /// endpoint's own limit.
    async fn session_diff(&self, session_id: Uuid) -> Diff {
        let base = self.resolved(DEFAULT_BRANCH).await;
        let head = self.resolved(&session_id.to_string()).await;

        diff::diff(
            &self.paths,
            self.project_id(),
            &base,
            &head,
            MAX_PATCH_BYTES,
        )
        .await
        .expect("the diff is computed")
    }
}

/// Stage everything in a work clone and commit it, returning the new tip.
async fn commit_all(work: &Path, message: &str) -> String {
    run_git(work, &["add", "--all"]).await;
    run_git(work, &["commit", "--quiet", "-m", message]).await;
    run_git(work, &["rev-parse", "HEAD"])
        .await
        .trim()
        .to_string()
}

/// `(path, status, additions, deletions)` for each file in a diff, which is
/// what the assertions below compare.
fn summary(diff: &Diff) -> Vec<(String, DiffStatus, u32, u32)> {
    diff.files
        .iter()
        .map(|file| {
            (
                file.path.clone(),
                file.status,
                file.additions,
                file.deletions,
            )
        })
        .collect()
}

#[tokio::test]
async fn a_session_diff_lists_every_changed_file_with_its_status_and_counts() {
    let project = Project::create().await;
    let branch_point = project.resolved(DEFAULT_BRANCH).await.commit;
    let session_id = project.start_session().await;
    let work = project.work(session_id);

    std::fs::write(work.join("a.txt"), "one\ntwo\n").expect("the new file is written");
    std::fs::write(work.join("README.md"), "# a changed fixture\n").expect("the file is rewritten");
    std::fs::remove_file(work.join("old.txt")).expect("the old file is removed");
    commit_all(&work, "feat: the agent's work").await;
    let tip = project.sync(session_id).await;

    let diff = project.session_diff(session_id).await;

    assert_eq!(diff.base, DEFAULT_BRANCH);
    assert_eq!(diff.head, session_id.to_string());
    assert_eq!(
        diff.merge_base, branch_point,
        "the diff did not start from the branch point"
    );
    assert_eq!(
        summary(&diff),
        vec![
            ("README.md".to_string(), DiffStatus::Modified, 1, 1),
            ("a.txt".to_string(), DiffStatus::Added, 2, 0),
            ("old.txt".to_string(), DiffStatus::Deleted, 0, 1),
        ]
    );
    assert!(
        diff.patch.contains("diff --git a/a.txt b/a.txt"),
        "the patch does not carry the new file: {}",
        diff.patch
    );
    assert!(!diff.truncated);

    // A guard on the fixture: the diff really is of the published tip.
    assert_eq!(
        run_git(
            &project.repo(),
            &["rev-parse", &format!("refs/sessions/{session_id}")]
        )
        .await
        .trim(),
        tip
    );
}

#[tokio::test]
async fn the_diff_follows_the_merge_base_when_the_default_branch_moves_on() {
    let project = Project::create().await;
    let branch_point = project.resolved(DEFAULT_BRANCH).await.commit;
    let session_id = project.start_session().await;
    let work = project.work(session_id);

    std::fs::write(work.join("a.txt"), "one\ntwo\n").expect("the new file is written");
    commit_all(&work, "feat: the agent's work").await;
    project.sync(session_id).await;

    // Somebody else merged into the integration branch after this session
    // started. Comparing against its tip would report their file as a
    // deletion; comparing from the merge base shows only this session's work.
    let advanced = project
        .advance_default_branch("later.txt", "somebody else\n", "feat: unrelated work")
        .await;
    assert_ne!(advanced, branch_point);

    let diff = project.session_diff(session_id).await;

    assert_eq!(diff.merge_base, branch_point);
    assert_eq!(
        summary(&diff),
        vec![("a.txt".to_string(), DiffStatus::Added, 2, 0)]
    );
    assert!(
        !diff.patch.contains("later.txt"),
        "the other branch's work leaked into the patch: {}",
        diff.patch
    );
}

#[tokio::test]
async fn a_patch_above_the_limit_is_truncated_to_a_prefix_of_the_whole_one() {
    let project = Project::create().await;
    let session_id = project.start_session().await;
    let work = project.work(session_id);

    // Multi-byte text, so the cut has to find a character boundary rather than
    // landing wherever the byte limit happens to fall. 12 bytes per patch line
    // ("+" and five two-byte characters and a newline), comfortably over the
    // 1 MiB limit.
    let line = "αβγδε\n";
    std::fs::write(work.join("big.txt"), line.repeat(200_000)).expect("the big file is written");
    commit_all(&work, "feat: a large file").await;
    project.sync(session_id).await;

    let base = project.resolved(DEFAULT_BRANCH).await;
    let head = project.resolved(&session_id.to_string()).await;

    let truncated = diff::diff(
        &project.paths,
        project.project_id(),
        &base,
        &head,
        MAX_PATCH_BYTES,
    )
    .await
    .expect("the diff is computed");
    let whole = diff::diff(
        &project.paths,
        project.project_id(),
        &base,
        &head,
        usize::MAX,
    )
    .await
    .expect("the unlimited diff is computed");

    assert!(truncated.truncated);
    assert!(
        truncated.patch.len() <= MAX_PATCH_BYTES,
        "the patch is {} bytes",
        truncated.patch.len()
    );
    assert!(!whole.truncated);
    assert!(
        whole.patch.len() > MAX_PATCH_BYTES,
        "the fixture did not produce a patch over the limit"
    );
    assert!(
        whole.patch.starts_with(&truncated.patch),
        "the truncated patch is not a prefix of the whole one"
    );

    // The file list is never truncated: only the patch is.
    assert_eq!(
        summary(&truncated),
        vec![("big.txt".to_string(), DiffStatus::Added, 200_000, 0)]
    );
}

#[tokio::test]
async fn a_binary_file_reports_no_counted_lines() {
    let project = Project::create().await;
    let session_id = project.start_session().await;
    let work = project.work(session_id);

    std::fs::write(work.join("blob.bin"), [0u8, 1, 2, 0, 255, 254, 0, 7])
        .expect("the binary file is written");
    commit_all(&work, "feat: a binary file").await;
    project.sync(session_id).await;

    let diff = project.session_diff(session_id).await;

    assert_eq!(
        summary(&diff),
        vec![("blob.bin".to_string(), DiffStatus::Added, 0, 0)]
    );
}

#[tokio::test]
async fn a_head_that_is_its_own_base_has_nothing_to_show() {
    let project = Project::create().await;
    let base = project.resolved(DEFAULT_BRANCH).await;

    let diff = diff::diff(
        &project.paths,
        project.project_id(),
        &base,
        &base,
        MAX_PATCH_BYTES,
    )
    .await
    .expect("a diff of a commit against itself is still a diff");

    assert_eq!(diff.merge_base, base.commit);
    assert!(diff.files.is_empty(), "{:?}", diff.files);
    assert_eq!(diff.patch, "");
    assert!(!diff.truncated);
}

#[tokio::test]
async fn two_commits_with_no_common_ancestor_have_no_merge_base() {
    let project = Project::create().await;
    let repo = project.repo();
    let base = project.resolved(DEFAULT_BRANCH).await;

    // A root commit reusing the default branch's tree, which is the cheapest
    // way to get an unrelated history into the project repository.
    let tree = run_git(&repo, &["rev-parse", "refs/heads/main^{tree}"])
        .await
        .trim()
        .to_string();
    let orphan = run_git(&repo, &["commit-tree", "-m", "an unrelated root", &tree])
        .await
        .trim()
        .to_string();
    let head = ResolvedRef {
        git_ref: GitRef::Commit(orphan.clone()),
        commit: orphan,
    };

    let error = diff::diff(
        &project.paths,
        project.project_id(),
        &base,
        &head,
        MAX_PATCH_BYTES,
    )
    .await
    .expect_err("unrelated histories cannot be diffed from a merge base");

    let GitError::UnknownRef(ref message) = error else {
        panic!("expected an unknown ref, got {error:?}");
    };
    assert_eq!(message, "no merge base");
    assert_eq!(error.status(), axum::http::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn session_branches_count_ahead_and_behind_against_the_default_branch() {
    let project = Project::create().await;

    let older = project.start_session().await;
    let older_work = project.work(older);
    std::fs::write(older_work.join("one.txt"), "one\n").expect("the file is written");
    commit_all(&older_work, "feat: the first commit").await;
    std::fs::write(older_work.join("two.txt"), "two\n").expect("the file is written");
    let older_tip = commit_all(&older_work, "feat: the second commit").await;
    project.sync(older).await;

    tokio::time::sleep(A_DIFFERENT_SECOND).await;

    let newer = project.start_session().await;
    let newer_work = project.work(newer);
    std::fs::write(newer_work.join("three.txt"), "three\n").expect("the file is written");
    let newer_tip = commit_all(&newer_work, "feat: the only commit").await;
    project.sync(newer).await;

    project
        .advance_default_branch("later.txt", "somebody else\n", "feat: unrelated work")
        .await;

    let branches = session_branches(&project.paths, project.project_id(), DEFAULT_BRANCH)
        .await
        .expect("the session branches are listed");

    assert_eq!(branches.len(), 2, "{branches:?}");

    // Newest tip first (`SPEC.md`, "Git").
    assert_eq!(branches[0].session_id, newer);
    assert_eq!(branches[0].git_ref, format!("refs/sessions/{newer}"));
    assert_eq!(branches[0].commit, newer_tip);
    assert_eq!((branches[0].ahead, branches[0].behind), (1, 1));
    assert_eq!(branches[0].base, DEFAULT_BRANCH);

    assert_eq!(branches[1].session_id, older);
    assert_eq!(branches[1].git_ref, format!("refs/sessions/{older}"));
    assert_eq!(branches[1].commit, older_tip);
    assert_eq!((branches[1].ahead, branches[1].behind), (2, 1));
    assert_eq!(branches[1].base, DEFAULT_BRANCH);

    assert!(branches[0].updated_at >= branches[1].updated_at);
}

#[tokio::test]
async fn a_session_that_has_never_synced_has_no_branch_to_list() {
    let project = Project::create().await;
    let session_id = project.start_session().await;
    let work = project.work(session_id);

    std::fs::write(work.join("a.txt"), "one\n").expect("the file is written");
    commit_all(&work, "feat: unpublished work").await;

    // Fetch-back is what puts `refs/sessions/<id>` in the project repository,
    // and the listing reads nothing else.
    let branches = session_branches(&project.paths, project.project_id(), DEFAULT_BRANCH)
        .await
        .expect("an empty listing is still a listing");

    assert!(branches.is_empty(), "{branches:?}");
}

#[tokio::test]
async fn session_branches_without_its_integration_head_is_an_unknown_ref() {
    let project = Project::create().await;
    let session_id = project.start_session().await;
    project.sync(session_id).await;

    for default_branch in ["not-a-branch-here", LONELY_TAG] {
        let error = session_branches(&project.paths, project.project_id(), default_branch)
            .await
            .expect_err("ahead/behind needs the integration head it is measured against");

        let GitError::UnknownRef(ref name) = error else {
            panic!("expected an unknown ref for {default_branch}, got {error:?}");
        };
        assert_eq!(name, default_branch);
        assert_eq!(error.status(), axum::http::StatusCode::BAD_REQUEST);
    }
}
