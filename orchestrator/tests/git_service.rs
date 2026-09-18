//! [`GitService`]: the composite operations, their locking and ref-validation
//! order, and the `git` outcome events they record
//! (`ARCHITECTURE.md`, "Git model", Fetch-back, Merge, rebase, push,
//! Serialization, Diff; "Session owner task"; "MCP design", Side effects;
//! `SPEC.md`, "AgentEvent", "Git", "Sessions"; ADR 0007, ADR 0021, ADR 0028).
//!
//! The primitives have their own suites against real repositories
//! (`tests/git_merge.rs`, `git_rebase.rs`, `git_push.rs`, `git_diff.rs`,
//! `git_session.rs`). What only the service shows is everything around them,
//! and that is what this file asserts: that a session's branch is synced
//! before it is merged, rebased or pushed, that the outcome reaches the right
//! sessions' transcripts as a `git` event with the documented `detail`, that a
//! conflict and a non-fast-forward rejection are recorded as outcomes rather
//! than swallowed, that the read-only operations record nothing at all, and
//! that a ref of the wrong kind is refused before anything is locked, fetched
//! or written.
//!
//! Git is never mocked (`CLAUDE.md`, "Testing expectations"): every case here
//! builds a real upstream, a real project repository and real session work
//! clones under the `TestApp`'s own `DATA_DIR`. Only the credential provider
//! is a mock, and every value it hands out is obviously fake (rule 3).
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use std::time::Duration;

use common::TestApp;
use mars_orchestrator::git::testutil::{TestUpstream, run_git, test_identity};
use mars_orchestrator::git::{
    DataPaths, DiffSelector, GitActor, GitError, GitRef, GitService, WorkTreeOutcome,
    create_work_clone, init_project_repo, refs, resolve_base,
};
use mars_orchestrator::models::{
    BranchName, EventRow, NewAgentProfile, NewProject, NewSession, ProfileKind, Project,
    ProjectStatus, RemoteUrl, User,
};
use mars_orchestrator::prelude::*;
use mars_orchestrator::repositories::{ProjectRepository, SessionRepository};
use serde_json::Value;
use sqlx::postgres::PgListener;
use uuid::Uuid;

/// Not a real remote: the fixture value every project test stores (rule 3).
const TEST_REMOTE: &str = "https://git.example.com/fake/repo.git";

/// A GitHub-shaped remote, for the one case that asserts the compare link. The
/// organisation and repository do not exist.
const GITHUB_REMOTE: &str = "https://github.com/example-org/fake-repo.git";

/// How long a test waits for the `session_events` notification.
const NOTIFY_TIMEOUT: Duration = Duration::from_secs(10);

/// A ready project with a real repository, an upstream behind it and a user
/// and profile to hang sessions off.
struct Fixture {
    app: TestApp,
    upstream: TestUpstream,
    project: Project,
    user: User,
    profile_id: Uuid,
}

impl Fixture {
    /// The usual case: a project whose remote is not a GitHub one.
    async fn create(name: &str) -> Self {
        Self::create_with_remote(name, TEST_REMOTE).await
    }

    /// A project whose stored `remote_url` is `remote`, which is what decides
    /// whether a push can offer a compare link.
    async fn create_with_remote(name: &str, remote: &str) -> Self {
        let app = TestApp::spawn().await;
        let upstream = TestUpstream::create().await;

        let mut new_project = NewProject::new(name, remote).expect("the test project is valid");
        new_project.default_branch =
            Some(BranchName::parse("main").expect("main is a branch name"));

        let projects = ProjectRepository::new(&app.pool);
        let mut tx = app.pool.begin().await.expect("a transaction begins");
        let project = projects
            .insert(&mut tx, &new_project)
            .await
            .expect("the project inserts");
        tx.commit().await.expect("the transaction commits");

        {
            let guard = app.state.git_locks.lock(project.id).await;
            init_project_repo(
                &guard,
                &DataPaths::from_config(&app.state.config),
                &RemoteUrl::local_for_tests(&upstream.path),
                Some("main"),
                None,
            )
            .await
            .expect("the project repository is initialised");
        }

        let mut tx = app.pool.begin().await.expect("a transaction begins");
        let project = projects
            .set_status(&mut tx, project.id, ProjectStatus::Ready, None)
            .await
            .expect("the status is set")
            .expect("the project exists");
        tx.commit().await.expect("the transaction commits");

        let user = app
            .insert_user("ada", "ada@example.com", false, false)
            .await;

        let profile = NewAgentProfile::new(project.id, "default", "localhost/mars-session:test")
            .expect("the test profile is valid");
        let mut tx = app.pool.begin().await.expect("a transaction begins");
        let profile = projects
            .insert_profile(&mut tx, &profile)
            .await
            .expect("the profile inserts");
        tx.commit().await.expect("the transaction commits");

        Self {
            app,
            upstream,
            project,
            user,
            profile_id: profile.id,
        }
    }

    /// The service under test, built exactly as a handler builds it.
    fn service(&self) -> GitService {
        GitService::from_state(&self.app.state)
    }

    fn paths(&self) -> DataPaths {
        DataPaths::from_config(&self.app.state.config)
    }

    /// A session row with no work clone: what a session in `creating` looks
    /// like to git.
    async fn seed_session(&self) -> Uuid {
        let mut session = NewSession::new(
            self.project.id,
            self.profile_id,
            ProfileKind::Conversational,
            "main",
            format!("fake-mcp-token-hash-{}", Uuid::new_v4()),
        );
        session.created_by = Some(self.user.id);

        let mut tx = self.app.pool.begin().await.expect("a transaction begins");
        let inserted = SessionRepository::new(&self.app.pool)
            .insert(&mut tx, &session)
            .await
            .expect("the session inserts");
        tx.commit().await.expect("the transaction commits");

        inserted.id
    }

    /// A session with a work clone and one commit in it, *not* fetched back:
    /// the state the service finds when it is asked to sync, merge, rebase,
    /// push or diff.
    async fn session_with_commit(&self, path: &str, content: &str, message: &str) -> Uuid {
        let session_id = self.seed_session().await;
        let paths = self.paths();

        {
            let guard = self.app.state.git_locks.lock(self.project.id).await;
            let base = resolve_base(&guard, &paths, None, "main")
                .await
                .expect("the base resolves");
            create_work_clone(&guard, &paths, session_id, &base, &test_identity())
                .await
                .expect("the work clone is created");
        }

        let work = paths.session_work(session_id);
        let file = work.join(path);
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent).expect("the parent directory is created");
        }
        std::fs::write(&file, content).expect("the file is written");
        run_git(&work, &["add", "--", path]).await;
        run_git(&work, &["commit", "--quiet", "-m", message]).await;

        session_id
    }

    /// Every event stored for a session, oldest first.
    async fn events(&self, session_id: Uuid) -> Vec<EventRow> {
        SessionRepository::new(&self.app.pool)
            .list_events(session_id, None, 100)
            .await
            .expect("the events are read")
            .0
    }

    /// The only event stored for a session, with its payload.
    async fn only_event(&self, session_id: Uuid) -> (EventRow, Value) {
        let mut events = self.events(session_id).await;
        assert_eq!(events.len(), 1, "expected exactly one event: {events:?}");
        let row = events.remove(0);
        let payload = row.payload.clone();

        assert_eq!(row.kind, "git");
        (row, payload)
    }

    /// The commit a ref in the project repository points at, by API name.
    async fn commit_of(&self, name: &str) -> String {
        let git_ref = GitRef::parse(name).expect("a parsable ref name");

        refs::resolve(&self.paths().project_repo(self.project.id), &git_ref)
            .await
            .unwrap_or_else(|err| panic!("{name} resolves: {err}"))
            .commit
    }
}

#[tokio::test]
async fn an_explicit_sync_records_one_git_event_and_announces_it() {
    let fixture = Fixture::create("sync").await;
    let session_id = fixture
        .session_with_commit("NOTES.md", "work\n", "docs: notes")
        .await;

    let mut listener = PgListener::connect_with(&fixture.app.pool)
        .await
        .expect("a listener connects");
    listener
        .listen("session_events")
        .await
        .expect("the listener subscribes");

    let outcome = fixture
        .service()
        .sync_session(
            fixture.project.id,
            session_id,
            &GitActor::User(fixture.user.id),
        )
        .await
        .expect("the sync succeeds");

    assert_eq!(outcome.git_ref, refs::session_ref(session_id));
    assert_eq!(
        outcome.commit,
        fixture.commit_of(&session_id.to_string()).await,
        "the answer names the commit the session ref now points at"
    );

    let (row, payload) = fixture.only_event(session_id).await;
    assert_eq!(row.seq, 1, "the first event of a session is seq 1");
    assert_eq!(payload["op"], "sync");
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["detail"]["ref"], refs::session_ref(session_id));
    assert_eq!(payload["detail"]["commit"], outcome.commit);
    assert!(
        payload["detail"].get("error").is_none(),
        "a successful sync carries no error: {payload}"
    );

    // ADR 0028: the notification is issued inside the writing transaction, so
    // it arrives only once the row is readable.
    let notification = tokio::time::timeout(NOTIFY_TIMEOUT, listener.recv())
        .await
        .expect("the notification arrives")
        .expect("the listener stays healthy");
    assert_eq!(notification.payload(), format!("{session_id}:1"));
}

#[tokio::test]
async fn a_session_with_no_work_tree_yet_is_refused_without_an_event() {
    let fixture = Fixture::create("creating").await;
    let session_id = fixture.seed_session().await;

    let error = fixture
        .service()
        .sync_session(
            fixture.project.id,
            session_id,
            &GitActor::User(fixture.user.id),
        )
        .await
        .expect_err("a session still creating has nothing to sync");

    assert_eq!(error.status(), axum::http::StatusCode::BAD_REQUEST);
    assert!(
        matches!(error, Error::Git(GitError::UnknownRef(_))),
        "expected an unknown ref, got {error:?}"
    );
    assert!(
        fixture.events(session_id).await.is_empty(),
        "nothing happened, so nothing is recorded"
    );
}

#[tokio::test]
async fn a_sync_of_a_session_whose_work_clone_is_gone_uses_the_ref_it_left_behind() {
    let fixture = Fixture::create("deleted-work").await;
    let session_id = fixture
        .session_with_commit("GONE.md", "work\n", "docs: gone")
        .await;
    let service = fixture.service();

    let first = service
        .sync_session(fixture.project.id, session_id, &GitActor::System)
        .await
        .expect("the first sync succeeds");

    // What session deletion leaves: the ref in the mirror, no directory.
    std::fs::remove_dir_all(fixture.paths().session_work(session_id))
        .expect("the work clone is removed");

    let second = service
        .sync_session(fixture.project.id, session_id, &GitActor::System)
        .await
        .expect("the ref is still there, so the sync is a no-op rather than a failure");

    assert_eq!(second.commit, first.commit);
    assert_eq!(fixture.events(session_id).await.len(), 2);
}

#[tokio::test]
async fn a_merge_records_its_outcome_on_the_source_session_and_on_the_caller() {
    let fixture = Fixture::create("merge").await;
    let source = fixture
        .session_with_commit("FEATURE.md", "feature\n", "feat: a feature")
        .await;
    let caller = fixture.seed_session().await;

    let outcome = fixture
        .service()
        .merge_branch(
            fixture.project.id,
            &source.to_string(),
            "main",
            None,
            &GitActor::Session(caller),
        )
        .await
        .expect("the merge succeeds");

    assert_eq!(
        fixture.commit_of("main").await,
        outcome.commit,
        "the integration head moved to what the merge produced"
    );
    assert!(
        outcome.fast_forward,
        "a session branched from main with one commit fast-forwards"
    );

    for session_id in [source, caller] {
        let (_, payload) = fixture.only_event(session_id).await;
        assert_eq!(payload["op"], "merge");
        assert_eq!(payload["ok"], true);
        assert_eq!(payload["detail"]["source"], source.to_string());
        assert_eq!(payload["detail"]["target"], "main");
        assert_eq!(payload["detail"]["commit"], outcome.commit);
        assert_eq!(payload["detail"]["fast_forward"], true);
        assert_eq!(
            payload["detail"]["requested_by"],
            format!("session:{caller}"),
            "the event names who asked, as the commit trailer would"
        );
    }
}

#[tokio::test]
async fn a_merge_for_a_user_records_nothing_on_a_session_that_did_not_take_part() {
    let fixture = Fixture::create("bystander").await;
    let source = fixture
        .session_with_commit("SRC.md", "src\n", "feat: src")
        .await;
    let bystander = fixture.seed_session().await;

    fixture
        .service()
        .merge_branch(
            fixture.project.id,
            &source.to_string(),
            "main",
            Some("chore: integrate"),
            &GitActor::User(fixture.user.id),
        )
        .await
        .expect("the merge succeeds");

    assert_eq!(fixture.events(source).await.len(), 1);
    assert!(
        fixture.events(bystander).await.is_empty(),
        "only the sessions whose refs took part are told"
    );
}

#[tokio::test]
async fn a_pinned_commit_is_merged_under_the_callers_own_lock_without_syncing() {
    let fixture = Fixture::create("merge-commit").await;
    let session_id = fixture
        .session_with_commit("PINNED.md", "pinned\n", "feat: pinned")
        .await;
    let service = fixture.service();
    let handoff_id = Uuid::new_v4();

    let pinned = service
        .sync_session(fixture.project.id, session_id, &GitActor::System)
        .await
        .expect("the sync succeeds")
        .commit;

    // The session moves on after the hand-off was pinned; the task merge must
    // take the pinned commit and not this one (ADR 0018).
    let work = fixture.paths().session_work(session_id);
    std::fs::write(work.join("LATER.md"), "later\n").expect("the file is written");
    run_git(&work, &["add", "--", "LATER.md"]).await;
    run_git(&work, &["commit", "--quiet", "-m", "feat: later"]).await;

    let outcome = {
        // The hand-off epic holds the lock across its verification and the
        // merge together, which is what this signature is for.
        let guard = fixture.app.state.git_locks.lock(fixture.project.id).await;
        service
            .merge_commit(
                &guard,
                &pinned,
                &handoff_id.to_string(),
                "main",
                Some("chore: merge the approved hand-off"),
                &GitActor::Session(session_id),
            )
            .await
            .expect("the merge succeeds")
    };

    assert_eq!(
        outcome.commit, pinned,
        "the pinned commit is what moved main"
    );
    assert_eq!(fixture.commit_of("main").await, pinned);
    assert_eq!(
        fixture.commit_of(&session_id.to_string()).await,
        pinned,
        "the session ref was not re-synced, so the later commit stayed behind"
    );

    let events = fixture.events(session_id).await;
    assert_eq!(events.len(), 2, "the sync and the merge: {events:?}");
    let payload = &events[1].payload;
    assert_eq!(payload["op"], "merge");
    assert_eq!(payload["ok"], true);
    assert_eq!(
        payload["detail"]["source"],
        handoff_id.to_string(),
        "the task merge names the hand-off, not a branch"
    );
    assert_eq!(payload["detail"]["target"], "main");
}

#[tokio::test]
async fn a_conflicting_merge_records_ok_false_with_the_conflicting_paths() {
    let fixture = Fixture::create("conflict").await;
    let service = fixture.service();

    let first = fixture
        .session_with_commit("SHARED.md", "one\n", "feat: one")
        .await;
    let second = fixture
        .session_with_commit("SHARED.md", "two\n", "feat: two")
        .await;

    service
        .merge_branch(
            fixture.project.id,
            &first.to_string(),
            "main",
            None,
            &GitActor::System,
        )
        .await
        .expect("the first merge succeeds");
    let main_before = fixture.commit_of("main").await;

    let error = service
        .merge_branch(
            fixture.project.id,
            &second.to_string(),
            "main",
            None,
            &GitActor::User(fixture.user.id),
        )
        .await
        .expect_err("the second merge conflicts");

    assert_eq!(error.status(), axum::http::StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        matches!(&error, Error::GitConflict { conflicts, .. } if conflicts.contains(&"SHARED.md".to_string())),
        "expected the conflicting path in the error, got {error:?}"
    );
    assert_eq!(
        fixture.commit_of("main").await,
        main_before,
        "a conflict leaves the target exactly as it was"
    );

    let (_, payload) = fixture.only_event(second).await;
    assert_eq!(payload["op"], "merge");
    assert_eq!(payload["ok"], false);
    assert_eq!(payload["detail"]["conflicts"][0], "SHARED.md");
    assert_eq!(payload["detail"]["error"], "merge conflict");
    assert!(
        payload["detail"].get("commit").is_none(),
        "a conflict produced no commit: {payload}"
    );
}

#[tokio::test]
async fn a_rebase_records_what_became_of_the_session_checkout() {
    let fixture = Fixture::create("rebase").await;
    let service = fixture.service();

    let ahead = fixture
        .session_with_commit("AHEAD.md", "ahead\n", "feat: ahead")
        .await;
    let branch = fixture
        .session_with_commit("BRANCH.md", "branch\n", "feat: branch")
        .await;

    // `main` moves on under the branch, which is what gives the rebase
    // something to replay onto.
    service
        .merge_branch(
            fixture.project.id,
            &ahead.to_string(),
            "main",
            None,
            &GitActor::System,
        )
        .await
        .expect("the merge succeeds");

    let outcome = service
        .rebase(
            fixture.project.id,
            &branch.to_string(),
            "main",
            &GitActor::User(fixture.user.id),
        )
        .await
        .expect("the rebase succeeds");

    assert_eq!(
        outcome.work_tree,
        WorkTreeOutcome::Updated,
        "a clean checkout on the session branch is reset onto the rewrite"
    );

    let (_, payload) = fixture.only_event(branch).await;
    assert_eq!(payload["op"], "rebase");
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["detail"]["branch"], branch.to_string());
    assert_eq!(payload["detail"]["onto"], "main");
    assert_eq!(payload["detail"]["commit"], outcome.commit);
    assert_eq!(payload["detail"]["work_tree"], "updated");
    assert_eq!(
        payload["detail"]["requested_by"],
        format!("user:{}", fixture.user.id)
    );
}

#[tokio::test]
async fn a_successful_push_records_the_branch_and_the_compare_link() {
    let fixture = Fixture::create_with_remote("push-ok", GITHUB_REMOTE).await;
    let session_id = fixture
        .session_with_commit("PUSHED.md", "pushed\n", "feat: pushed")
        .await;

    let outcome = fixture
        .service()
        .push(
            fixture.project.id,
            &session_id.to_string(),
            None,
            false,
            &GitActor::Session(session_id),
        )
        .await
        .expect("the push succeeds against the fixture upstream");

    assert_eq!(outcome.remote_branch, format!("session/{session_id}"));
    assert_eq!(
        run_git(
            &fixture.upstream.path,
            &["rev-parse", &format!("refs/heads/session/{session_id}")]
        )
        .await
        .trim(),
        outcome.commit,
        "the commit the session had just committed reached upstream"
    );

    let (_, payload) = fixture.only_event(session_id).await;
    assert_eq!(payload["op"], "push");
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["detail"]["ref"], session_id.to_string());
    assert_eq!(
        payload["detail"]["remote_branch"],
        format!("session/{session_id}")
    );
    assert_eq!(payload["detail"]["force"], false);
    assert_eq!(
        payload["detail"]["compare_url"],
        format!(
            "https://github.com/example-org/fake-repo/compare/main...session/{session_id}?expand=1"
        ),
        "the compare link is what the UI offers as \"open a pull request\""
    );
}

#[tokio::test]
async fn a_non_fast_forward_push_records_ok_false_and_changes_nothing() {
    let fixture = Fixture::create("push-rejected").await;
    let session_id = fixture
        .session_with_commit("LOCAL.md", "local\n", "feat: local")
        .await;

    // Upstream `main` advances past everything the mirror knows, so sending a
    // session's branch to it cannot be a fast-forward.
    fixture
        .upstream
        .commit_file("main", "UPSTREAM.md", "upstream\n", "docs: upstream")
        .await;
    let main_before = fixture.commit_of("main").await;

    let error = fixture
        .service()
        .push(
            fixture.project.id,
            &session_id.to_string(),
            Some("main"),
            false,
            &GitActor::Session(session_id),
        )
        .await
        .expect_err("upstream has moved on");

    assert_eq!(error.status(), axum::http::StatusCode::CONFLICT);
    assert!(
        matches!(error, Error::Git(GitError::NonFastForward { .. })),
        "expected a non-fast-forward, got {error:?}"
    );
    assert_eq!(
        fixture.commit_of("main").await,
        main_before,
        "a rejected push retains every local ref"
    );

    let (_, payload) = fixture.only_event(session_id).await;
    assert_eq!(payload["op"], "push");
    assert_eq!(payload["ok"], false);
    assert_eq!(payload["detail"]["remote_branch"], "main");
    assert!(
        payload["detail"]["error"]
            .as_str()
            .expect("the detail names the failure")
            .contains("non-fast-forward"),
        "{payload}"
    );
    assert!(
        payload["detail"].get("commit").is_none(),
        "nothing was published: {payload}"
    );
}

#[tokio::test]
async fn a_diff_of_a_session_head_syncs_silently() {
    let fixture = Fixture::create("diff-head").await;
    let session_id = fixture
        .session_with_commit("CHANGED.md", "changed\n", "feat: changed")
        .await;

    let diff = fixture
        .service()
        .diff(
            fixture.project.id,
            DiffSelector::Head(session_id.to_string()),
            None,
        )
        .await
        .expect("the diff succeeds");

    assert_eq!(diff.base, "main");
    assert_eq!(diff.head, session_id.to_string());
    assert_eq!(
        diff.files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>(),
        vec!["CHANGED.md"],
        "the diff shows work that had not been fetched back before the call"
    );
    assert!(
        fixture.events(session_id).await.is_empty(),
        "the internal fetch-back emits no git event (ARCHITECTURE.md, Diff)"
    );
}

#[tokio::test]
async fn a_diff_by_handoff_id_never_touches_the_work_clone() {
    let fixture = Fixture::create("diff-handoff").await;
    let session_id = fixture
        .session_with_commit("HANDED.md", "handed\n", "feat: handed")
        .await;
    let service = fixture.service();

    let synced = service
        .sync_session(fixture.project.id, session_id, &GitActor::System)
        .await
        .expect("the sync succeeds");

    let handoff_id = Uuid::new_v4();
    refs::retain_handoff(
        &fixture.paths().project_repo(fixture.project.id),
        handoff_id,
        &synced.commit,
    )
    .await
    .expect("the hand-off commit is retained");

    // A hand-off is reviewable after its session is gone, which is exactly
    // what "never syncs a moving branch" has to survive.
    std::fs::remove_dir_all(fixture.paths().session_work(session_id))
        .expect("the work clone is removed");

    let diff = service
        .diff(
            fixture.project.id,
            DiffSelector::Handoff(handoff_id),
            Some("main"),
        )
        .await
        .expect("the diff succeeds without a work clone");

    assert_eq!(diff.head, handoff_id.to_string());
    assert_eq!(
        diff.files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>(),
        vec!["HANDED.md"]
    );
    assert_eq!(
        fixture.events(session_id).await.len(),
        1,
        "only the explicit sync recorded anything"
    );
}

#[tokio::test]
async fn listing_session_branches_records_nothing() {
    let fixture = Fixture::create("list").await;
    let session_id = fixture
        .session_with_commit("LISTED.md", "listed\n", "feat: listed")
        .await;
    let service = fixture.service();

    service
        .sync_session(fixture.project.id, session_id, &GitActor::System)
        .await
        .expect("the sync succeeds");

    let branches = service
        .list_session_branches(fixture.project.id)
        .await
        .expect("the listing succeeds");

    assert_eq!(branches.len(), 1);
    assert_eq!(branches[0].session_id, session_id);
    assert_eq!(branches[0].ahead, 1);
    assert_eq!(
        fixture.events(session_id).await.len(),
        1,
        "the listing is read-only (ARCHITECTURE.md, MCP design, Side effects)"
    );
}

#[tokio::test]
async fn a_target_that_may_not_be_written_is_refused_before_anything_happens() {
    let fixture = Fixture::create("ordering").await;
    let session_id = fixture
        .session_with_commit("NEVER.md", "never\n", "feat: never")
        .await;

    let error = fixture
        .service()
        .merge_branch(
            fixture.project.id,
            &session_id.to_string(),
            "origin/main",
            None,
            &GitActor::Session(session_id),
        )
        .await
        .expect_err("an upstream-tracking ref is not a mutation target");

    assert_eq!(error.status(), axum::http::StatusCode::BAD_REQUEST);
    assert!(
        matches!(&error, Error::Git(GitError::InvalidRef(name)) if name == "origin/main"),
        "expected the rejected name in the error, got {error:?}"
    );
    assert!(
        fixture.events(session_id).await.is_empty(),
        "a refused request records nothing"
    );
    assert!(
        fixture.app.mock_git().requested().is_empty(),
        "and reads no credential, so it writes no secret_uses row"
    );
    assert!(
        refs::resolve(
            &fixture.paths().project_repo(fixture.project.id),
            &GitRef::Session(session_id)
        )
        .await
        .is_err(),
        "and syncs nothing: the session ref was never created"
    );
}

#[tokio::test]
async fn a_session_that_is_not_this_project_s_is_a_bad_request() {
    let fixture = Fixture::create("stranger").await;
    let stranger = Uuid::new_v4();

    let error = fixture
        .service()
        .merge_branch(
            fixture.project.id,
            &stranger.to_string(),
            "main",
            None,
            &GitActor::System,
        )
        .await
        .expect_err("there is no such session in this project");

    assert!(
        matches!(&error, Error::BadRequest(message) if message == &format!("unknown session {stranger}")),
        "expected the documented bad request, got {error:?}"
    );
}

#[tokio::test]
async fn a_project_that_is_not_ready_is_refused_before_the_lock() {
    let fixture = Fixture::create("not-ready").await;
    let session_id = fixture
        .session_with_commit("EARLY.md", "early\n", "feat: early")
        .await;

    let mut tx = fixture
        .app
        .pool
        .begin()
        .await
        .expect("a transaction begins");
    ProjectRepository::new(&fixture.app.pool)
        .set_status(&mut tx, fixture.project.id, ProjectStatus::Error, None)
        .await
        .expect("the status is set");
    tx.commit().await.expect("the transaction commits");

    let error = fixture
        .service()
        .sync_session(fixture.project.id, session_id, &GitActor::System)
        .await
        .expect_err("a project in error is not operated on");

    assert!(
        matches!(&error, Error::Conflict(message) if message == "project is not ready"),
        "expected the documented conflict, got {error:?}"
    );
    assert!(fixture.events(session_id).await.is_empty());
}
