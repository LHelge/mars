//! `POST /api/projects/{pid}/git/merge` in its task form: the approval check
//! (`SPEC.md`, "Git"; `ARCHITECTURE.md`, "Git model" → Merge, rebase, push and
//! "Task tracker" → "Review approval").
//!
//! The merge route and its branch form are `tests/git_routes.rs`'s. What is
//! asserted here is the half this epic owns — `TaskHandoffVerifier` behind
//! `GitService::merge_handoff`:
//!
//! - an approved current hand-off merges its *pinned* commit, not the source
//!   session's newer tip, and the merge commit carries the documented default
//!   message;
//! - `unreviewed` and `changes_requested` are 409 with the target untouched;
//! - a superseded hand-off id is 409 even though it was approved, and so is a
//!   hand-off of another task of the same project;
//! - a task this project does not have is 404;
//! - a conflicting pinned commit is 422 with `conflicts`, the target unmoved
//!   and the approval still standing;
//! - the branch form over the same session still merges and records no
//!   approval on the task.
//!
//! Hand-offs are published through `PUT /projects/{pid}/tasks/{id}`, the way a
//! user publishes and reviews them, so every row and ref under test was
//! written by the endpoint that owns it. Git is never mocked (`CLAUDE.md`,
//! "Testing expectations"): `common::handoffs::Fixture` builds a real bare
//! upstream, a real project repository and real session work clones.
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use axum::http::StatusCode;
use axum_test::TestResponse;
use common::AuthenticatedUser;
use common::handoffs::Fixture;
use mars_orchestrator::git::testutil::{run_git, test_identity};
use mars_orchestrator::git::{GitRef, create_work_clone, refs, resolve_base};
use mars_orchestrator::models::{ReviewStatus, Task, TaskRef};
use mars_orchestrator::repositories::{SessionRepository, TaskRepository};
use mars_orchestrator::tracker::TaskDto;
use serde_json::{Value, json};
use uuid::Uuid;

// ---- helpers ----

impl Fixture {
    /// `/api/projects/{this project}/git/merge`.
    fn merge_path(&self) -> String {
        format!("/api/projects/{}/git/merge", self.project.id)
    }

    /// `PUT` a task with a hand-off, as the signed-in user, and expect it to
    /// be published.
    async fn publish(
        &self,
        user: &AuthenticatedUser,
        task: &Task,
        state: &str,
        handoff: Value,
    ) -> TaskDto {
        let response = self
            .app
            .put_as(
                user,
                &format!("/api/projects/{}/tasks/{}", self.project.id, task.id),
            )
            .json(&json!({ "state": state, "handoff": handoff }))
            .await;

        response.assert_status(StatusCode::OK);
        response.json::<TaskDto>()
    }

    /// `POST` the merge body as the signed-in user.
    async fn merge(&self, user: &AuthenticatedUser, body: Value) -> TestResponse {
        self.app.post_as(user, &self.merge_path()).json(&body).await
    }

    /// The commit a ref points at, by API name.
    async fn commit_of(&self, name: &str) -> String {
        let git_ref = GitRef::parse(name).expect("a parsable ref name");

        refs::resolve(&self.paths().project_repo(self.project.id), &git_ref)
            .await
            .unwrap_or_else(|err| panic!("{name} resolves: {err}"))
            .commit
    }

    /// Every path in a ref's tree, which is how "the merge took A and not B"
    /// is asserted by content rather than by commit id.
    async fn files_in(&self, name: &str) -> Vec<String> {
        run_git(
            &self.paths().project_repo(self.project.id),
            &["ls-tree", "--name-only", "-r", name],
        )
        .await
        .lines()
        .map(str::to_string)
        .collect()
    }

    /// The subject of a ref's tip commit.
    async fn subject_of(&self, name: &str) -> String {
        run_git(
            &self.paths().project_repo(self.project.id),
            &["log", "-1", "--format=%s", name],
        )
        .await
        .trim()
        .to_string()
    }

    /// The task as it now stands.
    async fn reload(&self, task_id: Uuid) -> Task {
        TaskRepository::new(&self.app.pool)
            .find_task(self.project.id, TaskRef::Id(task_id))
            .await
            .expect("the task reads")
            .expect("the task exists")
    }

    /// The current hand-off's review status.
    async fn review_status(&self, task_id: Uuid) -> ReviewStatus {
        let handoff_id = self
            .reload(task_id)
            .await
            .current_handoff_id
            .expect("the task has a current hand-off");

        TaskRepository::new(&self.app.pool)
            .find_handoff(self.project.id, handoff_id)
            .await
            .expect("the hand-off reads")
            .expect("the hand-off exists")
            .review_status
    }

    /// The `git` events a session was told about, newest last.
    async fn git_events(&self, session_id: Uuid) -> Vec<Value> {
        SessionRepository::new(&self.app.pool)
            .list_events(session_id, None, 100)
            .await
            .expect("the events read")
            .0
            .into_iter()
            .filter(|row| row.kind == "git")
            .map(|row| row.payload)
            .collect()
    }
}

/// A session with a work clone holding one commit to `file`, not fetched
/// back.
///
/// `Fixture::session_with_commit` always writes `NOTES.md`, which is what the
/// conflict case wants and what a second, *independent* line of work must not
/// have; this is that second line.
async fn session_with_file(fixture: &Fixture, file: &str, content: &str) -> (Uuid, String) {
    let session_id = fixture.seed_session().await;
    {
        let guard = fixture.guard().await;
        let paths = fixture.paths();
        let base = resolve_base(&guard, &paths, None, "main")
            .await
            .expect("the base resolves");
        create_work_clone(&guard, &paths, session_id, &base, &test_identity())
            .await
            .expect("the work clone is created");
    }

    let commit = fixture
        .commit_in_work_clone(session_id, file, content)
        .await;

    (session_id, commit)
}

/// A `revision` hand-off body.
fn revision(source_session_id: Uuid, commit: &str) -> Value {
    json!({
        "kind": "revision",
        "source_session_id": source_session_id,
        "commit": commit,
        "comment": "ready for review",
    })
}

/// A `forward` hand-off body carrying a review decision.
fn forward(handoff_id: Uuid, review: &str) -> Value {
    json!({
        "kind": "forward",
        "handoff_id": handoff_id,
        "review": review,
        "comment": "reviewed",
    })
}

/// Assert the documented error body (`SPEC.md`, "REST API").
fn assert_error(response: &TestResponse, status: StatusCode, message: &str) {
    response.assert_status(status);
    response.assert_json(&json!({ "status": status.as_u16(), "error": message }));
}

/// A task in `review` whose current hand-off pins `NOTES.md`, with the source
/// session's branch already advanced past it and synced.
///
/// The arrangement every case below starts from: whatever the verifier then
/// answers, the commit that may be merged is the pinned one and the tip of
/// `refs/sessions/<id>` is not it.
struct Published {
    task: Task,
    handoff_id: Uuid,
    session_id: Uuid,
    /// The commit the hand-off pinned.
    pinned: String,
}

async fn publish_revision(fixture: &Fixture, user: &AuthenticatedUser, title: &str) -> Published {
    let (session_id, pinned) = fixture.session_with_commit("feat: the work").await;
    let task = fixture.task(title, "ready").await;

    let published = fixture
        .publish(user, &task, "review", revision(session_id, &pinned))
        .await;
    let handoff_id = published.handoff.expect("the hand-off is published").id;

    // The agent keeps working after the hand-off was pinned, and the branch is
    // synced: the merge must still take the pinned commit (ADR 0018).
    fixture
        .commit_in_work_clone(session_id, "LATER.md", "later")
        .await;
    fixture.sync(session_id).await;

    Published {
        task,
        handoff_id,
        session_id,
        pinned,
    }
}

// ---- the approved path ----

#[tokio::test]
async fn an_approved_current_handoff_merges_its_pinned_commit_and_not_a_newer_tip() {
    let fixture = Fixture::create("merge-approved").await;
    let user = fixture.signed_in();
    let published = publish_revision(&fixture, &user, "implement it").await;

    let approved = fixture
        .publish(
            &user,
            &published.task,
            "merge",
            forward(published.handoff_id, "approved"),
        )
        .await;
    let handoff_id = approved.handoff.expect("the forward publishes").id;

    // Unrelated work lands on `main` first, so the task merge is a real merge
    // commit rather than a fast-forward and its message can be read.
    let (other, _) = session_with_file(&fixture, "OTHER.md", "other").await;
    fixture
        .merge(
            &user,
            json!({ "target": "main", "source": other.to_string() }),
        )
        .await
        .assert_status(StatusCode::OK);

    let response = fixture
        .merge(
            &user,
            json!({
                "target": "main",
                "task_id": published.task.id,
                "handoff_id": handoff_id,
            }),
        )
        .await;

    response.assert_status(StatusCode::OK);
    let merged = response.json::<Value>()["commit"]
        .as_str()
        .expect("the merge answers a commit")
        .to_string();
    assert_eq!(fixture.commit_of("main").await, merged);
    assert_ne!(merged, published.pinned, "a merge commit was expected");

    // By content: the pinned revision is in, the commit the session made
    // afterwards is not, even though the branch was synced past it.
    let files = fixture.files_in("main").await;
    assert!(files.contains(&"NOTES.md".to_string()), "{files:?}");
    assert!(
        !files.contains(&"LATER.md".to_string()),
        "the newer session tip was merged: {files:?}",
    );
    let branch = fixture
        .files_in(&format!("refs/sessions/{}", published.session_id))
        .await;
    assert!(
        branch.contains(&"LATER.md".to_string()) && !branch.contains(&"OTHER.md".to_string()),
        "the session branch itself was touched: {branch:?}",
    );

    // The documented default message (`SPEC.md`, "Git").
    assert_eq!(
        fixture.subject_of("main").await,
        format!(
            "Merge handoff {handoff_id} (session/{}) into main",
            published.session_id
        ),
    );

    // The session whose work was merged is told, even though a user asked
    // (`SPEC.md`, "AgentEvent").
    let events = fixture.git_events(published.session_id).await;
    let merge = events
        .iter()
        .find(|payload| payload["op"] == "merge")
        .expect("the source session is told about the merge");
    assert_eq!(merge["ok"], true);
    assert_eq!(merge["detail"]["source"], handoff_id.to_string());
    assert_eq!(merge["detail"]["target"], "main");
}

// ---- the refusals ----

#[tokio::test]
async fn an_unreviewed_handoff_is_409_and_leaves_the_target_alone() {
    let fixture = Fixture::create("merge-unreviewed").await;
    let user = fixture.signed_in();
    let published = publish_revision(&fixture, &user, "not reviewed yet").await;
    let main_before = fixture.commit_of("main").await;

    let response = fixture
        .merge(
            &user,
            json!({
                "target": "main",
                "task_id": published.task.id,
                "handoff_id": published.handoff_id,
            }),
        )
        .await;

    assert_error(&response, StatusCode::CONFLICT, "hand-off is not approved");
    assert_eq!(fixture.commit_of("main").await, main_before);
}

#[tokio::test]
async fn a_handoff_with_changes_requested_is_409() {
    let fixture = Fixture::create("merge-rejected").await;
    let user = fixture.signed_in();
    let published = publish_revision(&fixture, &user, "rejected").await;

    let rejected = fixture
        .publish(
            &user,
            &published.task,
            "ready",
            forward(published.handoff_id, "changes_requested"),
        )
        .await;
    let handoff_id = rejected.handoff.expect("the forward publishes").id;
    let main_before = fixture.commit_of("main").await;

    let response = fixture
        .merge(
            &user,
            json!({
                "target": "main",
                "task_id": published.task.id,
                "handoff_id": handoff_id,
            }),
        )
        .await;

    assert_error(&response, StatusCode::CONFLICT, "hand-off is not approved");
    assert_eq!(fixture.commit_of("main").await, main_before);
}

#[tokio::test]
async fn an_approved_handoff_superseded_by_a_new_revision_is_409_stale() {
    let fixture = Fixture::create("merge-stale").await;
    let user = fixture.signed_in();
    let published = publish_revision(&fixture, &user, "superseded").await;

    let approved = fixture
        .publish(
            &user,
            &published.task,
            "merge",
            forward(published.handoff_id, "approved"),
        )
        .await;
    let approved_id = approved.handoff.expect("the forward publishes").id;

    // A second revision arrives before anyone pressed merge: the approval
    // stands on a record that is no longer current.
    let next = fixture
        .commit_in_work_clone(published.session_id, "MORE.md", "more")
        .await;
    fixture
        .publish(
            &user,
            &published.task,
            "review",
            revision(published.session_id, &next),
        )
        .await;

    let main_before = fixture.commit_of("main").await;
    let response = fixture
        .merge(
            &user,
            json!({
                "target": "main",
                "task_id": published.task.id,
                "handoff_id": approved_id,
            }),
        )
        .await;

    assert_error(
        &response,
        StatusCode::CONFLICT,
        "handoff_id is not the task's current hand-off",
    );
    assert_eq!(fixture.commit_of("main").await, main_before);
    assert_eq!(
        fixture.review_status(published.task.id).await,
        ReviewStatus::Unreviewed,
        "the new revision is the current one and is unreviewed",
    );
}

#[tokio::test]
async fn another_tasks_handoff_in_the_same_project_is_409_stale() {
    let fixture = Fixture::create("merge-other-task").await;
    let user = fixture.signed_in();
    let mine = publish_revision(&fixture, &user, "mine").await;
    let theirs = publish_revision(&fixture, &user, "theirs").await;

    let approved = fixture
        .publish(
            &user,
            &theirs.task,
            "merge",
            forward(theirs.handoff_id, "approved"),
        )
        .await;
    let approved_id = approved.handoff.expect("the forward publishes").id;
    let main_before = fixture.commit_of("main").await;

    let response = fixture
        .merge(
            &user,
            json!({
                "target": "main",
                "task_id": mine.task.id,
                "handoff_id": approved_id,
            }),
        )
        .await;

    assert_error(
        &response,
        StatusCode::CONFLICT,
        "handoff_id is not the task's current hand-off",
    );
    assert_eq!(fixture.commit_of("main").await, main_before);
}

#[tokio::test]
async fn a_task_this_project_does_not_have_is_404() {
    let fixture = Fixture::create("merge-unknown-task").await;
    let user = fixture.signed_in();
    let main_before = fixture.commit_of("main").await;

    let response = fixture
        .merge(
            &user,
            json!({
                "target": "main",
                "task_id": Uuid::new_v4(),
                "handoff_id": Uuid::new_v4(),
            }),
        )
        .await;

    assert_error(&response, StatusCode::NOT_FOUND, "not found");
    assert_eq!(fixture.commit_of("main").await, main_before);
}

#[tokio::test]
async fn a_conflicting_pinned_commit_is_422_and_the_approval_stands() {
    let fixture = Fixture::create("merge-conflict").await;
    let user = fixture.signed_in();
    let published = publish_revision(&fixture, &user, "conflicting").await;

    let approved = fixture
        .publish(
            &user,
            &published.task,
            "merge",
            forward(published.handoff_id, "approved"),
        )
        .await;
    let handoff_id = approved.handoff.expect("the forward publishes").id;

    // Another session writes the same file and is merged first, through the
    // branch form: now the pinned commit cannot be merged cleanly.
    let (other, _) = fixture.session_with_commit("feat: the other work").await;
    let branch_merge = fixture
        .merge(
            &user,
            json!({ "target": "main", "source": other.to_string() }),
        )
        .await;
    branch_merge.assert_status(StatusCode::OK);
    let main_before = fixture.commit_of("main").await;

    let response = fixture
        .merge(
            &user,
            json!({
                "target": "main",
                "task_id": published.task.id,
                "handoff_id": handoff_id,
            }),
        )
        .await;

    response.assert_status(StatusCode::UNPROCESSABLE_ENTITY);
    let body = response.json::<Value>();
    assert_eq!(body["status"], 422);
    assert_eq!(
        body["conflicts"],
        json!(["NOTES.md"]),
        "the conflicting paths are reported: {body}",
    );
    assert_eq!(fixture.commit_of("main").await, main_before);
    assert_eq!(
        fixture.review_status(published.task.id).await,
        ReviewStatus::Approved,
        "a failed merge does not withdraw the approval",
    );
}

// ---- the branch form beside it ----

#[tokio::test]
async fn the_branch_form_over_the_same_session_merges_and_records_no_approval() {
    let fixture = Fixture::create("merge-branch-form").await;
    let user = fixture.signed_in();
    let published = publish_revision(&fixture, &user, "explicit branch merge").await;
    let before = fixture.reload(published.task.id).await;

    let response = fixture
        .merge(
            &user,
            json!({ "target": "main", "source": published.session_id.to_string() }),
        )
        .await;

    response.assert_status(StatusCode::OK);

    // The branch form syncs and takes the session's tip, which is past the
    // pinned commit — that is exactly what makes it a different operation.
    let files = fixture.files_in("main").await;
    assert!(files.contains(&"LATER.md".to_string()), "{files:?}");

    let after = fixture.reload(published.task.id).await;
    assert_eq!(after.state_id, before.state_id, "the task moved");
    assert_eq!(
        after.current_handoff_id, before.current_handoff_id,
        "the current hand-off moved",
    );
    assert_eq!(
        fixture.review_status(published.task.id).await,
        ReviewStatus::Unreviewed,
        "a generic branch merge granted approval",
    );
}
