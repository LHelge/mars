//! Dropping a task's current hand-off (`SPEC.md`, "Code hand-offs and
//! review", `POST /api/projects/{pid}/tasks/{id}/drop-handoff`;
//! `ARCHITECTURE.md`, "Task tracker" → "Code hand-offs").
//!
//! What is asserted:
//!
//! - the endpoint clears the pointer and nothing else — state, lease,
//!   `attempts`, `rounds` and `closed_at` are what they were, a held task keeps
//!   its holder, and the hand-off history and its `refs/handoffs/<id>` stay;
//! - the comment is the user's, and the events are `updated` then `commented`;
//! - the refusals: 409 for a task with no current hand-off, 400 for an empty
//!   comment, 401 without a token, 404 for a task of another project and an
//!   unknown project, each leaving nothing behind;
//! - the next launch for the task starts from the default branch;
//! - the verb drops several tasks' hand-offs inside one mutation, which is
//!   what a rollback of the default branch does, and a refusal in that
//!   mutation rolls all of them back.
//!
//! Git is never mocked: hand-offs are published through the real protocol
//! against the real repositories of `common::handoffs::Fixture`, except in the
//! verb scenarios, which arrange the pointer with `common::tracker`.
//!
//! Every credential-shaped value is an obviously fake stand-in (rule 3).
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use std::time::Duration;

use axum::http::StatusCode;
use axum_test::TestResponse;
use common::AuthenticatedUser;
use common::handoffs::Fixture;
use common::tracker::{Handoff, handoff_in_place, in_mutation, move_to};
use mars_orchestrator::events::TaskActor;
use mars_orchestrator::git::testutil::run_git;
use mars_orchestrator::models::{Session, SessionState, Task};
use mars_orchestrator::prelude::*;
use mars_orchestrator::projects::{NewProjectRequest, create_project};
use mars_orchestrator::repositories::{SessionRepository, TaskRepository};
use mars_orchestrator::tracker::{
    NO_CURRENT_HANDOFF, ReviewCarry, TaskDetailDto, TaskDto, drop_handoff,
};
use serde_json::{Value, json};
use uuid::Uuid;

/// How long a launch may take to reach `running` before the scenario gives up.
const PATIENCE: Duration = Duration::from_secs(30);

/// How often the launch poll re-reads the session.
const POLL: Duration = Duration::from_millis(25);

/// Not a real remote: `.invalid` can never resolve (rule 3).
const TEST_REMOTE: &str = "https://example.invalid/org/repo.git";

/// An obviously fake but well-formed object id for the verb scenarios, whose
/// hand-offs are arranged without a ref (rule 3).
const FAKE_COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

// ---- helpers ----

/// `/api/projects/{pid}/tasks/{id}/drop-handoff`.
fn drop_path(pid: Uuid, id: impl std::fmt::Display) -> String {
    format!("/api/projects/{pid}/tasks/{id}/drop-handoff")
}

/// Assert the documented error body (`SPEC.md`, "REST API").
fn assert_error(response: &TestResponse, status: StatusCode, message: &str) {
    response.assert_status(status);
    response.assert_json(&json!({ "status": status.as_u16(), "error": message }));
}

impl Fixture {
    /// A task in `review` whose current hand-off is a revision really
    /// published over HTTP, with a retained ref: the task and the hand-off id.
    async fn published(&self, user: &AuthenticatedUser, title: &str) -> (Task, Uuid, String) {
        let (source, commit) = self.session_with_commit("feat: the parser").await;
        let task = self.task(title, "ready").await;

        let response = self
            .app
            .put_as(
                user,
                &format!("/api/projects/{}/tasks/{}", self.project.id, task.id),
            )
            .json(&json!({
                "state": "review",
                "handoff": {
                    "kind": "revision",
                    "source_session_id": source,
                    "commit": commit,
                    "comment": "built on the wrong base",
                },
            }))
            .await;
        response.assert_status(StatusCode::OK);
        let handoff_id = response
            .json::<TaskDto>()
            .handoff
            .expect("a publication leaves a current hand-off")
            .id;

        (self.task_row(task.id).await, handoff_id, commit)
    }

    /// `POST .../drop-handoff` as `user` with `body`.
    async fn drop_as(
        &self,
        user: &AuthenticatedUser,
        task: impl std::fmt::Display,
        body: Value,
    ) -> TestResponse {
        self.app
            .post_as(user, &drop_path(self.project.id, task))
            .json(&body)
            .await
    }

    /// The task as the drawer reads it.
    async fn detail(&self, user: &AuthenticatedUser, task_id: Uuid) -> TaskDetailDto {
        let response = self
            .app
            .get_as(
                user,
                &format!("/api/projects/{}/tasks/{task_id}", self.project.id),
            )
            .await;
        response.assert_status(StatusCode::OK);

        response.json::<TaskDetailDto>()
    }

    /// The project's last event sequence, as a cursor for what comes next.
    async fn cursor(&self) -> i64 {
        self.events_after(0)
            .await
            .last()
            .map(|(seq, _, _)| *seq)
            .unwrap_or(0)
    }

    /// The project's events after `after`, as `(seq, kind, payload)`.
    async fn events_after(&self, after: i64) -> Vec<(i64, String, Value)> {
        TaskRepository::new(&self.app.pool)
            .list_task_events_after(self.project.id, after, 1000)
            .await
            .expect("the events read")
            .into_iter()
            .map(|row| (row.seq, row.kind, row.payload))
            .collect()
    }

    /// The session row as it stands.
    async fn session_row(&self, id: Uuid) -> Session {
        SessionRepository::new(&self.app.pool)
            .find(id)
            .await
            .expect("the session reads")
            .expect("the session exists")
    }
}

/// Every column the drop must leave alone, side by side.
fn untouched(task: &Task) -> impl PartialEq + std::fmt::Debug {
    (
        task.state_id,
        task.lease_holder_session_id,
        task.lease_since,
        task.attempts,
        task.rounds,
        task.closed_at,
        task.needs_human_reason.clone(),
    )
}

// ---- the endpoint ----

#[tokio::test]
async fn dropping_clears_the_pointer_and_nothing_else_on_a_held_task() {
    let fixture = Fixture::create("drop-held").await;
    let user = fixture.signed_in();
    let (task, handoff_id, _) = fixture.published(&user, "the parser").await;

    // Held by a session, which keeps its lease through the drop.
    let holder = fixture.seed_session().await;
    let before = fixture.claim(task.id, holder).await;
    assert_eq!(before.current_handoff_id, Some(handoff_id));
    assert_eq!(before.rounds, 1, "the revision counted a round");
    let cursor = fixture.cursor().await;

    let response = fixture
        .drop_as(
            &user,
            task.id,
            json!({ "comment": "wrong base; start over" }),
        )
        .await;
    response.assert_status(StatusCode::OK);
    let answered = response.json::<TaskDto>();
    assert!(answered.handoff.is_none(), "the answer has no hand-off");
    assert_eq!(answered.state, "review");
    assert_eq!(answered.lease_holder_session_id, Some(holder));

    let after = fixture.task_row(task.id).await;
    assert_eq!(after.current_handoff_id, None);
    assert_eq!(untouched(&after), untouched(&before));

    // The history and its ref stay; only the pointer moved.
    let detail = fixture.detail(&user, task.id).await;
    assert_eq!(
        detail
            .handoffs
            .iter()
            .map(|handoff| handoff.id)
            .collect::<Vec<_>>(),
        vec![handoff_id],
    );
    assert!(
        fixture
            .handoff_refs()
            .await
            .iter()
            .any(|(id, _)| *id == handoff_id),
        "the dropped hand-off's ref is retained",
    );

    // The comment is the user's own.
    let comment = detail.comments.last().expect("the drop wrote a comment");
    assert_eq!(comment.body, "wrong base; start over");
    assert_eq!(comment.author_user_id, Some(user.user.id));
    assert!(!comment.system);

    // `updated` carrying the task without its hand-off, then `commented`.
    let events = fixture.events_after(cursor).await;
    let kinds: Vec<&str> = events.iter().map(|(_, kind, _)| kind.as_str()).collect();
    assert_eq!(kinds, vec!["updated", "commented"]);
    for (_, _, payload) in &events {
        assert_eq!(payload["task"]["handoff"], Value::Null);
        assert_eq!(
            payload["actor"],
            json!({ "kind": "user", "user_id": user.user.id }),
        );
    }
    assert_eq!(
        events[1].2["comment"]["body"],
        json!("wrong base; start over")
    );
}

#[tokio::test]
async fn a_task_without_a_current_hand_off_is_409() {
    let fixture = Fixture::create("drop-none").await;
    let user = fixture.signed_in();
    let task = fixture.task("never published", "ready").await;
    let cursor = fixture.cursor().await;

    let response = fixture
        .drop_as(&user, task.id, json!({ "comment": "nothing to drop" }))
        .await;
    assert_error(&response, StatusCode::CONFLICT, NO_CURRENT_HANDOFF);

    assert!(fixture.detail(&user, task.id).await.comments.is_empty());
    assert!(fixture.events_after(cursor).await.is_empty());
}

#[tokio::test]
async fn an_empty_comment_is_400_and_leaves_the_hand_off() {
    let fixture = Fixture::create("drop-empty").await;
    let user = fixture.signed_in();
    let (task, handoff_id, _) = fixture.published(&user, "the parser").await;
    let comments = fixture.detail(&user, task.id).await.comments.len();
    let cursor = fixture.cursor().await;

    for comment in ["", "  \n "] {
        let response = fixture
            .drop_as(&user, task.id, json!({ "comment": comment }))
            .await;
        assert_error(
            &response,
            StatusCode::BAD_REQUEST,
            "comment body must not be empty",
        );
    }

    // A body without the field at all is a malformed body.
    let response = fixture.drop_as(&user, task.id, json!({})).await;
    response.assert_status(StatusCode::BAD_REQUEST);

    assert_eq!(
        fixture.task_row(task.id).await.current_handoff_id,
        Some(handoff_id)
    );
    assert_eq!(
        fixture.detail(&user, task.id).await.comments.len(),
        comments
    );
    assert!(fixture.events_after(cursor).await.is_empty());
}

#[tokio::test]
async fn dropping_needs_a_token() {
    let fixture = Fixture::create("drop-unauth").await;
    let user = fixture.signed_in();
    let (task, handoff_id, _) = fixture.published(&user, "the parser").await;

    let response = fixture
        .app
        .server
        .post(&drop_path(fixture.project.id, task.id))
        .json(&json!({ "comment": "who am I" }))
        .await;
    response.assert_status(StatusCode::UNAUTHORIZED);

    assert_eq!(
        fixture.task_row(task.id).await.current_handoff_id,
        Some(handoff_id)
    );
}

#[tokio::test]
async fn a_task_of_another_project_and_an_unknown_project_are_both_404() {
    let fixture = Fixture::create("drop-scope").await;
    let user = fixture.signed_in();
    let (task, handoff_id, _) = fixture.published(&user, "the parser").await;

    // There is no project membership: a task is out of reach only by being
    // addressed under a project it is not in.
    let other = create_project(
        &fixture.app.state,
        NewProjectRequest {
            name: "drop-scope-other".to_string(),
            remote_url: TEST_REMOTE.to_string(),
            default_branch: None,
            credential: None,
        },
        user.user.id,
    )
    .await
    .expect("the other project is created");
    let elsewhere = fixture
        .app
        .post_as(&user, &format!("/api/projects/{}/tasks", other.id))
        .json(&json!({ "title": "somebody else's" }))
        .await;
    elsewhere.assert_status(StatusCode::CREATED);
    let elsewhere = elsewhere.json::<TaskDto>();

    // Addressed under the wrong project, the task is not found.
    let response = fixture
        .app
        .post_as(&user, &drop_path(fixture.project.id, elsewhere.id))
        .json(&json!({ "comment": "wrong door" }))
        .await;
    assert_error(&response, StatusCode::NOT_FOUND, "not found");

    let response = fixture
        .app
        .post_as(&user, &drop_path(Uuid::new_v4(), task.id))
        .json(&json!({ "comment": "nobody home" }))
        .await;
    assert_error(&response, StatusCode::NOT_FOUND, "not found");

    let response = fixture
        .app
        .post_as(&user, &drop_path(fixture.project.id, "not-a-task"))
        .json(&json!({ "comment": "nobody home" }))
        .await;
    assert_error(&response, StatusCode::NOT_FOUND, "not found");

    assert_eq!(
        fixture.task_row(task.id).await.current_handoff_id,
        Some(handoff_id)
    );
}

#[tokio::test]
async fn the_next_launch_starts_from_the_default_branch() {
    let fixture = Fixture::create("drop-launch").await;
    let user = fixture.signed_in();
    let (task, _, commit) = fixture.published(&user, "the parser").await;

    // Addressed by its number, as the board does.
    let response = fixture
        .drop_as(&user, task.number, json!({ "comment": "start over" }))
        .await;
    response.assert_status(StatusCode::OK);

    let response = fixture
        .app
        .post_as(
            &user,
            &format!("/api/projects/{}/sessions", fixture.project.id),
        )
        .json(&json!({ "profile_id": fixture.profile_id, "task_id": task.id }))
        .await;
    response.assert_status(StatusCode::CREATED);
    let body = response.json::<Value>();
    let id: Uuid = body["id"]
        .as_str()
        .expect("a session carries an id")
        .parse()
        .expect("the id is a uuid");

    assert_eq!(body["base_ref"], json!("main"));
    assert_eq!(body["handoff_id"], Value::Null);

    // And the checkout really is the integration head, not the dropped work.
    let deadline = tokio::time::Instant::now() + PATIENCE;
    loop {
        let session = fixture.session_row(id).await;
        if session.state == SessionState::Running {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the session did not start within {PATIENCE:?}: {:?} {:?}",
            session.state,
            session.error,
        );
        tokio::time::sleep(POLL).await;
    }

    let head = run_git(&fixture.paths().session_work(id), &["rev-parse", "HEAD"]).await;
    let main = run_git(
        &fixture.paths().project_repo(fixture.project.id),
        &["rev-parse", "refs/heads/main"],
    )
    .await;
    assert_eq!(head.trim(), main.trim());
    assert_ne!(head.trim(), commit);
}

// ---- the verb, many tasks in one mutation ----

/// A task in `state` with a current hand-off arranged through the tracker.
async fn with_handoff(fixture: &Fixture, title: &str, state: &str) -> (Task, Uuid) {
    let task = fixture.task(title, "ready").await;
    let (_, handoff_id) = handoff_in_place(
        &fixture.app.pool,
        fixture.project.id,
        task.id,
        Handoff {
            source_session_id: None,
            source_branch: "session/fake",
            commit: FAKE_COMMIT,
            comment: "the first revision",
            target_state: "",
            caller: fixture.user_caller(),
            review: ReviewCarry::Fresh,
        },
    )
    .await;
    move_to(&fixture.app.pool, fixture.project.id, task.id, state).await;

    (fixture.task_row(task.id).await, handoff_id)
}

#[tokio::test]
async fn one_mutation_drops_the_hand_offs_of_several_tasks() {
    let fixture = Fixture::create("drop-many").await;
    let (open, _) = with_handoff(&fixture, "open work", "ready").await;
    let (closed, _) = with_handoff(&fixture, "merged work", "done").await;
    assert!(closed.closed_at.is_some());
    let user_id = fixture.user.id;

    let dropped = in_mutation(
        &fixture.app.pool,
        fixture.project.id,
        TaskActor::User { user_id },
        async |m| {
            let one = drop_handoff(m, open.id, user_id, "rolled back").await?;
            let two = drop_handoff(m, closed.id, user_id, "rolled back").await?;
            Ok((one, two))
        },
    )
    .await
    .expect("both drops commit");
    assert!(dropped.0.handoff.is_none() && dropped.1.handoff.is_none());

    for task in [&open, &closed] {
        let after = fixture.task_row(task.id).await;
        assert_eq!(after.current_handoff_id, None);
        assert_eq!(
            untouched(&after),
            untouched(task),
            "a terminal task stays closed"
        );
    }
}

#[tokio::test]
async fn a_refusal_rolls_back_every_drop_of_the_mutation() {
    let fixture = Fixture::create("drop-many-refused").await;
    let (published, handoff_id) = with_handoff(&fixture, "open work", "ready").await;
    let bare = fixture.task("never published", "ready").await;
    let user_id = fixture.user.id;

    let refused = in_mutation(
        &fixture.app.pool,
        fixture.project.id,
        TaskActor::User { user_id },
        async |m| {
            drop_handoff(m, published.id, user_id, "rolled back").await?;
            drop_handoff(m, bare.id, user_id, "rolled back").await
        },
    )
    .await;
    assert!(
        matches!(&refused, Err(Error::Conflict(message)) if message == NO_CURRENT_HANDOFF),
        "{refused:?}",
    );

    assert_eq!(
        fixture.task_row(published.id).await.current_handoff_id,
        Some(handoff_id),
        "the first drop rolled back with the second",
    );
}
