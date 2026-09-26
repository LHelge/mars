//! Base resolution, the session work clone and fetch-back against real
//! repositories (`ARCHITECTURE.md`, "Git model", Session clone and Fetch-back;
//! ADR 0001, ADR 0007, ADR 0017).
//!
//! Git is never mocked (`CLAUDE.md`, "Testing expectations"), so every case
//! here builds a project repository from a `TestUpstream` in a `tempfile`
//! directory and then does to it exactly what a launch does. What this asserts
//! and the unit tests beside the module cannot is the part that only a real
//! clone shows: that each supported base kind ends up checked out at the
//! commit it resolved to, that the clone borrows the mirror's objects through
//! its alternates file rather than copying them, that it carries no credential
//! and that fetch-back follows a rewritten session branch without touching the
//! project repository when there is nothing to fetch.
//!
//! No database and no `TestApp`: nothing in this file touches Postgres, so it
//! needs no container engine either.

#![cfg(feature = "integration-tests")]

use std::path::{Path, PathBuf};

use mars_orchestrator::git::testutil::{
    TEST_AUTHOR_EMAIL, TEST_AUTHOR_NAME, TestUpstream, run_git,
};
use mars_orchestrator::git::{
    CommitIdentity, DataPaths, FetchedBack, GitCommand, GitError, GitRef, ProjectGitGuard,
    ProjectGitLocks, ResolvedRef, create_work_clone, fetch_back, fetch_back_ended,
    init_project_repo, refs, remove_work_clone, resolve_base, session_branch,
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

/// A `DATA_DIR`, the project git lock and an initialised project repository.
///
/// The guard is held for the whole test, which is what a fresh launch does:
/// base resolution and clone setup run under one acquisition
/// (`ARCHITECTURE.md`, "Git model", Serialization).
struct Project {
    /// The directories `DATA_DIR` lives in: one, or two when it is reached
    /// through a symlink.
    _data: Vec<TempDir>,
    _upstream: TestUpstream,
    paths: DataPaths,
    guard: ProjectGitGuard,
}

impl Project {
    /// An upstream with `main`, a `feature/x` branch and an annotated tag, and
    /// a project repository initialised from it.
    async fn create() -> Self {
        let data = tempfile::tempdir().expect("a temporary data directory");
        let data_dir = data.path().to_path_buf();
        Self::create_in(vec![data], &data_dir).await
    }

    /// The same project with `DATA_DIR` spelled through a symlink, as macOS
    /// spells every temporary directory (`/var` is a link to `/private/var`).
    async fn create_behind_a_symlink() -> Self {
        let target = tempfile::tempdir().expect("the directory the link names");
        let holder = tempfile::tempdir().expect("the directory holding the link");
        let data_dir = holder.path().join("data");
        std::os::unix::fs::symlink(target.path(), &data_dir).expect("the symlink is created");
        Self::create_in(vec![holder, target], &data_dir).await
    }

    async fn create_in(data: Vec<TempDir>, data_dir: &Path) -> Self {
        let upstream = TestUpstream::create().await;
        upstream
            .commit_file("feature/x", "x.txt", "x\n", "feat: start x")
            .await;
        upstream.tag("v1.0.0", "main", true).await;

        let paths = DataPaths::new(data_dir);
        let guard = ProjectGitLocks::new().lock(Uuid::new_v4()).await;
        let remote = RemoteUrl::local_for_tests(&upstream.path);

        init_project_repo(&guard, &paths, &remote, None, None)
            .await
            .expect("the project repository is initialised");

        Self {
            _data: data,
            _upstream: upstream,
            paths,
            guard,
        }
    }

    /// The project repository this project's sessions clone from.
    fn repo(&self) -> PathBuf {
        self.paths.project_repo(self.guard.project_id())
    }

    /// Resolve `base_ref` and clone a fresh session's work directory from it.
    async fn launch(&self, session_id: Uuid, base_ref: Option<&str>) -> ResolvedRef {
        let base = resolve_base(&self.guard, &self.paths, base_ref, "main")
            .await
            .unwrap_or_else(|err| panic!("{base_ref:?} resolves: {err}"));

        create_work_clone(
            &self.guard,
            &self.paths,
            session_id,
            &base,
            &launching_user(),
        )
        .await
        .unwrap_or_else(|err| panic!("the work clone for {base_ref:?} is created: {err}"));

        base
    }

    /// The work directory of one session.
    fn work(&self, session_id: Uuid) -> PathBuf {
        self.paths.session_work(session_id)
    }
}

/// One configured value, or `None` when the key is unset.
///
/// `git config --get` exits 1 for an unset key, which is an answer rather than
/// a failure, so this goes through [`GitCommand::run`].
async fn config_value(repo: &Path, key: &str) -> Option<String> {
    let output = GitCommand::new()
        .args(["config", "--get", "--end-of-options", key])
        .cwd(repo)
        .run()
        .await
        .expect("git config runs");

    (output.status == 0).then(|| output.stdout.trim().to_string())
}

/// The commit a ref points at in `repo`, by its API name.
async fn commit_of(repo: &Path, name: &str) -> String {
    let git_ref = GitRef::parse(name).expect("a parsable ref name");
    refs::resolve(repo, &git_ref)
        .await
        .unwrap_or_else(|err| panic!("{name} resolves in {}: {err}", repo.display()))
        .commit
}

/// Assert everything a freshly created work clone owes its session, whatever
/// base it started from.
async fn assert_clone_is_well_formed(project: &Project, session_id: Uuid, base: &ResolvedRef) {
    let work = project.work(session_id);
    let repo = project.repo();

    assert_eq!(
        run_git(&work, &["rev-parse", "HEAD"]).await.trim(),
        base.commit,
        "the clone is not checked out at the resolved commit"
    );
    assert_eq!(
        run_git(&work, &["symbolic-ref", "--short", "HEAD"])
            .await
            .trim(),
        session_branch(session_id),
        "the clone is not on the session branch"
    );

    // The work tree was actually populated: `--no-checkout` plus an explicit
    // checkout, not a clone that never wrote a file.
    assert!(work.join("README.md").is_file(), "the work tree is empty");

    // ADR 0001: the objects are borrowed from the mirror at the orchestrator's
    // own path, which is what has to resolve inside the container too. One
    // line, spelled as `DATA_DIR` spells it, never with its symlinks resolved
    // (ADR 0054).
    let alternates = std::fs::read_to_string(work.join(".git/objects/info/alternates"))
        .expect("the clone has an alternates file");
    assert_eq!(alternates, format!("{}\n", repo.join("objects").display()));

    // The other half of that: a clone that copied the objects would still have
    // the alternates file, and the disk cost ADR 0001 rules out would only show
    // up on a real project. A fresh clone owns nothing yet.
    assert_eq!(
        own_object_files(&work.join(".git/objects")),
        0,
        "the clone copied objects instead of borrowing them"
    );

    // ADR 0007: no session container ever holds a credential, so the clone it
    // is given must not carry one either.
    assert_eq!(config_value(&work, "http.extraHeader").await, None);
    assert_eq!(
        config_value(&work, "remote.origin.url").await,
        Some(repo.display().to_string()),
        "the clone's origin is the project repository"
    );

    // The launching user's identity, so the agent's commits are attributed
    // (`ARCHITECTURE.md`, "Git model", Commit identity).
    let identity = launching_user();
    assert_eq!(config_value(&work, "user.name").await, Some(identity.name));
    assert_eq!(
        config_value(&work, "user.email").await,
        Some(identity.email)
    );
}

/// How many object files a repository holds itself, ignoring the `info/`
/// bookkeeping the alternates file lives in.
fn own_object_files(objects: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(objects) else {
        return 0;
    };

    entries
        .map(|entry| entry.expect("a readable directory entry").path())
        .filter(|path| path.file_name().is_some_and(|name| name != "info"))
        .map(|path| {
            if path.is_dir() {
                own_object_files(&path)
            } else {
                1
            }
        })
        .sum()
}

/// Commit `content` on the session branch inside a work clone and return the
/// new tip.
async fn commit_in_work(work: &Path, path: &str, content: &str, message: &str) -> String {
    std::fs::write(work.join(path), content).expect("the file is written");
    run_git(work, &["add", "--", path]).await;
    run_git(work, &["commit", "--quiet", "-m", message]).await;
    run_git(work, &["rev-parse", "HEAD"])
        .await
        .trim()
        .to_string()
}

#[tokio::test]
async fn an_omitted_base_starts_from_the_project_default_branch() {
    let project = Project::create().await;
    let session_id = Uuid::new_v4();

    let base = project.launch(session_id, None).await;

    assert_eq!(base.git_ref, GitRef::Head("main".to_string()));
    assert_eq!(base.commit, commit_of(&project.repo(), "main").await);
    assert_clone_is_well_formed(&project, session_id, &base).await;
}

#[tokio::test]
async fn a_data_dir_behind_a_symlink_is_recorded_as_spelled_not_resolved() {
    let project = Project::create_behind_a_symlink().await;
    let session_id = Uuid::new_v4();

    let base = project.launch(session_id, None).await;

    // The mount inside the container is at `DATA_DIR`'s own spelling, so a
    // resolved path in the alternates file would name nothing there.
    assert_ne!(
        std::fs::canonicalize(project.repo()).expect("the repository exists"),
        project.repo(),
        "the fixture does not put a symlink in the path"
    );
    assert_clone_is_well_formed(&project, session_id, &base).await;
}

#[tokio::test]
async fn every_supported_base_kind_resolves_and_produces_a_clone_at_that_commit() {
    let project = Project::create().await;
    let repo = project.repo();

    // A commit that exists only under `refs/sessions/*`: it proves both that
    // a session ref is a usable base and that a bare commit id reaches an
    // object no local ref of the clone will point at.
    let source_session = Uuid::new_v4();
    let source_base = project.launch(source_session, None).await;
    let session_only = commit_in_work(
        &project.work(source_session),
        "agent.txt",
        "written by the agent\n",
        "feat: the agent's work",
    )
    .await;
    assert_ne!(session_only, source_base.commit);
    assert_eq!(
        fetch_back(&project.guard, &project.paths, source_session)
            .await
            .expect("the source session is fetched back"),
        session_only
    );

    let head = commit_of(&repo, "main").await;
    let upstream = commit_of(&repo, "origin/feature/x").await;
    let tag = commit_of(&repo, "refs/tags/v1.0.0").await;

    let cases: Vec<(&str, String, String, GitRef)> = vec![
        (
            "an integration head",
            "main".into(),
            head.clone(),
            GitRef::Head("main".into()),
        ),
        (
            "a fully qualified integration head",
            "refs/heads/feature/x".into(),
            upstream.clone(),
            GitRef::Head("feature/x".into()),
        ),
        (
            "an upstream-tracking ref",
            "origin/feature/x".into(),
            upstream.clone(),
            GitRef::Upstream("feature/x".into()),
        ),
        (
            "a bare tag name",
            "v1.0.0".into(),
            tag.clone(),
            GitRef::Tag("v1.0.0".into()),
        ),
        (
            "a fully qualified tag",
            "refs/tags/v1.0.0".into(),
            tag.clone(),
            GitRef::Tag("v1.0.0".into()),
        ),
        (
            "a session id",
            source_session.to_string(),
            session_only.clone(),
            GitRef::Session(source_session),
        ),
        (
            "a fully qualified session ref",
            format!("refs/sessions/{source_session}"),
            session_only.clone(),
            GitRef::Session(source_session),
        ),
        (
            "a commit id",
            session_only.clone(),
            session_only.clone(),
            GitRef::Commit(session_only.clone()),
        ),
    ];

    for (what, base_ref, expected_commit, expected_ref) in cases {
        let session_id = Uuid::new_v4();
        let base = project.launch(session_id, Some(&base_ref)).await;

        assert_eq!(base.git_ref, expected_ref, "{what} resolved to another ref");
        assert_eq!(
            base.commit, expected_commit,
            "{what} resolved to another commit"
        );
        assert_clone_is_well_formed(&project, session_id, &base).await;
    }
}

#[tokio::test]
async fn a_fully_qualified_hand_off_ref_is_a_usable_base() {
    let project = Project::create().await;
    let repo = project.repo();

    // What the code hand-offs epic retains (ADR 0018). It is reachable here
    // only fully qualified: a bare UUID is a session ref.
    let handoff_id = Uuid::new_v4();
    let commit = commit_of(&repo, "main").await;
    refs::retain_handoff(&repo, handoff_id, &commit)
        .await
        .expect("the hand-off commit is retained");

    let session_id = Uuid::new_v4();
    let base = project
        .launch(session_id, Some(&format!("refs/handoffs/{handoff_id}")))
        .await;

    assert_eq!(base.git_ref, GitRef::Handoff(handoff_id));
    assert_eq!(base.commit, commit);
    assert_clone_is_well_formed(&project, session_id, &base).await;
}

#[tokio::test]
async fn a_base_that_is_not_in_the_project_repository_is_an_unknown_ref() {
    let project = Project::create().await;

    for base_ref in [
        "no-such-branch",
        "origin/no-such-branch",
        "refs/tags/v9.9.9",
        &Uuid::new_v4().to_string(),
        &"0".repeat(40),
    ] {
        let error = resolve_base(&project.guard, &project.paths, Some(base_ref), "main")
            .await
            .expect_err("a base that is not there does not resolve");

        assert!(
            matches!(error, GitError::UnknownRef(_)),
            "{base_ref} gave {error:?}"
        );
        assert_eq!(error.status(), axum::http::StatusCode::BAD_REQUEST);
    }
}

#[tokio::test]
async fn a_malformed_base_is_an_invalid_ref() {
    let project = Project::create().await;

    for base_ref in ["..", "main..other", "refs/nope/main", "-delete-everything"] {
        let error = resolve_base(&project.guard, &project.paths, Some(base_ref), "main")
            .await
            .expect_err("a malformed base is refused");

        assert!(
            matches!(error, GitError::InvalidRef(_)),
            "{base_ref} gave {error:?}"
        );
        assert_eq!(error.status(), axum::http::StatusCode::BAD_REQUEST);
    }
}

#[tokio::test]
async fn a_default_branch_that_does_not_resolve_is_reported_against_that_branch() {
    let project = Project::create().await;

    let error = resolve_base(&project.guard, &project.paths, None, "not-a-branch-here")
        .await
        .expect_err("the project default has to exist too");

    let GitError::UnknownRef(ref name) = error else {
        panic!("expected an unknown ref, got {error:?}");
    };
    assert_eq!(name, "not-a-branch-here");
}

#[tokio::test]
async fn a_relaunch_replaces_whatever_a_failed_attempt_left_behind() {
    let project = Project::create().await;
    let session_id = Uuid::new_v4();
    let work = project.work(session_id);

    let base = project.launch(session_id, None).await;
    let stale = commit_in_work(&work, "stale.txt", "stale\n", "chore: stale").await;
    assert_ne!(stale, base.commit);

    // The launcher only does this for a fresh launch; the point is that it
    // succeeds rather than tripping over the directory that is already there.
    let base = project.launch(session_id, None).await;

    assert_clone_is_well_formed(&project, session_id, &base).await;
    assert!(!work.join("stale.txt").exists(), "the old clone survived");
}

#[tokio::test]
async fn fetch_back_publishes_the_session_branch_and_follows_a_rewritten_one() {
    let project = Project::create().await;
    let session_id = Uuid::new_v4();
    let repo = project.repo();
    let work = project.work(session_id);

    project.launch(session_id, None).await;

    let tip = commit_in_work(&work, "agent.txt", "first\n", "feat: the agent's work").await;
    let published = fetch_back(&project.guard, &project.paths, session_id)
        .await
        .expect("the session branch is fetched back");

    assert_eq!(published, tip);
    assert_eq!(
        commit_of(&repo, &format!("refs/sessions/{session_id}")).await,
        tip
    );

    // The session owns its ref, so a rewritten history is followed rather than
    // rejected: the refspec is forced.
    run_git(
        &work,
        &["commit", "--quiet", "--amend", "-m", "feat: reworded"],
    )
    .await;
    let amended = run_git(&work, &["rev-parse", "HEAD"])
        .await
        .trim()
        .to_string();
    assert_ne!(amended, tip);

    let republished = fetch_back(&project.guard, &project.paths, session_id)
        .await
        .expect("a rewritten session branch is fetched back too");

    assert_eq!(republished, amended);
    assert_eq!(
        commit_of(&repo, &format!("refs/sessions/{session_id}")).await,
        amended
    );
}

#[tokio::test]
async fn fetch_back_without_a_work_clone_is_an_unknown_ref() {
    let project = Project::create().await;
    let session_id = Uuid::new_v4();

    let error = fetch_back(&project.guard, &project.paths, session_id)
        .await
        .expect_err("there is nothing to fetch back");

    let GitError::UnknownRef(ref name) = error else {
        panic!("expected an unknown ref, got {error:?}");
    };
    assert_eq!(name, &session_branch(session_id));
}

#[tokio::test]
async fn fetch_back_leaves_the_project_ref_alone_when_the_branch_is_gone() {
    let project = Project::create().await;
    let session_id = Uuid::new_v4();
    let repo = project.repo();
    let work = project.work(session_id);

    project.launch(session_id, None).await;
    let tip = commit_in_work(&work, "agent.txt", "first\n", "feat: the agent's work").await;
    fetch_back(&project.guard, &project.paths, session_id)
        .await
        .expect("the first sync succeeds");

    // The agent deleted its own branch inside the container.
    run_git(&work, &["checkout", "--quiet", "--detach"]).await;
    run_git(
        &work,
        &["branch", "--quiet", "-D", &session_branch(session_id)],
    )
    .await;

    let error = fetch_back(&project.guard, &project.paths, session_id)
        .await
        .expect_err("a deleted session branch has nothing to fetch back");

    let GitError::UnknownRef(ref name) = error else {
        panic!("expected an unknown ref, got {error:?}");
    };
    assert_eq!(name, &session_branch(session_id));
    assert_eq!(
        commit_of(&repo, &format!("refs/sessions/{session_id}")).await,
        tip,
        "the published ref was disturbed by a failed fetch-back"
    );
}

/// Whether `refs/sessions/<session_id>` exists in `repo`.
async fn has_session_ref(repo: &Path, session_id: Uuid) -> bool {
    refs::list_sessions(repo)
        .await
        .expect("the session refs list")
        .contains(&session_id)
}

#[tokio::test]
async fn an_ended_fetch_back_at_the_base_writes_no_ref() {
    let project = Project::create().await;
    let session_id = Uuid::new_v4();
    let base = project.launch(session_id, None).await;

    let outcome = fetch_back_ended(
        &project.guard,
        &project.paths,
        session_id,
        Some(&base.commit),
    )
    .await
    .expect("the ended fetch-back succeeds");

    assert_eq!(
        outcome,
        FetchedBack {
            commit: base.commit.clone(),
            kept: false,
        }
    );
    assert!(!has_session_ref(&project.repo(), session_id).await);
    assert!(
        project.work(session_id).is_dir(),
        "the work clone is left alone"
    );
}

#[tokio::test]
async fn an_ended_fetch_back_deletes_a_ref_an_earlier_sync_left_at_the_base() {
    let project = Project::create().await;
    let session_id = Uuid::new_v4();
    let base = project.launch(session_id, None).await;
    fetch_back(&project.guard, &project.paths, session_id)
        .await
        .expect("a live fetch-back keeps a ref at the base");
    assert!(has_session_ref(&project.repo(), session_id).await);

    fetch_back_ended(
        &project.guard,
        &project.paths,
        session_id,
        Some(&base.commit),
    )
    .await
    .expect("the ended fetch-back succeeds");

    assert!(!has_session_ref(&project.repo(), session_id).await);
}

#[tokio::test]
async fn an_ended_fetch_back_with_commits_publishes_them() {
    let project = Project::create().await;
    let session_id = Uuid::new_v4();
    let base = project.launch(session_id, None).await;
    let tip = commit_in_work(
        &project.work(session_id),
        "agent.txt",
        "work\n",
        "feat: the agent's work",
    )
    .await;

    let outcome = fetch_back_ended(
        &project.guard,
        &project.paths,
        session_id,
        Some(&base.commit),
    )
    .await
    .expect("the ended fetch-back succeeds");

    assert_eq!(
        outcome,
        FetchedBack {
            commit: tip.clone(),
            kept: true,
        }
    );
    assert_eq!(
        commit_of(&project.repo(), &format!("refs/sessions/{session_id}")).await,
        tip
    );
}

#[tokio::test]
async fn an_ended_fetch_back_with_no_recorded_base_is_judged_by_containment() {
    let project = Project::create().await;
    let session_id = Uuid::new_v4();
    let base = project.launch(session_id, None).await;

    // Untouched, and so contained in `main`, the head it was cloned from.
    let outcome = fetch_back_ended(&project.guard, &project.paths, session_id, None)
        .await
        .expect("the ended fetch-back succeeds");
    assert_eq!(
        outcome,
        FetchedBack {
            commit: base.commit.clone(),
            kept: false,
        }
    );
    assert!(!has_session_ref(&project.repo(), session_id).await);

    let tip = commit_in_work(
        &project.work(session_id),
        "agent.txt",
        "work\n",
        "feat: the agent's work",
    )
    .await;
    let outcome = fetch_back_ended(&project.guard, &project.paths, session_id, None)
        .await
        .expect("the ended fetch-back succeeds");
    assert!(outcome.kept);
    assert_eq!(
        commit_of(&project.repo(), &format!("refs/sessions/{session_id}")).await,
        tip
    );
}

#[tokio::test]
async fn an_ended_fetch_back_whose_tip_a_hand_off_or_a_head_contains_keeps_no_ref() {
    let project = Project::create().await;
    let repo = project.repo();
    let handed = Uuid::new_v4();
    let merged = Uuid::new_v4();
    let handed_base = project.launch(handed, None).await;
    let merged_base = project.launch(merged, None).await;

    let handed_tip = commit_in_work(&project.work(handed), "a.txt", "a\n", "feat: a").await;
    let merged_tip = commit_in_work(&project.work(merged), "b.txt", "b\n", "feat: b").await;
    fetch_back(&project.guard, &project.paths, handed)
        .await
        .expect("the live sync succeeds");
    fetch_back(&project.guard, &project.paths, merged)
        .await
        .expect("the live sync succeeds");

    // A hand-off of the first tip, and a head that moved past the second.
    refs::retain_handoff(&repo, Uuid::new_v4(), &handed_tip)
        .await
        .expect("the hand-off is pinned");
    refs::update(&repo, "refs/heads/release", &merged_tip, None)
        .await
        .expect("the head is written");

    for (session_id, base, tip) in [
        (handed, &handed_base, handed_tip),
        (merged, &merged_base, merged_tip),
    ] {
        let outcome = fetch_back_ended(
            &project.guard,
            &project.paths,
            session_id,
            Some(&base.commit),
        )
        .await
        .expect("the ended fetch-back succeeds");

        assert_eq!(
            outcome,
            FetchedBack {
                commit: tip,
                kept: false
            }
        );
        assert!(!has_session_ref(&repo, session_id).await);
    }
}

#[tokio::test]
async fn an_ended_fetch_back_whose_tip_only_upstream_refs_contain_keeps_the_ref() {
    let project = Project::create().await;
    let repo = project.repo();
    let session_id = Uuid::new_v4();
    let base = project.launch(session_id, None).await;
    let tip = commit_in_work(&project.work(session_id), "a.txt", "a\n", "feat: a").await;
    fetch_back(&project.guard, &project.paths, session_id)
        .await
        .expect("the live sync succeeds");

    refs::update(&repo, "refs/remotes/origin/main", &tip, None)
        .await
        .expect("the upstream-tracking ref is written");
    refs::update(&repo, "refs/tags/v-session", &tip, None)
        .await
        .expect("the tag is written");

    let outcome = fetch_back_ended(
        &project.guard,
        &project.paths,
        session_id,
        Some(&base.commit),
    )
    .await
    .expect("the ended fetch-back succeeds");

    assert_eq!(
        outcome,
        FetchedBack {
            commit: tip.clone(),
            kept: true
        }
    );
    assert_eq!(
        commit_of(&repo, &format!("refs/sessions/{session_id}")).await,
        tip
    );
}

#[tokio::test]
async fn an_ended_fetch_back_with_nothing_to_read_leaves_the_ref_alone() {
    let project = Project::create().await;
    let session_id = Uuid::new_v4();
    let repo = project.repo();
    let work = project.work(session_id);
    let base = project.launch(session_id, None).await;
    fetch_back(&project.guard, &project.paths, session_id)
        .await
        .expect("the first sync succeeds");

    run_git(&work, &["checkout", "--quiet", "--detach"]).await;
    run_git(
        &work,
        &["branch", "--quiet", "-D", &session_branch(session_id)],
    )
    .await;

    let error = fetch_back_ended(
        &project.guard,
        &project.paths,
        session_id,
        Some(&base.commit),
    )
    .await
    .expect_err("a deleted session branch has nothing to fetch back");

    assert!(matches!(error, GitError::UnknownRef(_)), "{error:?}");
    assert!(
        has_session_ref(&repo, session_id).await,
        "a failed fetch-back deletes nothing"
    );
}

#[tokio::test]
async fn removing_a_work_clone_is_idempotent_and_leaves_the_project_repository_alone() {
    let project = Project::create().await;
    let session_id = Uuid::new_v4();
    let repo = project.repo();
    let work = project.work(session_id);

    project.launch(session_id, None).await;
    let tip = commit_in_work(&work, "agent.txt", "first\n", "feat: the agent's work").await;
    fetch_back(&project.guard, &project.paths, session_id)
        .await
        .expect("the session is synced before it is deleted");

    remove_work_clone(&project.paths, session_id).expect("the work clone is removed");
    assert!(!work.exists());
    remove_work_clone(&project.paths, session_id).expect("removing it again is the wanted state");

    // The objects the clone borrowed were never its own, so the published ref
    // and its commit are still there.
    assert_eq!(
        commit_of(&repo, &format!("refs/sessions/{session_id}")).await,
        tip
    );
    assert_eq!(
        run_git(&repo, &["cat-file", "-t", &tip]).await.trim(),
        "commit"
    );
}

#[tokio::test]
async fn the_test_fixture_identity_is_not_what_a_session_clone_is_configured_with() {
    // A guard on the assertions above: `run_git` commits under the fixture
    // identity through the environment, so a clone that was never configured
    // would still produce commits and hide the missing configuration.
    let project = Project::create().await;
    let session_id = Uuid::new_v4();

    project.launch(session_id, None).await;

    let identity = launching_user();
    assert_ne!(identity.name, TEST_AUTHOR_NAME);
    assert_ne!(identity.email, TEST_AUTHOR_EMAIL);
    assert_eq!(
        config_value(&project.work(session_id), "user.email").await,
        Some(identity.email)
    );
}
