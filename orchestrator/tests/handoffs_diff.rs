//! `GET /api/projects/{pid}/git/diff?handoff_id=&base=` end to end, over
//! hand-offs published through the API (`SPEC.md`, "Git" and "Frontend" →
//! "Hand-off controls"; `ARCHITECTURE.md`, "Git model" → Diff).
//!
//! `tests/git_routes.rs` asserts the query adapter and `tests/git_diff.rs` the
//! patch itself. What is asserted here is the part that only exists once
//! `task_handoffs` does: that the selector is looked up in the URL project's
//! own hand-off rows before any ref is resolved, and that the revision the
//! diff shows is the pinned one and stays pinned however the source branch
//! moves afterwards.
//!
//! Every hand-off is published through `PUT /api/projects/{pid}/tasks/{id}`
//! rather than seeded, so the retained ref, the row and the commit really came
//! from the endpoint the revision view is paired with. Git is never mocked
//! (`CLAUDE.md`, "Testing expectations"): `common::handoffs::Fixture` builds a
//! real bare upstream, a real project repository and real session work clones.
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use axum::http::StatusCode;
use common::AuthenticatedUser;
use common::handoffs::Fixture;
use mars_orchestrator::events::TaskActor;
use mars_orchestrator::git::testutil::run_git;
use mars_orchestrator::git::{GitRef, refs};
use mars_orchestrator::models::{
    Diff, NewAgentProfile, NewProject, NewTask, NewTaskComment, NewTaskHandoff, SessionState,
    StateChange,
};
use mars_orchestrator::repositories::{ProjectRepository, SessionRepository, TaskRepository};
use mars_orchestrator::tracker::{TaskDto, TrackerMutation};
use serde_json::{Value, json};
use uuid::Uuid;

/// Not a real remote: the fixture value the other project stores (rule 3).
const OTHER_REMOTE: &str = "https://git.example.com/fake/other.git";

/// An obviously fake but well-formed object id, for the hand-off row that is
/// never resolved because its project loses the scope check first (rule 3).
const FAKE_COMMIT: &str = "89abcdef89abcdef89abcdef89abcdef89abcdef";

/// The file `Fixture::session_with_commit` writes: the first revision's only
/// change.
const FIRST_FILE: &str = "NOTES.md";

/// The file the session adds after the hand-off was published.
const SECOND_FILE: &str = "LATER.md";

// ---- helpers ----

impl Fixture {
    /// `/api/projects/{this project}/git/diff?{query}`.
    fn diff_path(&self, query: &str) -> String {
        format!("/api/projects/{}/git/diff?{query}", self.project.id)
    }

    /// Publish a revision hand-off through `PUT .../tasks/{id}` and answer the
    /// hand-off's id, as the task detail's publish control does.
    async fn publish_revision(
        &self,
        user: &AuthenticatedUser,
        task_id: Uuid,
        session_id: Uuid,
        commit: &str,
    ) -> Uuid {
        let response = self
            .app
            .put_as(
                user,
                &format!("/api/projects/{}/tasks/{task_id}", self.project.id),
            )
            .json(&json!({
                "state": "review",
                "handoff": {
                    "kind": "revision",
                    "source_session_id": session_id,
                    "commit": commit,
                    "comment": "ready for review",
                },
            }))
            .await;

        response.assert_status(StatusCode::OK);
        response
            .json::<TaskDto>()
            .handoff
            .expect("the published task carries its hand-off")
            .id
    }

    /// The diff the revision view would render, or the failure it would show.
    async fn diff(&self, user: &AuthenticatedUser, query: &str) -> Diff {
        let response = self.app.get_as(user, &self.diff_path(query)).await;
        response.assert_status(StatusCode::OK);
        response.json::<Diff>()
    }

    /// The changed paths of a diff, in git's order.
    async fn diff_files(&self, user: &AuthenticatedUser, query: &str) -> Vec<String> {
        self.diff(user, query)
            .await
            .files
            .into_iter()
            .map(|file| file.path)
            .collect()
    }

    /// The full object id an API ref name resolves to in the project
    /// repository.
    async fn commit_of(&self, name: &str) -> String {
        refs::resolve(
            &self.paths().project_repo(self.project.id),
            &GitRef::parse(name).expect("the ref name parses"),
        )
        .await
        .expect("the ref resolves")
        .commit
    }

    /// Every `git` event recorded on a session.
    async fn git_events(&self, session_id: Uuid) -> usize {
        SessionRepository::new(&self.app.pool)
            .list_events_after(session_id, 0)
            .await
            .expect("the events read")
            .into_iter()
            .filter(|row| row.kind == "git")
            .count()
    }

    /// Make a session syncable: `POST /sessions/{id}/sync` is 409 while a
    /// session is still `creating`, and the fixture's sessions are.
    async fn make_running(&self, session_id: Uuid) {
        let mut tx = self.app.pool.begin().await.expect("a transaction begins");
        SessionRepository::new(&self.app.pool)
            .set_state(
                &mut tx,
                session_id,
                SessionState::Running,
                &StateChange::plain(),
            )
            .await
            .expect("the session starts running");
        tx.commit().await.expect("the transaction commits");
    }

    /// Fetch the session's branch back through the documented route, as the
    /// session panel's "sync" control does.
    async fn sync_over_http(&self, user: &AuthenticatedUser, session_id: Uuid) {
        let response = self
            .app
            .post_as(user, &format!("/api/sessions/{session_id}/sync"))
            .await;
        response.assert_status(StatusCode::OK);
    }

    /// A hand-off row on a task of *another* project, for the scope refusal.
    ///
    /// Seeded rather than published: the point is an id that exists in
    /// `task_handoffs` and is not this project's, and the other project never
    /// gets as far as its repository.
    async fn handoff_in_another_project(&self) -> Uuid {
        let projects = ProjectRepository::new(&self.app.pool);
        let repository = TaskRepository::new(&self.app.pool);

        let new_project = NewProject::new(
            &format!("other-{}", &Uuid::new_v4().simple().to_string()[..8]),
            OTHER_REMOTE,
        )
        .expect("the project is valid");

        let mut tx = self.app.pool.begin().await.expect("a transaction begins");
        let other = projects
            .insert(&mut tx, &new_project)
            .await
            .expect("the project inserts");
        let profile = NewAgentProfile::new(other.id, "default", "localhost/mars-session:test")
            .expect("the profile is valid");
        projects
            .insert_profile(&mut tx, &profile)
            .await
            .expect("the profile inserts");
        tx.commit().await.expect("the transaction commits");

        let mut mutation = TrackerMutation::begin(&self.app.pool, other.id, TaskActor::System)
            .await
            .expect("the mutation opens");
        repository
            .insert_default_states(mutation.conn(), other.id)
            .await
            .expect("the default states insert");
        mutation.commit().await.expect("the mutation commits");

        let state = repository
            .find_state_by_name(other.id, "review")
            .await
            .expect("the state reads")
            .expect("the other project has this state");

        let mut new_task = NewTask::new(other.id, "the other project's work").expect("it parses");
        new_task.state_id = Some(state.id);

        let mut mutation = TrackerMutation::begin(&self.app.pool, other.id, TaskActor::System)
            .await
            .expect("the mutation opens");
        let task = repository
            .insert_task(mutation.conn(), other.id, &new_task)
            .await
            .expect("the task inserts");
        let comment = NewTaskComment::from_user(task.id, self.user.id, "their revision");
        let comment = repository
            .insert_comment(mutation.conn(), other.id, &comment)
            .await
            .expect("the comment inserts");
        let mut handoff = NewTaskHandoff::new(
            task.id,
            format!("refs/sessions/{}", Uuid::new_v4()),
            FAKE_COMMIT,
            comment.id,
        );
        handoff.created_by_user_id = Some(self.user.id);
        let handoff = repository
            .insert_handoff(mutation.conn(), other.id, &handoff)
            .await
            .expect("the hand-off inserts");
        mutation.commit().await.expect("the mutation commits");

        handoff.id
    }
}

/// A project with one task whose first revision is published: the arrangement
/// every case below starts from.
///
/// Answers the session that produced it, the hand-off's id and the commit it
/// pins.
async fn published(name: &str) -> (Fixture, AuthenticatedUser, Uuid, Uuid, String) {
    let fixture = Fixture::create(name).await;
    let user = fixture.signed_in();
    let (session_id, commit) = fixture
        .session_with_commit("feat: the first revision")
        .await;
    let task = fixture.task("implement it", "ready").await;
    let handoff_id = fixture
        .publish_revision(&user, task.id, session_id, &commit)
        .await;

    (fixture, user, session_id, handoff_id, commit)
}

/// Assert the documented error body (`SPEC.md`, "REST API").
fn assert_error(body: &Value, status: StatusCode) {
    assert_eq!(body["status"], status.as_u16());
    assert!(
        body["error"].as_str().is_some_and(|text| !text.is_empty()),
        "{body}"
    );
}

// ---- the pinned revision ----

#[tokio::test]
async fn a_handoff_diff_shows_the_pinned_revision_and_names_the_handoff_as_its_head() {
    let (fixture, user, _, handoff_id, commit) = published("diff-pinned").await;

    let diff = fixture
        .diff(&user, &format!("handoff_id={handoff_id}"))
        .await;

    // The `GitRef::Handoff` API name is the id itself (`SPEC.md`, "Git").
    assert_eq!(diff.head, handoff_id.to_string());
    assert_eq!(diff.base, "main", "base defaults to the default branch");
    assert_eq!(
        diff.merge_base,
        fixture.commit_of("main").await,
        "the session started from main, so main is the merge base of the pinned commit",
    );
    assert_eq!(
        diff.files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>(),
        vec![FIRST_FILE],
    );
    assert!(diff.patch.contains(FIRST_FILE), "{}", diff.patch);
    assert!(!diff.truncated);

    // The ref really is the one the publication retained.
    assert_eq!(
        fixture
            .commit_of(&format!("refs/handoffs/{handoff_id}"))
            .await,
        commit,
        "the diff was taken against the commit the hand-off pinned",
    );
}

#[tokio::test]
async fn a_handoff_diff_stays_on_the_pinned_revision_after_the_branch_advanced() {
    let (fixture, user, session_id, handoff_id, _) = published("diff-advanced").await;

    // The agent keeps working and the branch is fetched back, the way the
    // session panel's sync does it.
    fixture
        .commit_in_work_clone(session_id, SECOND_FILE, "the second revision")
        .await;
    fixture.make_running(session_id).await;
    fixture.sync_over_http(&user, session_id).await;

    assert_eq!(
        fixture
            .diff_files(&user, &format!("handoff_id={handoff_id}"))
            .await,
        vec![FIRST_FILE.to_string()],
        "the retained commit does not move with the branch",
    );
    let mut on_the_branch = fixture
        .diff_files(&user, &format!("head={session_id}"))
        .await;
    on_the_branch.sort();
    assert_eq!(
        on_the_branch,
        vec![SECOND_FILE.to_string(), FIRST_FILE.to_string()],
        "the session head shows the new tip",
    );
}

#[tokio::test]
async fn a_handoff_diff_syncs_nothing_and_records_no_git_event() {
    let (fixture, user, session_id, handoff_id, _) = published("diff-silent").await;

    // A commit left in the work clone and deliberately *not* fetched back: if
    // the hand-off diff synced anything, this file would appear in it.
    fixture
        .commit_in_work_clone(session_id, SECOND_FILE, "unsynced work")
        .await;
    let before = fixture.git_events(session_id).await;

    assert_eq!(
        fixture
            .diff_files(&user, &format!("handoff_id={handoff_id}"))
            .await,
        vec![FIRST_FILE.to_string()],
    );
    assert_eq!(
        fixture.git_events(session_id).await,
        before,
        "a hand-off diff is read-only and silent (ARCHITECTURE.md, Git model, Diff)",
    );
}

#[tokio::test]
async fn a_handoff_diff_survives_the_loss_of_the_source_work_clone() {
    let (fixture, user, session_id, handoff_id, _) = published("diff-no-clone").await;

    // A hand-off stays reviewable after its session's clone is gone, which is
    // what "never syncs a moving branch" has to survive.
    std::fs::remove_dir_all(fixture.paths().session_work(session_id))
        .expect("the work clone is removed");

    assert_eq!(
        fixture
            .diff_files(&user, &format!("handoff_id={handoff_id}"))
            .await,
        vec![FIRST_FILE.to_string()],
    );
}

// ---- refusals ----

#[tokio::test]
async fn a_handoff_of_another_project_is_404() {
    let (fixture, user, _, _, _) = published("diff-foreign").await;
    let foreign = fixture.handoff_in_another_project().await;

    // The row exists; it is simply not this project's, and the scope comes
    // through the task it belongs to (`SPEC.md`, "Git").
    let response = fixture
        .app
        .get_as(&user, &fixture.diff_path(&format!("handoff_id={foreign}")))
        .await;

    response.assert_status(StatusCode::NOT_FOUND);
    assert_error(&response.json::<Value>(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn an_unknown_handoff_id_is_404() {
    let (fixture, user, _, _, _) = published("diff-unknown").await;

    let response = fixture
        .app
        .get_as(
            &user,
            &fixture.diff_path(&format!("handoff_id={}", Uuid::new_v4())),
        )
        .await;

    response.assert_status(StatusCode::NOT_FOUND);
    assert_error(&response.json::<Value>(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn head_and_handoff_id_together_is_400() {
    let (fixture, user, session_id, handoff_id, _) = published("diff-both").await;

    let response = fixture
        .app
        .get_as(
            &user,
            &fixture.diff_path(&format!("head={session_id}&handoff_id={handoff_id}")),
        )
        .await;

    response.assert_status(StatusCode::BAD_REQUEST);
    assert_eq!(
        response.json::<Value>()["error"],
        "diff takes either head or handoff_id",
    );
}

#[tokio::test]
async fn a_base_naming_a_session_is_400() {
    let (fixture, user, session_id, handoff_id, _) = published("diff-base-kind").await;

    // `is_base()` is integration heads and upstream-tracking refs only
    // (`SPEC.md`, "Git"), and the base is checked whichever head was selected.
    let response = fixture
        .app
        .get_as(
            &user,
            &fixture.diff_path(&format!("handoff_id={handoff_id}&base={session_id}")),
        )
        .await;

    response.assert_status(StatusCode::BAD_REQUEST);
    assert_error(&response.json::<Value>(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_handoff_whose_ref_is_gone_is_400() {
    let (fixture, user, _, handoff_id, _) = published("diff-torn-ref").await;

    // Only reachable mid-deletion: the row is removed under the project git
    // lock together with its ref, so a row without a ref is the invariant's
    // boundary rather than a state the API can be left in. The scope check
    // passes and the resolution is what fails.
    run_git(
        &fixture.paths().project_repo(fixture.project.id),
        &["update-ref", "-d", &format!("refs/handoffs/{handoff_id}")],
    )
    .await;

    let response = fixture
        .app
        .get_as(
            &user,
            &fixture.diff_path(&format!("handoff_id={handoff_id}")),
        )
        .await;

    response.assert_status(StatusCode::BAD_REQUEST);
    assert_eq!(
        response.json::<Value>()["error"],
        format!("no such ref in this repository: {handoff_id}"),
    );
}
