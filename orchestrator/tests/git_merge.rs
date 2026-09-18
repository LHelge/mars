//! Merging into an integration head against real repositories
//! (`ARCHITECTURE.md`, "Git model", Merge, rebase, push and Commit identity;
//! `SPEC.md`, "Git"; ADR 0007, ADR 0017).
//!
//! Git is never mocked (`CLAUDE.md`, "Testing expectations"), so every case
//! here builds an upstream, a project repository and real session work clones
//! in `tempfile` directories and merges what an agent actually committed. What
//! only a real repository shows is what this asserts: that the target
//! integration head moves and nothing else does, that a merge commit carries
//! the bot identity and the `Requested-By` trailer, that a conflict leaves the
//! target untouched and the temporary clone gone, and that the temporary
//! `refs/tmp/*` of the temporary clone never reach the project repository.
//!
//! No database and no `TestApp`: nothing in this file touches Postgres, so it
//! needs no container engine either.

#![cfg(feature = "integration-tests")]

use std::path::{Path, PathBuf};

use mars_orchestrator::git::testutil::{TestUpstream, run_git};
use mars_orchestrator::git::{
    CommitIdentity, DataPaths, GitActor, GitError, GitRef, MergeOutcome, ProjectGitGuard,
    ProjectGitLocks, ResolvedRef, create_work_clone, fetch_back, fetch_upstream, init_project_repo,
    merge, refs, resolve_base,
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
/// the one every merge commit here must carry.
fn bot() -> CommitIdentity {
    CommitIdentity {
        name: "Mars Bot".to_string(),
        email: "mars-bot@example.invalid".to_string(),
    }
}

/// A `DATA_DIR`, one project git lock held for the whole test and an
/// initialised project repository — the state a merge request finds
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

    /// Launch a session from `base_ref`, commit `content` to `path` in its
    /// work clone and fetch the branch back. Returns the session's id and the
    /// commit its ref now points at, as the service does before a merge.
    async fn session_with_commit(
        &self,
        base_ref: Option<&str>,
        path: &str,
        content: &str,
        message: &str,
    ) -> (Uuid, String) {
        let session_id = Uuid::new_v4();
        let base = resolve_base(&self.guard, &self.paths, base_ref, "main")
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

        let work = self.paths.session_work(session_id);
        let file = work.join(path);
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent).expect("the parent directory is created");
        }
        std::fs::write(&file, content).expect("the file is written");
        run_git(&work, &["add", "--", path]).await;
        run_git(&work, &["commit", "--quiet", "-m", message]).await;

        let commit = fetch_back(&self.guard, &self.paths, session_id)
            .await
            .expect("the session branch is fetched back");

        (session_id, commit)
    }

    /// Resolve one ref in the project repository, as a route does under the
    /// lock before calling the primitive.
    async fn resolve(&self, name: &str) -> ResolvedRef {
        let git_ref = GitRef::parse(name).expect("a parsable ref name");
        refs::resolve(&self.repo(), &git_ref)
            .await
            .unwrap_or_else(|err| panic!("{name} resolves: {err}"))
    }

    /// Merge `source` into `target` by API name, with the bot identity.
    async fn merge(
        &self,
        source: &str,
        target: &str,
        message: Option<&str>,
        requested_by: &GitActor,
    ) -> std::result::Result<MergeOutcome, GitError> {
        let source = self.resolve(source).await;
        let target = self.resolve(target).await;

        merge(
            &self.guard,
            &self.paths,
            &source,
            &target,
            message,
            &bot(),
            requested_by,
        )
        .await
    }

    /// The commit an API-named ref points at in the project repository.
    async fn commit_of(&self, name: &str) -> String {
        self.resolve(name).await.commit
    }

    /// Everything left in `DATA_DIR/tmp/`. A finished operation leaves
    /// nothing: the temporary clone is removed on every exit path.
    fn tmp_entries(&self) -> Vec<PathBuf> {
        match std::fs::read_dir(self.paths.tmp()) {
            Ok(entries) => entries
                .map(|entry| entry.expect("a readable directory entry").path())
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Assert the operation left no temporary clone and wrote no `refs/tmp/*`
    /// into the project repository.
    async fn assert_no_leftovers(&self) {
        assert_eq!(
            self.tmp_entries(),
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

/// One commit's author, committer and full message.
async fn commit_details(repo: &Path, commit: &str) -> (String, String, String, String, String) {
    let shown = run_git(
        repo,
        &["show", "--quiet", "--format=%an%n%ae%n%cn%n%ce%n%B", commit],
    )
    .await;

    let mut lines = shown.splitn(5, '\n');
    let author_name = lines.next().unwrap_or_default().to_string();
    let author_email = lines.next().unwrap_or_default().to_string();
    let committer_name = lines.next().unwrap_or_default().to_string();
    let committer_email = lines.next().unwrap_or_default().to_string();
    let message = lines.next().unwrap_or_default().to_string();

    (
        author_name,
        author_email,
        committer_name,
        committer_email,
        message,
    )
}

/// The parents of one commit, as full object ids.
async fn parents_of(repo: &Path, commit: &str) -> Vec<String> {
    run_git(repo, &["rev-parse", &format!("{commit}^@")])
        .await
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

#[tokio::test]
async fn a_diverged_session_branch_is_merged_with_the_bot_identity_and_a_trailer() {
    let project = Project::create().await;
    let user_id = Uuid::new_v4();

    // Two sessions off the same `main`, touching different files: the first
    // fast-forwards `main`, which makes the second a real merge.
    let (first_session, first_commit) = project
        .session_with_commit(None, "first.txt", "first\n", "feat: the first session")
        .await;
    let (second, second_commit) = project
        .session_with_commit(None, "second.txt", "second\n", "feat: the second session")
        .await;

    project
        .merge(&first_session.to_string(), "main", None, &GitActor::System)
        .await
        .expect("the first session merges");
    assert_eq!(project.commit_of("main").await, first_commit);

    let outcome = project
        .merge(&second.to_string(), "main", None, &GitActor::User(user_id))
        .await
        .expect("the second session merges");

    assert!(
        !outcome.fast_forward,
        "a diverged branch cannot fast-forward"
    );
    assert_eq!(project.commit_of("main").await, outcome.commit);

    let repo = project.repo();
    assert_eq!(
        parents_of(&repo, &outcome.commit).await,
        vec![first_commit, second_commit.clone()],
        "the merge commit does not join the two branches"
    );

    let (author_name, author_email, committer_name, committer_email, message) =
        commit_details(&repo, &outcome.commit).await;
    let bot = bot();
    assert_eq!(author_name, bot.name);
    assert_eq!(author_email, bot.email);
    assert_eq!(committer_name, bot.name);
    assert_eq!(committer_email, bot.email);
    assert!(
        message.starts_with(&format!("Merge {second} into main\n")),
        "unexpected default message: {message}"
    );
    assert!(
        message.contains(&format!("\n\nRequested-By: user:{user_id}")),
        "the trailer is missing or not preceded by a blank line: {message}"
    );

    // The session's own ref is untouched by the merge; only the target moved.
    assert_eq!(
        project.commit_of(&format!("refs/sessions/{second}")).await,
        second_commit
    );
    project.assert_no_leftovers().await;
}

#[tokio::test]
async fn a_session_branch_ahead_of_an_untouched_target_fast_forwards() {
    let project = Project::create().await;
    let before = project.commit_of("main").await;

    let (session_id, commit) = project
        .session_with_commit(None, "agent.txt", "work\n", "feat: the agent's work")
        .await;

    let outcome = project
        .merge(
            &session_id.to_string(),
            "main",
            None,
            &GitActor::Session(session_id),
        )
        .await
        .expect("an unchanged target fast-forwards");

    assert!(outcome.fast_forward, "{outcome:?}");
    assert_eq!(outcome.commit, commit);
    assert_eq!(project.commit_of("main").await, commit);
    assert_eq!(
        parents_of(&project.repo(), &commit).await,
        vec![before],
        "a fast-forward must not create a merge commit"
    );
    project.assert_no_leftovers().await;
}

#[tokio::test]
async fn a_supplied_message_is_kept_and_still_gains_the_trailer() {
    let project = Project::create().await;
    let session_id = Uuid::new_v4();

    // A real merge is needed for the message to survive at all: a
    // fast-forward makes no commit.
    let (ahead_session, _) = project
        .session_with_commit(None, "ahead.txt", "ahead\n", "feat: ahead")
        .await;
    let (other, _) = project
        .session_with_commit(None, "other.txt", "other\n", "feat: other")
        .await;
    project
        .merge(&ahead_session.to_string(), "main", None, &GitActor::System)
        .await
        .expect("the first session merges");

    let outcome = project
        .merge(
            &other.to_string(),
            "main",
            Some("chore: integrate the other session"),
            &GitActor::Session(session_id),
        )
        .await
        .expect("the second session merges");

    let (_, _, _, _, message) = commit_details(&project.repo(), &outcome.commit).await;
    assert_eq!(
        message.trim_end(),
        format!("chore: integrate the other session\n\nRequested-By: session:{session_id}"),
        "unexpected message: {message}"
    );
}

#[tokio::test]
async fn a_conflict_reports_its_paths_and_leaves_the_target_where_it_was() {
    let project = Project::create().await;

    let (first, first_commit) = project
        .session_with_commit(
            None,
            "README.md",
            "# the first session's line\n",
            "docs: the first session",
        )
        .await;
    let (second, second_commit) = project
        .session_with_commit(
            None,
            "README.md",
            "# the second session's line\n",
            "docs: the second session",
        )
        .await;

    project
        .merge(&first.to_string(), "main", None, &GitActor::System)
        .await
        .expect("the first session merges");
    let after_first = project.commit_of("main").await;
    assert_eq!(after_first, first_commit);

    let error = project
        .merge(&second.to_string(), "main", None, &GitActor::System)
        .await
        .expect_err("the second session conflicts");

    let GitError::Conflict { ref paths } = error else {
        panic!("expected a conflict, got {error:?}");
    };
    assert_eq!(paths, &vec!["README.md".to_string()]);
    assert_eq!(error.status(), axum::http::StatusCode::UNPROCESSABLE_ENTITY);

    assert_eq!(
        project.commit_of("main").await,
        after_first,
        "a conflicting merge moved the target"
    );
    assert_eq!(
        project.commit_of(&format!("refs/sessions/{second}")).await,
        second_commit,
        "a conflicting merge disturbed the source"
    );
    project.assert_no_leftovers().await;
}

#[tokio::test]
async fn fetched_upstream_changes_are_integrated_by_merging_the_tracking_ref() {
    let project = Project::create().await;
    let before = project.commit_of("main").await;

    let upstream_commit = project
        .upstream
        .commit_file("main", "upstream.txt", "upstream\n", "feat: upstream moved")
        .await;
    fetch_upstream(&project.guard, &project.paths, None)
        .await
        .expect("the upstream fetch succeeds");

    // The fetch refreshes upstream tracking only; the integration head is
    // Mars's and moves through this explicit merge (ADR 0017).
    assert_eq!(project.commit_of("main").await, before);
    assert_eq!(project.commit_of("origin/main").await, upstream_commit);

    let outcome = project
        .merge("origin/main", "main", None, &GitActor::System)
        .await
        .expect("an upstream-tracking ref is a merge source");

    assert!(outcome.fast_forward, "{outcome:?}");
    assert_eq!(outcome.commit, upstream_commit);
    assert_eq!(project.commit_of("main").await, upstream_commit);
    project.assert_no_leftovers().await;
}

#[tokio::test]
async fn a_commit_id_source_needs_no_ref_of_its_own() {
    let project = Project::create().await;

    // What a task merge passes: a hand-off's pinned commit (ADR 0018), given
    // as an object id rather than as a branch that could move.
    let (session_id, commit) = project
        .session_with_commit(None, "handoff.txt", "reviewed\n", "feat: reviewed work")
        .await;
    refs::delete(&project.repo(), &format!("refs/sessions/{session_id}"))
        .await
        .expect("the session ref is removed");

    let outcome = project
        .merge(&commit, "main", None, &GitActor::System)
        .await
        .expect("a commit id is a merge source");

    assert_eq!(outcome.commit, commit);
    assert_eq!(project.commit_of("main").await, commit);
    project.assert_no_leftovers().await;
}

#[tokio::test]
async fn merging_something_already_merged_changes_nothing() {
    let project = Project::create().await;
    let (session_id, commit) = project
        .session_with_commit(None, "agent.txt", "work\n", "feat: the agent's work")
        .await;

    project
        .merge(&session_id.to_string(), "main", None, &GitActor::System)
        .await
        .expect("the first merge succeeds");
    let target = project.commit_of("main").await;
    assert_eq!(target, commit);

    for source in [session_id.to_string(), "main".to_string()] {
        let outcome = project
            .merge(&source, "main", None, &GitActor::System)
            .await
            .unwrap_or_else(|err| panic!("merging {source} again is a no-op: {err}"));

        assert_eq!(outcome.commit, target);
        assert!(
            !outcome.fast_forward,
            "nothing moved, so nothing fast-forwarded: {outcome:?}"
        );
        assert_eq!(project.commit_of("main").await, target);
    }

    project.assert_no_leftovers().await;
}

#[tokio::test]
async fn only_an_integration_head_may_be_a_target() {
    let project = Project::create().await;
    let (session_id, _) = project
        .session_with_commit(None, "agent.txt", "work\n", "feat: the agent's work")
        .await;
    let before = project.commit_of("main").await;

    for target in ["origin/main", &session_id.to_string(), "refs/tags/v1.0.0"] {
        // The tag has to exist for the resolution this stands in for to get
        // as far as the primitive.
        if target == "refs/tags/v1.0.0" {
            project.upstream.tag("v1.0.0", "main", true).await;
            fetch_upstream(&project.guard, &project.paths, None)
                .await
                .expect("the tag is fetched");
        }

        let error = project
            .merge("main", target, None, &GitActor::System)
            .await
            .expect_err("only integration heads are mutation targets");

        assert!(
            matches!(error, GitError::InvalidRef(_)),
            "{target} gave {error:?}"
        );
        assert_eq!(error.status(), axum::http::StatusCode::BAD_REQUEST);
    }

    assert_eq!(project.commit_of("main").await, before);
    project.assert_no_leftovers().await;
}

#[tokio::test]
async fn a_tag_is_not_a_merge_source() {
    let project = Project::create().await;
    project.upstream.tag("v1.0.0", "main", true).await;
    fetch_upstream(&project.guard, &project.paths, None)
        .await
        .expect("the tag is fetched");
    let before = project.commit_of("main").await;

    let error = project
        .merge("refs/tags/v1.0.0", "main", None, &GitActor::System)
        .await
        .expect_err("a tag may not be merged");

    assert!(matches!(error, GitError::InvalidRef(_)), "{error:?}");
    assert_eq!(project.commit_of("main").await, before);
    project.assert_no_leftovers().await;
}
