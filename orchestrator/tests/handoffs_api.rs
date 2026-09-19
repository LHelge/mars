//! `PUT /api/projects/{pid}/tasks/{id}` with a `handoff`, through the real
//! router (`SPEC.md`, "Tasks" and "Code hand-offs and review";
//! `ARCHITECTURE.md`, "Task tracker" → "Code hand-offs", "Review approval").
//!
//! `tests/handoffs_service.rs` asserts the composition of the two halves under
//! a session caller. What is asserted here is the REST endpoint on top of it,
//! over real repositories:
//!
//! - a revision publication pins the commit, moves the task and answers the
//!   `Task` the detail read then agrees with;
//! - every documented refusal — the input rules, the tip mismatch, the stale
//!   `handoff_id`, a project that is not `ready`, no token, no such task —
//!   with the exact status and message, and nothing written behind it;
//! - the review chain `SPEC.md` gives as its example: approve while
//!   forwarding, forward again carrying that attribution, then a new revision
//!   that resets review while the old approvals stay in history;
//! - that a user is not lease-bound: a user may hand off a task another
//!   session holds, naming any project session with a synced branch as the
//!   source.
//!
//! Git is never mocked (`CLAUDE.md`, "Testing expectations"): the fixture is
//! `common::handoffs::Fixture`, which builds a real bare upstream, a real
//! project repository and real session work clones.
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use axum::http::StatusCode;
use axum_test::TestResponse;
use common::AuthenticatedUser;
use common::handoffs::Fixture;
use mars_orchestrator::git::{GitRef, refs};
use mars_orchestrator::models::{ProjectStatus, ReviewStatus, Task, TaskRef};
use mars_orchestrator::repositories::{ProjectRepository, TaskRepository};
use mars_orchestrator::tracker::{TaskDetailDto, TaskDto};
use serde_json::{Value, json};
use uuid::Uuid;

/// An obviously fake but well-formed object id the repository does not have
/// (rule 3).
const ABSENT_COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

// ---- helpers ----

/// `/api/projects/{pid}/tasks/{id}`, with `id` in either accepted form.
fn task_path(fixture: &Fixture, id: &str) -> String {
    format!("/api/projects/{}/tasks/{}", fixture.project.id, id)
}

/// A `revision` hand-off body naming its source session.
fn revision(source_session_id: Uuid, commit: &str) -> Value {
    json!({
        "kind": "revision",
        "source_session_id": source_session_id,
        "commit": commit,
        "comment": "ready for review",
    })
}

/// A `forward` hand-off body, with or without a decision.
fn forward(handoff_id: Uuid, review: Option<&str>) -> Value {
    let mut body = json!({
        "kind": "forward",
        "handoff_id": handoff_id,
        "comment": "looks good",
    });
    if let Some(review) = review {
        body["review"] = json!(review);
    }
    body
}

/// `PUT` the task with this body, as `user`.
async fn put(fixture: &Fixture, user: &AuthenticatedUser, id: &str, body: Value) -> TestResponse {
    fixture
        .app
        .put_as(user, &task_path(fixture, id))
        .json(&body)
        .await
}

/// `PUT` a hand-off and expect it to be published.
async fn publish(
    fixture: &Fixture,
    user: &AuthenticatedUser,
    task: &Task,
    state: &str,
    handoff: Value,
) -> TaskDto {
    let response = put(
        fixture,
        user,
        &task.id.to_string(),
        json!({ "state": state, "handoff": handoff }),
    )
    .await;

    response.assert_status(StatusCode::OK);
    response.json::<TaskDto>()
}

/// Assert the documented error body (`SPEC.md`, "REST API").
fn assert_error(response: &TestResponse, status: StatusCode, message: &str) {
    response.assert_status(status);
    response.assert_json(&json!({ "status": status.as_u16(), "error": message }));
}

/// The task detail the drawer reads.
async fn detail(fixture: &Fixture, user: &AuthenticatedUser, id: &str) -> TaskDetailDto {
    let response = fixture.app.get_as(user, &task_path(fixture, id)).await;
    response.assert_status(StatusCode::OK);
    response.json::<TaskDetailDto>()
}

/// Everything a refused request must have left alone.
struct Snapshot {
    task: Task,
    events: usize,
    handoffs: usize,
    refs: Vec<(Uuid, String)>,
}

impl Fixture {
    async fn snapshot(&self, task_id: Uuid) -> Snapshot {
        let repository = TaskRepository::new(&self.app.pool);

        Snapshot {
            task: repository
                .find_task(self.project.id, TaskRef::Id(task_id))
                .await
                .expect("the task reads")
                .expect("the task exists"),
            events: self.event_kinds().await.len(),
            handoffs: repository
                .list_handoffs(self.project.id, task_id)
                .await
                .expect("the hand-offs read")
                .len(),
            refs: self.handoff_refs().await,
        }
    }

    /// Every task event of this project, oldest first, as `(kind, payload)`.
    async fn events(&self) -> Vec<(String, Value)> {
        TaskRepository::new(&self.app.pool)
            .list_task_events_after(self.project.id, 0, 100)
            .await
            .expect("the events read")
            .into_iter()
            .map(|row| (row.kind, row.payload))
            .collect()
    }

    async fn event_kinds(&self) -> Vec<String> {
        self.events()
            .await
            .into_iter()
            .map(|(kind, _)| kind)
            .collect()
    }

    /// Move the project out of `ready`, the way a failed clone would.
    async fn set_project_status(&self, status: ProjectStatus) {
        let mut tx = self.app.pool.begin().await.expect("a transaction begins");
        ProjectRepository::new(&self.app.pool)
            .set_status(&mut tx, self.project.id, status, None)
            .await
            .expect("the status is set")
            .expect("the project exists");
        tx.commit().await.expect("the transaction commits");
    }
}

/// Assert that nothing moved between two snapshots of a refused request.
fn unchanged(before: &Snapshot, after: &Snapshot) {
    assert_eq!(before.task.state_id, after.task.state_id, "the state moved");
    assert_eq!(
        before.task.lease_holder_session_id, after.task.lease_holder_session_id,
        "the lease moved",
    );
    assert_eq!(
        before.task.attempts, after.task.attempts,
        "the attempt count moved",
    );
    assert_eq!(
        before.task.current_handoff_id, after.task.current_handoff_id,
        "the current hand-off moved",
    );
    assert_eq!(
        before.events, after.events,
        "a refused request wrote task events",
    );
    assert_eq!(
        before.handoffs, after.handoffs,
        "a refused request wrote a hand-off row",
    );
    assert_eq!(
        before.refs, after.refs,
        "a refused request left a retained ref behind",
    );
}

// ---- publication ----

#[tokio::test]
async fn a_revision_publication_pins_the_commit_and_moves_the_task() {
    let fixture = Fixture::create("api-revision").await;
    let user = fixture.signed_in();
    let (session_id, commit) = fixture.session_with_commit("feat: the work").await;
    let task = fixture.task("implement it", "ready").await;

    let published = publish(
        &fixture,
        &user,
        &task,
        "review",
        revision(session_id, &commit),
    )
    .await;

    assert_eq!(published.state, "review");
    assert_eq!(published.lease_holder_session_id, None);
    assert_eq!(published.attempts, 0);

    let handoff = published.handoff.expect("the task carries its hand-off");
    assert_eq!(handoff.commit, commit);
    assert_eq!(handoff.review_status, ReviewStatus::Unreviewed);
    assert_eq!(handoff.source_session_id, Some(session_id));
    assert_eq!(handoff.created_by_user_id, Some(fixture.user.id));
    assert_eq!(handoff.created_by_session_id, None);
    assert_eq!(handoff.reviewed_at, None);

    // The ref really pins that commit, under the hand-off's own id (ADR 0018).
    let pinned = refs::resolve(
        &fixture.paths().project_repo(fixture.project.id),
        &GitRef::Handoff(handoff.id),
    )
    .await
    .expect("the hand-off ref resolves");
    assert_eq!(pinned.commit, commit);

    // The detail read agrees: one hand-off, and the comment it was published
    // with (`SPEC.md`, "Code hand-offs and review").
    let detail = detail(&fixture, &user, &task.id.to_string()).await;
    assert_eq!(detail.handoffs.len(), 1);
    assert_eq!(detail.handoffs[0].id, handoff.id);
    assert_eq!(
        detail.task.handoff.map(|current| current.id),
        Some(handoff.id)
    );
    assert_eq!(detail.comments.len(), 1);
    assert_eq!(handoff.comment_id, Some(detail.comments[0].id));
    assert_eq!(detail.comments[0].body, "ready for review");
    assert_eq!(detail.comments[0].author_user_id, Some(fixture.user.id));
    assert!(!detail.comments[0].system);
}

#[tokio::test]
async fn a_revision_carries_ordinary_field_updates_and_takes_the_task_number() {
    let fixture = Fixture::create("api-fields").await;
    let user = fixture.signed_in();
    let (session_id, commit) = fixture.session_with_commit("feat: the work").await;
    let task = fixture.task("implement it", "ready").await;

    // Addressed by its per-project number, not its UUID (`SPEC.md`, "Tasks").
    let response = put(
        &fixture,
        &user,
        &task.number.to_string(),
        json!({
            "state": "review",
            "title": "implement it properly",
            "labels": ["backend"],
            "handoff": revision(session_id, &commit),
        }),
    )
    .await;

    response.assert_status(StatusCode::OK);
    let published = response.json::<TaskDto>();
    assert_eq!(published.id, task.id);
    assert_eq!(published.title, "implement it properly");
    assert_eq!(published.labels, vec!["backend".to_string()]);
    assert_eq!(published.state, "review");
    assert!(published.handoff.is_some());
}

#[tokio::test]
async fn a_hand_off_into_the_human_state_is_a_state_change_by_the_user() {
    let fixture = Fixture::create("api-human").await;
    let user = fixture.signed_in();
    let (session_id, commit) = fixture.session_with_commit("feat: the work").await;
    let task = fixture.task("implement it", "ready").await;

    let published = publish(
        &fixture,
        &user,
        &task,
        "needs_human",
        revision(session_id, &commit),
    )
    .await;
    assert_eq!(published.state, "needs_human");
    assert_eq!(published.needs_human_reason, None);

    // Publishing code to a person is a hand-off, not an escalation
    // (`SPEC.md`, "TaskEvent").
    assert_eq!(
        fixture.event_kinds().await,
        vec!["state_changed".to_string(), "commented".to_string()],
    );
}

#[tokio::test]
async fn a_user_may_hand_off_a_task_held_by_another_session_and_the_lease_is_cleared() {
    let fixture = Fixture::create("api-lease").await;
    let user = fixture.signed_in();
    let holder = fixture.seed_session().await;
    let (source, commit) = fixture.session_with_commit("feat: the work").await;
    let task = fixture.task("implement it", "ready").await;
    let task = fixture.claim(task.id, holder).await;
    assert_eq!(task.lease_holder_session_id, Some(holder));

    // A user is not lease-bound, and the source may be any project session
    // with a synced branch (`SPEC.md`, "Code hand-offs and review").
    let published = publish(&fixture, &user, &task, "review", revision(source, &commit)).await;

    assert_eq!(published.lease_holder_session_id, None);
    assert_eq!(published.attempts, 0);
    assert_eq!(
        published.handoff.map(|handoff| handoff.source_session_id),
        Some(Some(source)),
    );
}

// ---- the review chain ----

#[tokio::test]
async fn forward_with_approved_then_forward_without_decision_carries_attribution() {
    let fixture = Fixture::create("api-forward").await;
    let user = fixture.signed_in();
    let (session_id, commit) = fixture.session_with_commit("feat: the work").await;
    let task = fixture.task("implement it", "ready").await;

    let published = publish(
        &fixture,
        &user,
        &task,
        "review",
        revision(session_id, &commit),
    )
    .await;
    let first = published.handoff.expect("the revision is current");

    let approved = publish(
        &fixture,
        &user,
        &task,
        "merge",
        forward(first.id, Some("approved")),
    )
    .await
    .handoff
    .expect("the forward is current");

    // A new record for the same commit, reviewed by the caller.
    assert_ne!(approved.id, first.id);
    assert_eq!(approved.commit, first.commit);
    assert_eq!(approved.source_branch, first.source_branch);
    assert_eq!(approved.review_status, ReviewStatus::Approved);
    assert_eq!(approved.reviewed_by_user_id, Some(fixture.user.id));
    assert!(approved.reviewed_at.is_some());

    // And the record it came from is untouched in history.
    let history = detail(&fixture, &user, &task.id.to_string()).await.handoffs;
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].id, first.id);
    assert_eq!(history[0].review_status, ReviewStatus::Unreviewed);
    assert_eq!(history[0].reviewed_by_user_id, None);
    assert_eq!(history[0].reviewed_at, None);

    // Forwarding again with no decision carries the verdict, the reviewer and
    // the review time forward (`SPEC.md`, "Code hand-offs and review").
    let carried = publish(&fixture, &user, &task, "done", forward(approved.id, None))
        .await
        .handoff
        .expect("the second forward is current");

    assert_ne!(carried.id, approved.id);
    assert_eq!(carried.commit, first.commit);
    assert_eq!(carried.review_status, ReviewStatus::Approved);
    assert_eq!(carried.reviewed_by_user_id, Some(fixture.user.id));
    assert_eq!(carried.reviewed_at, approved.reviewed_at);
}

#[tokio::test]
async fn a_new_revision_after_an_approval_is_unreviewed() {
    let fixture = Fixture::create("api-reset").await;
    let user = fixture.signed_in();
    let (session_id, commit) = fixture.session_with_commit("feat: the work").await;
    let task = fixture.task("implement it", "ready").await;

    let first = publish(
        &fixture,
        &user,
        &task,
        "review",
        revision(session_id, &commit),
    )
    .await
    .handoff
    .expect("the revision is current");

    let approved = publish(
        &fixture,
        &user,
        &task,
        "merge",
        forward(first.id, Some("approved")),
    )
    .await
    .handoff
    .expect("the forward is current");

    // Even at the very same commit: "a new revision always resets review
    // status to `unreviewed`; old approvals stay in history."
    let republished = publish(
        &fixture,
        &user,
        &task,
        "review",
        revision(session_id, &commit),
    )
    .await
    .handoff
    .expect("the new revision is current");

    assert_eq!(republished.commit, commit);
    assert_eq!(republished.review_status, ReviewStatus::Unreviewed);
    assert_eq!(republished.reviewed_by_user_id, None);
    assert_eq!(republished.reviewed_at, None);

    let history = detail(&fixture, &user, &task.id.to_string()).await.handoffs;
    assert_eq!(
        history.iter().map(|row| row.id).collect::<Vec<_>>(),
        vec![first.id, approved.id, republished.id],
        "the history is oldest first",
    );
    assert_eq!(history[1].review_status, ReviewStatus::Approved);
}

#[tokio::test]
async fn a_forward_works_after_the_source_session_row_is_deleted() {
    let fixture = Fixture::create("api-deleted-source").await;
    let user = fixture.signed_in();
    let (session_id, commit) = fixture.session_with_commit("feat: the work").await;
    let task = fixture.task("implement it", "ready").await;

    let first = publish(
        &fixture,
        &user,
        &task,
        "review",
        revision(session_id, &commit),
    )
    .await
    .handoff
    .expect("the revision is current");

    // The session ends and its row goes; `ON DELETE SET NULL` leaves the
    // branch and the commit behind (`docs/data-model.md`, `task_handoffs`).
    // No repository interface deletes a session outside the sessions route's
    // own transaction, so the row is removed directly.
    sqlx::query("DELETE FROM sessions WHERE id = $1")
        .bind(session_id)
        .execute(&fixture.app.pool)
        .await
        .expect("the session row is deleted");

    let forwarded = publish(
        &fixture,
        &user,
        &task,
        "merge",
        forward(first.id, Some("approved")),
    )
    .await
    .handoff
    .expect("the forward is current");

    assert_eq!(forwarded.commit, commit);
    assert_eq!(forwarded.source_branch, first.source_branch);
    assert_eq!(forwarded.source_session_id, None);
    assert_eq!(forwarded.review_status, ReviewStatus::Approved);
    assert_eq!(forwarded.reviewed_by_user_id, Some(fixture.user.id));
}

// ---- events ----

#[tokio::test]
async fn each_publication_emits_state_changed_with_the_hand_off_then_commented() {
    let fixture = Fixture::create("api-events").await;
    let user = fixture.signed_in();
    let (session_id, commit) = fixture.session_with_commit("feat: the work").await;
    let task = fixture.task("implement it", "ready").await;

    let first = publish(
        &fixture,
        &user,
        &task,
        "review",
        revision(session_id, &commit),
    )
    .await
    .handoff
    .expect("the revision is current");
    let second = publish(
        &fixture,
        &user,
        &task,
        "merge",
        forward(first.id, Some("approved")),
    )
    .await
    .handoff
    .expect("the forward is current");

    let events = fixture.events().await;
    assert_eq!(
        events
            .iter()
            .map(|(kind, _)| kind.as_str())
            .collect::<Vec<_>>(),
        vec!["state_changed", "commented", "state_changed", "commented"],
    );

    // "Every state event carries the full task including its current
    // hand-off" (`SPEC.md`, "Code hand-offs and review").
    assert_eq!(events[0].1["task"]["handoff"]["id"], json!(first.id));
    assert_eq!(events[0].1["task"]["state"], json!("review"));
    assert_eq!(events[2].1["task"]["handoff"]["id"], json!(second.id));
    assert_eq!(events[2].1["task"]["state"], json!("merge"));

    // A rejected request adds nothing at all.
    let refused = put(
        &fixture,
        &user,
        &task.id.to_string(),
        json!({ "state": "review", "handoff": forward(first.id, None) }),
    )
    .await;
    refused.assert_status(StatusCode::CONFLICT);
    assert_eq!(fixture.event_kinds().await.len(), 4);
}

// ---- refusals ----

#[tokio::test]
async fn a_tip_mismatch_leaves_task_lease_attempts_and_refs_unchanged() {
    let fixture = Fixture::create("api-tip").await;
    let user = fixture.signed_in();
    let (session_id, first) = fixture.session_with_commit("feat: the work").await;
    let task = fixture.task("implement it", "ready").await;
    let task = fixture.claim(task.id, session_id).await;

    // The agent commits again after naming `first`, so the branch tip has
    // moved on: "sync must produce that exact tip or return 409".
    let tip = fixture
        .commit_in_work_clone(session_id, "MORE.md", "feat: more work")
        .await;

    let before = fixture.snapshot(task.id).await;
    let response = put(
        &fixture,
        &user,
        &task.id.to_string(),
        json!({ "state": "review", "handoff": revision(session_id, &first) }),
    )
    .await;

    assert_error(
        &response,
        StatusCode::CONFLICT,
        &format!("session branch tip {tip} does not match commit {first}"),
    );
    unchanged(&before, &fixture.snapshot(task.id).await);
}

#[tokio::test]
async fn a_stale_handoff_id_after_a_forward_is_refused() {
    let fixture = Fixture::create("api-stale").await;
    let user = fixture.signed_in();
    let (session_id, commit) = fixture.session_with_commit("feat: the work").await;
    let task = fixture.task("implement it", "ready").await;

    let first = publish(
        &fixture,
        &user,
        &task,
        "review",
        revision(session_id, &commit),
    )
    .await
    .handoff
    .expect("the revision is current");
    publish(
        &fixture,
        &user,
        &task,
        "merge",
        forward(first.id, Some("approved")),
    )
    .await;

    // `first` is history now, so forwarding it again is the documented 409.
    let before = fixture.snapshot(task.id).await;
    let response = put(
        &fixture,
        &user,
        &task.id.to_string(),
        json!({ "state": "review", "handoff": forward(first.id, None) }),
    )
    .await;

    assert_error(
        &response,
        StatusCode::CONFLICT,
        "handoff_id is not the task's current hand-off",
    );
    unchanged(&before, &fixture.snapshot(task.id).await);
}

#[tokio::test]
async fn a_hand_off_is_refused_while_the_project_is_not_ready() {
    let fixture = Fixture::create("api-not-ready").await;
    let user = fixture.signed_in();
    let (session_id, commit) = fixture.session_with_commit("feat: the work").await;
    let task = fixture.task("implement it", "ready").await;
    fixture.set_project_status(ProjectStatus::Error).await;

    let before = fixture.snapshot(task.id).await;
    let response = put(
        &fixture,
        &user,
        &task.id.to_string(),
        json!({ "state": "review", "handoff": revision(session_id, &commit) }),
    )
    .await;

    assert_error(&response, StatusCode::CONFLICT, "project is not ready");
    unchanged(&before, &fixture.snapshot(task.id).await);
}

#[tokio::test]
async fn the_input_rules_are_refused_with_the_documented_messages() {
    let fixture = Fixture::create("api-input").await;
    let user = fixture.signed_in();
    let (session_id, commit) = fixture.session_with_commit("feat: the work").await;
    let other_project_session = fixture.session_in_another_project().await;
    let task = fixture.task("implement it", "ready").await;
    let id = task.id.to_string();
    let before = fixture.snapshot(task.id).await;

    let mut blank = revision(session_id, &commit);
    blank["comment"] = json!("   ");
    let mut sourceless = revision(session_id, &commit);
    sourceless["source_session_id"] = json!(null);

    let cases: Vec<(Value, &str)> = vec![
        // No `state` at all, and the state the task is already in: one
        // message (`SPEC.md`, "Code hand-offs and review").
        (
            json!({ "handoff": revision(session_id, &commit) }),
            "handoff requires a different target state",
        ),
        (
            json!({ "state": "ready", "handoff": revision(session_id, &commit) }),
            "handoff requires a different target state",
        ),
        (
            json!({ "state": "review", "handoff": blank }),
            "comment body must not be empty",
        ),
        (
            json!({ "state": "review", "handoff": sourceless }),
            "revision hand-off requires source_session_id",
        ),
        (
            json!({ "state": "review", "handoff": revision(session_id, &commit[..39]) }),
            "commit must be a full lowercase hexadecimal git object id",
        ),
        (
            json!({ "state": "review", "handoff": revision(session_id, &commit.to_uppercase()) }),
            "commit must be a full lowercase hexadecimal git object id",
        ),
        (
            json!({
                "state": "review",
                "handoff": revision(other_project_session, &commit),
            }),
            "source_session_id must name a session of this project",
        ),
    ];

    for (body, message) in cases {
        let response = put(&fixture, &user, &id, body.clone()).await;
        assert_error(&response, StatusCode::BAD_REQUEST, message);
        assert_eq!(
            response.json::<Value>()["error"],
            json!(message),
            "for {body}",
        );
    }

    // A commit the project repository does not have is a 409, not a 400: the
    // shape is right and the retention is what fails.
    let absent = put(
        &fixture,
        &user,
        &id,
        json!({ "state": "review", "handoff": revision(session_id, ABSENT_COMMIT) }),
    )
    .await;
    absent.assert_status(StatusCode::CONFLICT);

    unchanged(&before, &fixture.snapshot(task.id).await);
}

#[tokio::test]
async fn a_malformed_hand_off_body_is_a_bad_request_in_the_documented_shape() {
    let fixture = Fixture::create("api-malformed").await;
    let user = fixture.signed_in();
    let (session_id, commit) = fixture.session_with_commit("feat: the work").await;
    let task = fixture.task("implement it", "ready").await;
    let id = task.id.to_string();

    for handoff in [
        // No `kind`, an unknown one, and a missing required field.
        json!({ "commit": commit, "comment": "ready" }),
        json!({ "kind": "publish", "commit": commit, "comment": "ready" }),
        json!({ "kind": "revision", "source_session_id": session_id, "comment": "ready" }),
        json!({ "kind": "forward", "comment": "ready" }),
    ] {
        let response = put(
            &fixture,
            &user,
            &id,
            json!({ "state": "review", "handoff": handoff }),
        )
        .await;

        response.assert_status(StatusCode::BAD_REQUEST);
        let body = response.json::<Value>();
        assert_eq!(body["status"], json!(400), "for {handoff}");
        assert!(body["error"].is_string(), "for {handoff}");
    }

    // Unknown fields inside the hand-off are ignored, as elsewhere in the API.
    let response = put(
        &fixture,
        &user,
        &id,
        json!({
            "state": "review",
            "handoff": {
                "kind": "revision",
                "source_session_id": session_id,
                "commit": commit,
                "comment": "ready for review",
                "unexpected": true,
            },
        }),
    )
    .await;
    response.assert_status(StatusCode::OK);
}

#[tokio::test]
async fn publishing_needs_a_token_a_project_and_a_task_that_exist() {
    let fixture = Fixture::create("api-auth").await;
    let user = fixture.signed_in();
    let (session_id, commit) = fixture.session_with_commit("feat: the work").await;
    let task = fixture.task("implement it", "ready").await;
    let body = json!({ "state": "review", "handoff": revision(session_id, &commit) });

    let anonymous = fixture
        .app
        .server
        .put(&task_path(&fixture, &task.id.to_string()))
        .json(&body)
        .await;
    assert_error(
        &anonymous,
        StatusCode::UNAUTHORIZED,
        "authentication required",
    );

    let unknown_task = put(&fixture, &user, &Uuid::new_v4().to_string(), body.clone()).await;
    assert_error(&unknown_task, StatusCode::NOT_FOUND, "not found");

    let unknown_project = fixture
        .app
        .put_as(
            &user,
            &format!("/api/projects/{}/tasks/{}", Uuid::new_v4(), task.id),
        )
        .json(&body)
        .await;
    assert_error(&unknown_project, StatusCode::NOT_FOUND, "not found");

    // None of that published anything.
    assert!(fixture.handoff_refs().await.is_empty());
    assert!(fixture.event_kinds().await.is_empty());
}
