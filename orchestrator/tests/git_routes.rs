//! `/api/projects/{pid}/git` through the real router (`SPEC.md`, "Git
//! (`/api/projects/{pid}/git`)").
//!
//! The operations themselves — locking, fetch-back, ref resolution, outcome
//! events — are asserted against the service in `tests/git_service.rs`, and
//! the primitives in `tests/git_merge.rs`, `git_rebase.rs`, `git_push.rs` and
//! `git_diff.rs`. What is asserted here is the adapter: the five paths, the
//! JWT requirement on each of them, the status of every success and of every
//! documented failure — 400 for a ref of the wrong kind or a body that selects
//! neither or both of a pair, 404 for an unknown project, 409 for a project
//! that is not `ready`, for a non-fast-forward push and for a task hand-off no
//! verifier can approve, 422 with `conflicts` for a merge or rebase conflict —
//! and the exact response shapes.
//!
//! Every failure case also asserts that `refs/heads/main` in the project's own
//! repository is exactly what it was, because "a bad request therefore has no
//! side effects at all" is the property the ordering in [`GitService`] exists
//! for.
//!
//! Git is never mocked (`CLAUDE.md`, "Testing expectations"): each fixture
//! builds a real upstream, a real project repository and real session work
//! clones under the `TestApp`'s own `DATA_DIR`, exactly as
//! `tests/git_service.rs` does. Only the credential provider is a mock and
//! every value it hands out is obviously fake (rule 3).
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use axum::http::StatusCode;
use common::{AuthenticatedUser, TestApp};
use mars_orchestrator::git::testutil::{TestUpstream, run_git, test_identity};
use mars_orchestrator::git::{
    DataPaths, GitRef, create_work_clone, init_project_repo, refs, resolve_base,
};
use mars_orchestrator::models::{
    BranchName, NewAgentProfile, NewProject, NewSession, ProfileKind, Project, ProjectStatus,
    RemoteUrl,
};
use mars_orchestrator::repositories::{ProjectRepository, SessionRepository};
use serde_json::{Value, json};
use uuid::Uuid;

/// Not a real remote: the fixture value every project row stores. The
/// repository the tests actually work against is the temporary upstream, which
/// `init_project_repo` is given through [`RemoteUrl::local_for_tests`] — a
/// stored `remote_url` has to be an `https` URL, and a path is not one (rule
/// 3).
const TEST_REMOTE: &str = "https://git.example.com/fake/repo.git";

/// The documented 401 body (`SPEC.md`, "Authentication").
fn unauthorized() -> Value {
    json!({ "status": 401, "error": "authentication required" })
}

/// An obviously fake password of the length `POST /api/test/users` requires
/// (rule 3).
fn password(name: &str) -> String {
    format!("fake-password-{name}")
}

/// `/api/projects/{project}/git/{op}`.
fn git_path(project: Uuid, op: &str) -> String {
    format!("/api/projects/{project}/git/{op}")
}

/// Every git path, for the tests that assert the same answer on all five.
fn every_path(project: Uuid) -> [String; 5] {
    [
        git_path(project, "session-branches"),
        git_path(project, "diff?head=main"),
        git_path(project, "merge"),
        git_path(project, "rebase"),
        git_path(project, "push"),
    ]
}

/// A ready project with a real repository, an upstream behind it, and a
/// signed-in user to make requests as.
struct Fixture {
    app: TestApp,
    upstream: TestUpstream,
    project: Project,
    user: AuthenticatedUser,
    profile_id: Uuid,
}

impl Fixture {
    async fn create(name: &str) -> Self {
        let app = TestApp::spawn().await;
        let upstream = TestUpstream::create().await;
        let project = insert_project(&app, name).await;

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
        let project = ProjectRepository::new(&app.pool)
            .set_status(&mut tx, project.id, ProjectStatus::Ready, None)
            .await
            .expect("the status is set")
            .expect("the project exists");
        tx.commit().await.expect("the transaction commits");

        let user = app
            .create_user("ada", "ada@example.test", &password("ada"))
            .await;

        let profile = NewAgentProfile::new(project.id, "default", "localhost/mars-session:test")
            .expect("the test profile is valid");
        let mut tx = app.pool.begin().await.expect("a transaction begins");
        let profile = ProjectRepository::new(&app.pool)
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

    fn paths(&self) -> DataPaths {
        DataPaths::from_config(&self.app.state.config)
    }

    /// `/api/projects/{this project}/git/{op}`.
    fn path(&self, op: &str) -> String {
        git_path(self.project.id, op)
    }

    /// A session row with a work clone holding one commit, not fetched back:
    /// the state the routes find a working session in.
    async fn session_with_commit(&self, path: &str, content: &str, message: &str) -> Uuid {
        let mut session = NewSession::new(
            self.project.id,
            self.profile_id,
            ProfileKind::Conversational,
            "main",
            format!("fake-mcp-token-hash-{}", Uuid::new_v4()),
        );
        session.created_by = Some(self.user.user.id);

        let mut tx = self.app.pool.begin().await.expect("a transaction begins");
        let session_id = SessionRepository::new(&self.app.pool)
            .insert(&mut tx, &session)
            .await
            .expect("the session inserts")
            .id;
        tx.commit().await.expect("the transaction commits");

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

    /// The commit a ref in the project repository points at, by API name.
    async fn commit_of(&self, name: &str) -> String {
        let git_ref = GitRef::parse(name).expect("a parsable ref name");

        refs::resolve(&self.paths().project_repo(self.project.id), &git_ref)
            .await
            .unwrap_or_else(|err| panic!("{name} resolves: {err}"))
            .commit
    }
}

/// A project row in the status the database gives a fresh one, `cloning`.
async fn insert_project(app: &TestApp, name: &str) -> Project {
    let mut new_project = NewProject::new(name, TEST_REMOTE).expect("the test project is valid");
    new_project.default_branch = Some(BranchName::parse("main").expect("main is a branch name"));

    let mut tx = app.pool.begin().await.expect("a transaction begins");
    let project = ProjectRepository::new(&app.pool)
        .insert(&mut tx, &new_project)
        .await
        .expect("the project inserts");
    tx.commit().await.expect("the transaction commits");

    project
}

/// Assert an error body is the documented `{ status, error }` with this code.
#[track_caller]
fn assert_error_body(body: &Value, status: StatusCode) {
    assert_eq!(body["status"], status.as_u16(), "unexpected body: {body}");
    assert!(
        body["error"].as_str().is_some_and(|text| !text.is_empty()),
        "an error body always names the failure: {body}"
    );
}

// ---- authentication ----

#[tokio::test]
async fn every_git_endpoint_requires_a_token() {
    let fixture = Fixture::create("auth").await;

    for path in every_path(fixture.project.id) {
        let response = if path.contains("session-branches") || path.contains("diff") {
            fixture.app.server.get(&path).await
        } else {
            fixture.app.server.post(&path).json(&json!({})).await
        };

        response.assert_status(StatusCode::UNAUTHORIZED);
        assert_eq!(response.json::<Value>(), unauthorized(), "{path}");
    }
}

// ---- project scope ----

#[tokio::test]
async fn an_unknown_project_is_404_on_every_endpoint() {
    let app = TestApp::spawn().await;
    let user = app
        .create_user("ada", "ada@example.test", &password("ada"))
        .await;
    let unknown = Uuid::new_v4();

    for path in every_path(unknown) {
        let response = if path.contains("session-branches") || path.contains("diff") {
            app.get_as(&user, &path).await
        } else if path.contains("merge") {
            app.post_as(&user, &path)
                .json(&json!({ "target": "main", "source": "main" }))
                .await
        } else if path.contains("rebase") {
            app.post_as(&user, &path)
                .json(&json!({ "branch": "main", "onto": "origin/main" }))
                .await
        } else {
            app.post_as(&user, &path)
                .json(&json!({ "ref": "main" }))
                .await
        };

        response.assert_status(StatusCode::NOT_FOUND);
        assert_error_body(&response.json::<Value>(), StatusCode::NOT_FOUND);
    }
}

#[tokio::test]
async fn a_project_that_is_not_ready_is_409_on_every_endpoint() {
    let app = TestApp::spawn().await;
    let user = app
        .create_user("ada", "ada@example.test", &password("ada"))
        .await;
    // Inserted and left alone: a fresh row is `cloning`, which is exactly the
    // state that has no repository to work in yet.
    let project = insert_project(&app, "still-cloning").await;

    for path in every_path(project.id) {
        let response = if path.contains("session-branches") || path.contains("diff") {
            app.get_as(&user, &path).await
        } else if path.contains("merge") {
            app.post_as(&user, &path)
                .json(&json!({ "target": "main", "source": "main" }))
                .await
        } else if path.contains("rebase") {
            app.post_as(&user, &path)
                .json(&json!({ "branch": "main", "onto": "origin/main" }))
                .await
        } else {
            app.post_as(&user, &path)
                .json(&json!({ "ref": "main" }))
                .await
        };

        response.assert_status(StatusCode::CONFLICT);
        let body = response.json::<Value>();
        assert_error_body(&body, StatusCode::CONFLICT);
        assert_eq!(body["error"], "project is not ready", "{path}");
    }
}

// ---- session branches ----

#[tokio::test]
async fn session_branches_answers_the_documented_shape() {
    let fixture = Fixture::create("branches").await;
    let session_id = fixture
        .session_with_commit("NOTES.md", "work\n", "docs: notes")
        .await;

    // A branch is listed once it has been synced, which the merge below does;
    // before that the project repository has no `refs/sessions/<id>` at all.
    let empty = fixture
        .app
        .get_as(&fixture.user, &fixture.path("session-branches"))
        .await;
    empty.assert_status_ok();
    assert_eq!(empty.json::<Vec<Value>>(), Vec::<Value>::new());

    fixture
        .app
        .post_as(&fixture.user, &fixture.path("merge"))
        .json(&json!({ "target": "main", "source": session_id.to_string() }))
        .await
        .assert_status_ok();

    let response = fixture
        .app
        .get_as(&fixture.user, &fixture.path("session-branches"))
        .await;

    response.assert_status_ok();
    let branches = response.json::<Vec<Value>>();
    assert_eq!(branches.len(), 1, "one session, one branch: {branches:?}");

    let branch = &branches[0];
    let mut keys: Vec<&str> = branch
        .as_object()
        .expect("a branch is an object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "ahead",
            "base",
            "behind",
            "commit",
            "ref",
            "session_id",
            "updated_at"
        ],
        "unexpected SessionBranch fields"
    );
    assert_eq!(branch["session_id"], session_id.to_string());
    assert_eq!(branch["ref"], format!("refs/sessions/{session_id}"));
    assert_eq!(branch["base"], "main");
    assert_eq!(
        branch["commit"],
        fixture.commit_of(&session_id.to_string()).await
    );
}

// ---- diff ----

#[tokio::test]
async fn a_diff_answers_the_patch_from_the_merge_base() {
    let fixture = Fixture::create("diff").await;
    let session_id = fixture
        .session_with_commit("CHANGED.md", "changed\n", "feat: change")
        .await;

    let response = fixture
        .app
        .get_as(
            &fixture.user,
            &fixture.path(&format!("diff?head={session_id}")),
        )
        .await;

    response.assert_status_ok();
    let diff = response.json::<Value>();
    assert_eq!(diff["head"], session_id.to_string());
    assert_eq!(diff["base"], "main", "base defaults to the default branch");
    assert_eq!(
        diff["merge_base"],
        fixture.commit_of("main").await,
        "the session started from main, so main is the merge base"
    );
    assert_eq!(diff["files"][0]["path"], "CHANGED.md");
    // The one-letter status the panel renders as a badge (`crate::models`,
    // `DiffStatus`): the file did not exist at the merge base.
    assert_eq!(diff["files"][0]["status"], "A");
    assert_eq!(diff["truncated"], false);
    assert!(
        diff["patch"]
            .as_str()
            .expect("the patch is a string")
            .contains("CHANGED.md"),
        "{diff}"
    );
}

#[tokio::test]
async fn a_diff_takes_exactly_one_selector() {
    let fixture = Fixture::create("diff-selector").await;
    let handoff_id = Uuid::new_v4();

    for query in [
        "diff".to_string(),
        format!("diff?head=main&handoff_id={handoff_id}"),
    ] {
        let response = fixture
            .app
            .get_as(&fixture.user, &fixture.path(&query))
            .await;

        response.assert_status(StatusCode::BAD_REQUEST);
        let body = response.json::<Value>();
        assert_error_body(&body, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "diff takes either head or handoff_id");
    }
}

#[tokio::test]
async fn a_diff_of_a_handoff_this_project_does_not_have_is_404() {
    let fixture = Fixture::create("diff-handoff").await;
    let handoff_id = Uuid::new_v4();

    // The selector is looked up in `task_handoffs` scoped to the URL project
    // before any ref is resolved, so an id this project does not have is a
    // missing resource rather than an unresolvable ref (`SPEC.md`, "Git").
    // `tests/handoffs_diff.rs` covers the same answer for an id that exists in
    // another project.
    let response = fixture
        .app
        .get_as(
            &fixture.user,
            &fixture.path(&format!("diff?handoff_id={handoff_id}")),
        )
        .await;

    response.assert_status(StatusCode::NOT_FOUND);
    assert_error_body(&response.json::<Value>(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_diff_base_of_the_wrong_kind_is_400() {
    let fixture = Fixture::create("diff-base").await;
    let session_id = fixture
        .session_with_commit("BASE.md", "base\n", "feat: base")
        .await;

    // A session ref is a head, never a base (`SPEC.md`, "Git").
    let response = fixture
        .app
        .get_as(
            &fixture.user,
            &fixture.path(&format!("diff?head=main&base={session_id}")),
        )
        .await;

    response.assert_status(StatusCode::BAD_REQUEST);
    assert_error_body(&response.json::<Value>(), StatusCode::BAD_REQUEST);
}

// ---- merge ----

#[tokio::test]
async fn a_merge_moves_the_target_and_answers_its_commit() {
    let fixture = Fixture::create("merge").await;
    let session_id = fixture
        .session_with_commit("FEATURE.md", "feature\n", "feat: a feature")
        .await;

    let response = fixture
        .app
        .post_as(&fixture.user, &fixture.path("merge"))
        .json(&json!({
            "target": "main",
            "source": session_id.to_string(),
            "message": "merge: the feature",
        }))
        .await;

    response.assert_status_ok();
    let body = response.json::<Value>();
    assert_eq!(
        body.as_object().expect("an object").keys().count(),
        1,
        "the merge body is exactly {{commit}}: {body}"
    );
    assert_eq!(body["commit"], fixture.commit_of("main").await);
}

#[tokio::test]
async fn two_merges_to_the_same_target_each_see_the_one_before() {
    let fixture = Fixture::create("merge-twice").await;
    let first = fixture
        .session_with_commit("ONE.md", "one\n", "feat: one")
        .await;
    let second = fixture
        .session_with_commit("TWO.md", "two\n", "feat: two")
        .await;

    let mut commits = Vec::new();
    for session_id in [first, second] {
        let response = fixture
            .app
            .post_as(&fixture.user, &fixture.path("merge"))
            .json(&json!({ "target": "main", "source": session_id.to_string() }))
            .await;

        response.assert_status_ok();
        commits.push(response.json::<Value>()["commit"].clone());
    }

    assert_ne!(commits[0], commits[1], "the target moved twice");
    assert_eq!(commits[1], fixture.commit_of("main").await);
}

#[tokio::test]
async fn a_conflicting_merge_is_422_with_the_conflicting_paths() {
    let fixture = Fixture::create("merge-conflict").await;
    let first = fixture
        .session_with_commit("SHARED.md", "one\n", "feat: one")
        .await;
    let second = fixture
        .session_with_commit("SHARED.md", "two\n", "feat: two")
        .await;

    fixture
        .app
        .post_as(&fixture.user, &fixture.path("merge"))
        .json(&json!({ "target": "main", "source": first.to_string() }))
        .await
        .assert_status_ok();
    let main_before = fixture.commit_of("main").await;

    let response = fixture
        .app
        .post_as(&fixture.user, &fixture.path("merge"))
        .json(&json!({ "target": "main", "source": second.to_string() }))
        .await;

    response.assert_status(StatusCode::UNPROCESSABLE_ENTITY);
    let body = response.json::<Value>();
    assert_error_body(&body, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        body["conflicts"],
        json!(["SHARED.md"]),
        "a 422 always carries at least one conflicting path: {body}"
    );
    assert_eq!(
        fixture.commit_of("main").await,
        main_before,
        "a conflict leaves the target exactly as it was"
    );
}

#[tokio::test]
async fn a_merge_target_that_is_not_an_integration_head_is_400() {
    let fixture = Fixture::create("merge-target").await;
    let main_before = fixture.commit_of("main").await;

    for target in ["origin/main", "refs/remotes/origin/main"] {
        let response = fixture
            .app
            .post_as(&fixture.user, &fixture.path("merge"))
            .json(&json!({ "target": target, "source": "main" }))
            .await;

        response.assert_status(StatusCode::BAD_REQUEST);
        assert_error_body(&response.json::<Value>(), StatusCode::BAD_REQUEST);
    }

    assert_eq!(
        fixture.commit_of("main").await,
        main_before,
        "a refused request wrote nothing"
    );
}

#[tokio::test]
async fn a_merge_source_naming_another_project_s_session_is_400() {
    let fixture = Fixture::create("merge-scope").await;
    let stranger = Uuid::new_v4();
    let main_before = fixture.commit_of("main").await;

    let response = fixture
        .app
        .post_as(&fixture.user, &fixture.path("merge"))
        .json(&json!({ "target": "main", "source": stranger.to_string() }))
        .await;

    response.assert_status(StatusCode::BAD_REQUEST);
    assert_error_body(&response.json::<Value>(), StatusCode::BAD_REQUEST);
    assert_eq!(fixture.commit_of("main").await, main_before);
}

#[tokio::test]
async fn a_merge_takes_either_a_source_or_a_task_hand_off() {
    let fixture = Fixture::create("merge-form").await;
    let main_before = fixture.commit_of("main").await;

    let bodies = [
        json!({ "target": "main" }),
        json!({ "target": "main", "source": "main", "task_id": Uuid::new_v4() }),
        json!({ "target": "main", "task_id": Uuid::new_v4() }),
        json!({ "target": "main", "handoff_id": Uuid::new_v4() }),
        json!({
            "target": "main",
            "source": "main",
            "task_id": Uuid::new_v4(),
            "handoff_id": Uuid::new_v4(),
        }),
    ];

    for body in bodies {
        let response = fixture
            .app
            .post_as(&fixture.user, &fixture.path("merge"))
            .json(&body)
            .await;

        response.assert_status(StatusCode::BAD_REQUEST);
        let answered = response.json::<Value>();
        assert_error_body(&answered, StatusCode::BAD_REQUEST);
        assert_eq!(
            answered["error"], "merge takes either source or task_id and handoff_id",
            "{body}"
        );
    }

    assert_eq!(fixture.commit_of("main").await, main_before);
}

#[tokio::test]
async fn a_task_merge_is_409_while_no_hand_off_verifier_is_installed() {
    let fixture = Fixture::create("merge-task").await;
    let main_before = fixture.commit_of("main").await;

    // The `NoHandoffs` default: until "Code hand-offs and review" installs a
    // verifier, no hand-off can be shown to be current and approved, so the
    // task form answers the documented conflict rather than merging.
    let response = fixture
        .app
        .post_as(&fixture.user, &fixture.path("merge"))
        .json(&json!({
            "target": "main",
            "task_id": Uuid::new_v4(),
            "handoff_id": Uuid::new_v4(),
        }))
        .await;

    response.assert_status(StatusCode::CONFLICT);
    let body = response.json::<Value>();
    assert_error_body(&body, StatusCode::CONFLICT);
    assert_eq!(body["error"], "task hand-offs are not available");
    assert_eq!(fixture.commit_of("main").await, main_before);
}

#[tokio::test]
async fn a_task_merge_with_a_target_of_the_wrong_kind_is_400_before_the_hand_off_is_looked_at() {
    let fixture = Fixture::create("merge-task-target").await;

    let response = fixture
        .app
        .post_as(&fixture.user, &fixture.path("merge"))
        .json(&json!({
            "target": "origin/main",
            "task_id": Uuid::new_v4(),
            "handoff_id": Uuid::new_v4(),
        }))
        .await;

    response.assert_status(StatusCode::BAD_REQUEST);
    assert_error_body(&response.json::<Value>(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_merge_message_over_ten_kibibytes_is_400() {
    let fixture = Fixture::create("merge-message").await;
    let session_id = fixture
        .session_with_commit("LONG.md", "long\n", "feat: long")
        .await;
    let main_before = fixture.commit_of("main").await;

    let response = fixture
        .app
        .post_as(&fixture.user, &fixture.path("merge"))
        .json(&json!({
            "target": "main",
            "source": session_id.to_string(),
            "message": "m".repeat(10 * 1024 + 1),
        }))
        .await;

    response.assert_status(StatusCode::BAD_REQUEST);
    assert_error_body(&response.json::<Value>(), StatusCode::BAD_REQUEST);
    assert_eq!(
        fixture.commit_of("main").await,
        main_before,
        "the limit is checked before anything is merged"
    );
}

#[tokio::test]
async fn a_malformed_merge_body_is_400() {
    let fixture = Fixture::create("merge-malformed").await;

    // `target` missing entirely: the body does not deserialise, and the
    // wrapper's rejection is the documented `{ status, error }` shape rather
    // than axum's plain text (`SPEC.md`, "REST API").
    let response = fixture
        .app
        .post_as(&fixture.user, &fixture.path("merge"))
        .json(&json!({ "source": "main" }))
        .await;

    response.assert_status(StatusCode::BAD_REQUEST);
    assert_error_body(&response.json::<Value>(), StatusCode::BAD_REQUEST);
}

// ---- rebase ----

#[tokio::test]
async fn a_rebase_replays_the_branch_and_answers_its_commit() {
    let fixture = Fixture::create("rebase").await;
    let ahead = fixture
        .session_with_commit("AHEAD.md", "ahead\n", "feat: ahead")
        .await;
    let branch = fixture
        .session_with_commit("BRANCH.md", "branch\n", "feat: branch")
        .await;

    fixture
        .app
        .post_as(&fixture.user, &fixture.path("merge"))
        .json(&json!({ "target": "main", "source": ahead.to_string() }))
        .await
        .assert_status_ok();

    let response = fixture
        .app
        .post_as(&fixture.user, &fixture.path("rebase"))
        .json(&json!({ "branch": branch.to_string(), "onto": "main" }))
        .await;

    response.assert_status_ok();
    let body = response.json::<Value>();
    assert_eq!(
        body.as_object().expect("an object").keys().count(),
        1,
        "the rebase body is exactly {{commit}}: {body}"
    );
    assert_eq!(body["commit"], fixture.commit_of(&branch.to_string()).await);
}

#[tokio::test]
async fn a_conflicting_rebase_is_422_with_the_conflicting_paths() {
    let fixture = Fixture::create("rebase-conflict").await;
    let ahead = fixture
        .session_with_commit("SHARED.md", "one\n", "feat: one")
        .await;
    let branch = fixture
        .session_with_commit("SHARED.md", "two\n", "feat: two")
        .await;

    fixture
        .app
        .post_as(&fixture.user, &fixture.path("merge"))
        .json(&json!({ "target": "main", "source": ahead.to_string() }))
        .await
        .assert_status_ok();
    let main_before = fixture.commit_of("main").await;

    let response = fixture
        .app
        .post_as(&fixture.user, &fixture.path("rebase"))
        .json(&json!({ "branch": branch.to_string(), "onto": "main" }))
        .await;

    response.assert_status(StatusCode::UNPROCESSABLE_ENTITY);
    let body = response.json::<Value>();
    assert_error_body(&body, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["conflicts"], json!(["SHARED.md"]), "{body}");
    assert_eq!(fixture.commit_of("main").await, main_before);
}

#[tokio::test]
async fn a_rebase_refuses_an_upstream_branch_and_a_session_onto() {
    let fixture = Fixture::create("rebase-refs").await;
    let session_id = fixture
        .session_with_commit("REF.md", "ref\n", "feat: ref")
        .await;
    let main_before = fixture.commit_of("main").await;

    let bodies = [
        json!({ "branch": "origin/main", "onto": "main" }),
        json!({ "branch": "main", "onto": session_id.to_string() }),
    ];

    for body in bodies {
        let response = fixture
            .app
            .post_as(&fixture.user, &fixture.path("rebase"))
            .json(&body)
            .await;

        response.assert_status(StatusCode::BAD_REQUEST);
        assert_error_body(&response.json::<Value>(), StatusCode::BAD_REQUEST);
    }

    assert_eq!(fixture.commit_of("main").await, main_before);
}

// ---- push ----

#[tokio::test]
async fn a_push_publishes_the_branch_upstream() {
    let fixture = Fixture::create("push").await;
    let session_id = fixture
        .session_with_commit("PUSHED.md", "pushed\n", "feat: pushed")
        .await;

    let response = fixture
        .app
        .post_as(&fixture.user, &fixture.path("push"))
        .json(&json!({ "ref": session_id.to_string() }))
        .await;

    response.assert_status_ok();
    let body = response.json::<Value>();
    let mut keys: Vec<&str> = body
        .as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        ["commit", "remote_branch"],
        "the push body is exactly the two documented fields: {body}"
    );
    assert_eq!(body["remote_branch"], format!("session/{session_id}"));
    assert_eq!(
        run_git(
            &fixture.upstream.path,
            &["rev-parse", &format!("refs/heads/session/{session_id}")],
        )
        .await
        .trim(),
        body["commit"].as_str().expect("the commit is a string"),
        "the commit reached the upstream repository"
    );
}

#[tokio::test]
async fn a_non_fast_forward_push_is_409_and_force_true_publishes_it() {
    let fixture = Fixture::create("push-rejected").await;
    let session_id = fixture
        .session_with_commit("LOCAL.md", "local\n", "feat: local")
        .await;

    // Upstream `main` moves past everything the mirror knows, so sending the
    // session's branch to it cannot be a fast-forward.
    fixture
        .upstream
        .commit_file("main", "UPSTREAM.md", "upstream\n", "docs: upstream")
        .await;
    let main_before = fixture.commit_of("main").await;

    let rejected = fixture
        .app
        .post_as(&fixture.user, &fixture.path("push"))
        .json(&json!({ "ref": session_id.to_string(), "remote_branch": "main" }))
        .await;

    rejected.assert_status(StatusCode::CONFLICT);
    assert_error_body(&rejected.json::<Value>(), StatusCode::CONFLICT);
    assert_eq!(
        fixture.commit_of("main").await,
        main_before,
        "a rejected push retains every local ref"
    );

    let forced = fixture
        .app
        .post_as(&fixture.user, &fixture.path("push"))
        .json(&json!({
            "ref": session_id.to_string(),
            "remote_branch": "main",
            "force": true,
        }))
        .await;

    forced.assert_status_ok();
    let body = forced.json::<Value>();
    assert_eq!(body["remote_branch"], "main");
    assert_eq!(
        run_git(&fixture.upstream.path, &["rev-parse", "refs/heads/main"])
            .await
            .trim(),
        body["commit"].as_str().expect("the commit is a string"),
        "only `force: true` overwrites upstream"
    );
}

#[tokio::test]
async fn a_push_refuses_an_upstream_ref_and_a_qualified_remote_branch() {
    let fixture = Fixture::create("push-refs").await;
    let upstream_head = run_git(&fixture.upstream.path, &["rev-parse", "refs/heads/main"])
        .await
        .trim()
        .to_string();

    let bodies = [
        json!({ "ref": "origin/main" }),
        json!({ "ref": "main", "remote_branch": "refs/heads/main" }),
        json!({ "ref": "main", "remote_branch": "" }),
    ];

    for body in bodies {
        let response = fixture
            .app
            .post_as(&fixture.user, &fixture.path("push"))
            .json(&body)
            .await;

        response.assert_status(StatusCode::BAD_REQUEST);
        assert_error_body(&response.json::<Value>(), StatusCode::BAD_REQUEST);
    }

    assert_eq!(
        run_git(&fixture.upstream.path, &["rev-parse", "refs/heads/main"])
            .await
            .trim(),
        upstream_head,
        "a refused push sent nothing"
    );
}
