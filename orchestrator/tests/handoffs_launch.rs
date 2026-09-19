//! Launching a session for a task whose hand-off was published through the
//! real protocol (`SPEC.md`, "Sessions" and "Code hand-offs and review";
//! `ARCHITECTURE.md`, "Task tracker" → "Launching a session for a task").
//!
//! `tests/sessions_api.rs` asserts the launch against hand-off rows written
//! straight into the tables. What is asserted here is the same launch over
//! hand-offs that really were published — `PUT /api/projects/{pid}/tasks/{id}`
//! with a `handoff` body, revision and forward, against real session work
//! clones and a real project repository — so that the pin the launcher clones
//! at is the one the publication produced:
//!
//! - a forwarded hand-off is the session's `handoff_id` and its 40-hex commit
//!   the session's `base_ref`, the work clone starts at that commit on
//!   `session/<sid>`, and the task is claimed by the new session;
//! - an explicit `base_ref` overrides it — `handoff_id` null, the disclosure
//!   paragraph still in the generated message, plus the sentence naming
//!   `refs/handoffs/<id>` — including when it names the hand-off commit itself;
//! - the disclosure reports the *current* row's review status and the
//!   hand-off's own comment, not the task's latest;
//! - a hand-off superseded after the task was read is never the base, because
//!   the selection happens under the task row lock;
//! - ending the session releases the lease and leaves the hand-off alone.
//!
//! Git is never mocked (`CLAUDE.md`, "Testing expectations"): the fixture is
//! `common::handoffs::Fixture`, and the launcher's clone is a real one against
//! the project repository. The engine is `MockEngine`, so a launch records the
//! container it would have created.
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
use mars_orchestrator::git::testutil::run_git;
use mars_orchestrator::models::{ReviewStatus, Session, SessionState, Task};
use mars_orchestrator::repositories::SessionRepository;
use mars_orchestrator::tracker::{TaskDetailDto, TaskDto};
use serde_json::{Value, json};
use uuid::Uuid;

/// How long a launch, a clone or the owner's first write may take before the
/// scenario gives up with a message rather than hanging.
const PATIENCE: Duration = Duration::from_secs(30);

/// How often the polls re-read what they are waiting for.
const POLL: Duration = Duration::from_millis(25);

// ---- publishing through the endpoint ----

/// A `revision` hand-off body naming its source session.
fn revision(source_session_id: Uuid, commit: &str, comment: &str) -> Value {
    json!({
        "kind": "revision",
        "source_session_id": source_session_id,
        "commit": commit,
        "comment": comment,
    })
}

/// A `forward` hand-off body carrying a review decision.
fn forward(handoff_id: Uuid, review: &str, comment: &str) -> Value {
    json!({
        "kind": "forward",
        "handoff_id": handoff_id,
        "review": review,
        "comment": comment,
    })
}

impl Fixture {
    /// `PUT /api/projects/{pid}/tasks/{id}` with this hand-off, expecting the
    /// publication to succeed, and answer the hand-off it made current.
    async fn publish(
        &self,
        user: &AuthenticatedUser,
        task: &Task,
        state: &str,
        handoff: Value,
    ) -> HandoffSummary {
        let response = self
            .app
            .put_as(
                user,
                &format!("/api/projects/{}/tasks/{}", self.project.id, task.id),
            )
            .json(&json!({ "state": state, "handoff": handoff }))
            .await;
        response.assert_status(StatusCode::OK);

        let published = response.json::<TaskDto>();
        let current = published
            .handoff
            .expect("a publication leaves the task with a current hand-off");

        HandoffSummary {
            id: current.id,
            commit: current.commit,
            review_status: current.review_status,
        }
    }

    /// A revision of `commit`, published into `review`.
    async fn publish_revision(
        &self,
        user: &AuthenticatedUser,
        task: &Task,
        source_session_id: Uuid,
        commit: &str,
        comment: &str,
    ) -> HandoffSummary {
        self.publish(
            user,
            task,
            "review",
            revision(source_session_id, commit, comment),
        )
        .await
    }

    /// The task as the board reads it.
    async fn read_task(&self, user: &AuthenticatedUser, task_id: Uuid) -> TaskDetailDto {
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

    /// An ordinary comment on the task, which is not a hand-off comment.
    async fn comment(&self, user: &AuthenticatedUser, task_id: Uuid, body: &str) {
        let response = self
            .app
            .post_as(
                user,
                &format!("/api/projects/{}/tasks/{task_id}/comments", self.project.id),
            )
            .json(&json!({ "body": body }))
            .await;
        response.assert_status(StatusCode::CREATED);
    }

    /// `POST /api/projects/{pid}/sessions` with `body`, expecting 201.
    async fn launch(&self, user: &AuthenticatedUser, body: &Value) -> Value {
        let response = self
            .app
            .post_as(user, &format!("/api/projects/{}/sessions", self.project.id))
            .json(body)
            .await;
        response.assert_status(StatusCode::CREATED);

        response.json::<Value>()
    }

    /// An ephemeral profile of this project, for the `-p` prompt scenario.
    async fn ephemeral_profile(&self, user: &AuthenticatedUser) -> Uuid {
        let response = self
            .app
            .post_as(user, &format!("/api/projects/{}/profiles", self.project.id))
            .json(&json!({ "name": "implementer", "kind": "ephemeral" }))
            .await;
        response.assert_status(StatusCode::CREATED);

        id_of(&response.json::<Value>())
    }

    /// The session row as it stands.
    async fn session(&self, id: Uuid) -> Session {
        SessionRepository::new(&self.app.pool)
            .find(id)
            .await
            .expect("the session reads")
            .expect("the session exists")
    }
}

/// The parts of a published hand-off every scenario here names.
#[derive(Debug, Clone)]
struct HandoffSummary {
    id: Uuid,
    commit: String,
    review_status: ReviewStatus,
}

// ---- waiting ----

/// The `id` of a resource body, as a [`Uuid`].
fn id_of(body: &Value) -> Uuid {
    body["id"]
        .as_str()
        .expect("a resource carries an id")
        .parse()
        .expect("the id is a uuid")
}

/// Poll until `predicate` holds, or fail naming what never happened.
async fn wait_for(what: &str, mut predicate: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + PATIENCE;

    while !predicate() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "{what} did not happen within {PATIENCE:?}",
        );
        tokio::time::sleep(POLL).await;
    }
}

/// Wait until the session is `running`, so its clone and container exist.
async fn wait_until_running(fixture: &Fixture, id: Uuid) -> Session {
    let deadline = tokio::time::Instant::now() + PATIENCE;

    loop {
        let session = fixture.session(id).await;
        if session.state == SessionState::Running {
            return session;
        }

        assert!(
            tokio::time::Instant::now() < deadline,
            "the session did not start within {PATIENCE:?}: {:?} {:?}",
            session.state,
            session.error,
        );
        tokio::time::sleep(POLL).await;
    }
}

/// The text of the session's first `user_message`, which is the generated one.
async fn first_message(fixture: &Fixture, id: Uuid) -> String {
    let deadline = tokio::time::Instant::now() + PATIENCE;

    loop {
        let payload = sqlx::query_scalar::<_, Value>(
            "SELECT payload FROM events WHERE session_id = $1 AND kind = 'user_message' \
             ORDER BY seq LIMIT 1",
        )
        .bind(id)
        .fetch_optional(&fixture.app.pool)
        .await
        .expect("the events are readable");

        if let Some(payload) = payload {
            assert_eq!(
                payload["user_id"],
                Value::Null,
                "nobody typed the generated message",
            );
            return payload["text"]
                .as_str()
                .expect("a message carries its text")
                .to_string();
        }

        assert!(
            tokio::time::Instant::now() < deadline,
            "no generated message within {PATIENCE:?}",
        );
        tokio::time::sleep(POLL).await;
    }
}

/// The `HEAD` commit and branch of a session's work clone.
async fn checkout(fixture: &Fixture, id: Uuid) -> (String, String) {
    let work = fixture.paths().session_work(id);

    (
        run_git(&work, &["rev-parse", "HEAD"])
            .await
            .trim()
            .to_string(),
        run_git(&work, &["rev-parse", "--abbrev-ref", "HEAD"])
            .await
            .trim()
            .to_string(),
    )
}

/// The disclosure paragraph the generated message owes a hand-off.
fn disclosure(handoff: &HandoffSummary, source_session_id: Uuid, comment: &str) -> String {
    format!(
        "Current hand-off {} from session {source_session_id} on branch session/{source_session_id} \
         at commit {} (review: {}). Hand-off comment: {comment}",
        handoff.id, handoff.commit, handoff.review_status,
    )
}

/// `POST /api/sessions/{id}/end`.
async fn end(fixture: &Fixture, user: &AuthenticatedUser, id: Uuid) -> TestResponse {
    fixture
        .app
        .post_as(user, &format!("/api/sessions/{id}/end"))
        .await
}

// ---- the pinned base ----

#[tokio::test]
async fn a_forwarded_hand_off_is_the_pinned_base_and_the_claim() {
    let fixture = Fixture::create("launch-forwarded").await;
    let user = fixture.signed_in();
    let (source, commit) = fixture.session_with_commit("feat: the parser").await;
    let task = fixture.task("continue the parser", "ready").await;

    let revision = fixture
        .publish_revision(&user, &task, source, &commit, "first pass")
        .await;
    let forwarded = fixture
        .publish(
            &user,
            &task,
            "merge",
            forward(revision.id, "approved", "looks good"),
        )
        .await;

    assert_eq!(forwarded.commit, commit, "a forward keeps the commit");
    assert_ne!(forwarded.id, revision.id, "a forward is its own record");

    let body = fixture
        .launch(
            &user,
            &json!({ "profile_id": fixture.profile_id, "task_id": task.id }),
        )
        .await;
    let id = id_of(&body);

    // The session starts from the *current* hand-off, at its full object id
    // (`SPEC.md`, "Sessions"; `docs/data-model.md`, `sessions.handoff_id`).
    assert_eq!(body["handoff_id"], json!(forwarded.id));
    assert_eq!(body["base_ref"], json!(commit));
    assert_eq!(commit.len(), 40, "a hand-off pins a full object id");

    wait_until_running(&fixture, id).await;
    let (head, branch) = checkout(&fixture, id).await;
    assert_eq!(
        head, commit,
        "the clone did not start at the hand-off commit"
    );
    assert_eq!(branch, format!("session/{id}"));

    // And the claim committed with the session row.
    let task = fixture.read_task(&user, task.id).await.task;
    assert_eq!(task.lease_holder_session_id, Some(id));
    assert_eq!(task.attempts, 1);
    assert_eq!(
        task.handoff.map(|current| current.id),
        Some(forwarded.id),
        "a launch does not move the hand-off",
    );
}

#[tokio::test]
async fn an_explicit_base_overrides_the_published_hand_off_and_says_so() {
    let fixture = Fixture::create("launch-override").await;
    let user = fixture.signed_in();
    let (source, commit) = fixture.session_with_commit("feat: the parser").await;
    let task = fixture.task("review the parser", "ready").await;

    let revision = fixture
        .publish_revision(&user, &task, source, &commit, "ready for review")
        .await;

    let body = fixture
        .launch(
            &user,
            &json!({
                "profile_id": fixture.profile_id,
                "task_id": task.id,
                "base_ref": "main",
            }),
        )
        .await;
    let id = id_of(&body);

    assert_eq!(body["base_ref"], json!("main"));
    assert_eq!(
        body["handoff_id"],
        Value::Null,
        "an explicit base records no hand-off",
    );

    // The revision is still disclosed — the agent needs to know the work
    // exists — and the override is named with the ref to fetch it from.
    let text = first_message(&fixture, id).await;
    assert!(
        text.contains(&disclosure(&revision, source, "ready for review")),
        "{text}",
    );
    assert!(
        text.ends_with(&format!(
            "\n\nYour checkout starts from main, not from the hand-off commit; fetch \
             refs/handoffs/{} before continuing that work.",
            revision.id,
        )),
        "{text}",
    );

    // And the checkout really is the integration head, not the revision.
    wait_until_running(&fixture, id).await;
    let (head, _) = checkout(&fixture, id).await;
    assert_ne!(head, commit, "the explicit base did not win");
}

#[tokio::test]
async fn an_explicit_base_at_the_hand_off_commit_still_records_no_hand_off() {
    let fixture = Fixture::create("launch-same-base").await;
    let user = fixture.signed_in();
    let (source, commit) = fixture.session_with_commit("feat: the parser").await;
    let task = fixture.task("continue the parser", "ready").await;

    let revision = fixture
        .publish_revision(&user, &task, source, &commit, "first pass")
        .await;

    let body = fixture
        .launch(
            &user,
            &json!({
                "profile_id": fixture.profile_id,
                "task_id": task.id,
                "base_ref": commit,
            }),
        )
        .await;
    let id = id_of(&body);

    // An explicit base is an override by definition, whatever it names: the
    // session was not started *from the hand-off*, so it records none
    // (`SPEC.md`, "Sessions").
    assert_eq!(body["base_ref"], json!(commit));
    assert_eq!(body["handoff_id"], Value::Null);

    let text = first_message(&fixture, id).await;
    assert!(
        text.ends_with(&format!(
            "\n\nYour checkout starts from {commit}, not from the hand-off commit; fetch \
             refs/handoffs/{} before continuing that work.",
            revision.id,
        )),
        "{text}",
    );

    // The commit is the same one either way, so the clone is at it regardless.
    wait_until_running(&fixture, id).await;
    let (head, _) = checkout(&fixture, id).await;
    assert_eq!(head, commit);
}

// ---- what the message discloses ----

#[tokio::test]
async fn the_message_discloses_the_current_review_and_the_hand_off_comment() {
    let fixture = Fixture::create("launch-disclosure").await;
    let user = fixture.signed_in();
    let (source, commit) = fixture.session_with_commit("feat: the parser").await;

    // An approved forward: the review status is the current row's, and the
    // comment is the one that hand-off was published with.
    let approved_task = fixture.task("merge the parser", "ready").await;
    let revision = fixture
        .publish_revision(&user, &approved_task, source, &commit, "first pass")
        .await;
    let forwarded = fixture
        .publish(
            &user,
            &approved_task,
            "merge",
            forward(revision.id, "approved", "approved: ship it"),
        )
        .await;
    assert_eq!(forwarded.review_status, ReviewStatus::Approved);

    // A later ordinary comment is not a hand-off comment and must not be the
    // one disclosed (`docs/data-model.md`, `task_handoffs.comment_id`).
    fixture
        .comment(&user, approved_task.id, "unrelated: remember the changelog")
        .await;

    let id = id_of(
        &fixture
            .launch(
                &user,
                &json!({ "profile_id": fixture.profile_id, "task_id": approved_task.id }),
            )
            .await,
    );
    let text = first_message(&fixture, id).await;

    assert!(
        text.starts_with(&format!(
            "You hold task #{}: merge the parser. Call get_task to read it before starting.",
            approved_task.number,
        )),
        "{text}",
    );
    assert!(
        text.contains(&disclosure(&forwarded, source, "approved: ship it")),
        "{text}",
    );
    assert!(
        !text.contains("remember the changelog"),
        "the latest task comment is not the hand-off's: {text}",
    );
    assert!(
        !text.contains("first pass"),
        "the superseded record's comment is not the current one: {text}",
    );

    // A fresh revision resets review, and the next launch says so.
    let unreviewed_task = fixture.task("continue the parser", "ready").await;
    let next = fixture
        .commit_in_work_clone(source, "PARSER.md", "second pass")
        .await;
    let revision = fixture
        .publish_revision(&user, &unreviewed_task, source, &next, "second pass")
        .await;
    assert_eq!(revision.review_status, ReviewStatus::Unreviewed);

    let id = id_of(
        &fixture
            .launch(
                &user,
                &json!({ "profile_id": fixture.profile_id, "task_id": unreviewed_task.id }),
            )
            .await,
    );
    let text = first_message(&fixture, id).await;
    assert!(
        text.contains(&disclosure(&revision, source, "second pass")),
        "{text}",
    );
}

#[tokio::test]
async fn an_ephemeral_launch_carries_the_disclosure_in_its_prompt() {
    let fixture = Fixture::create("launch-ephemeral").await;
    let user = fixture.signed_in();
    let ephemeral = fixture.ephemeral_profile(&user).await;
    let (source, commit) = fixture.session_with_commit("feat: the parser").await;
    let task = fixture.task("review the parser", "ready").await;

    let revision = fixture
        .publish_revision(&user, &task, source, &commit, "ready for review")
        .await;

    let id = id_of(
        &fixture
            .launch(
                &user,
                &json!({ "profile_id": ephemeral, "task_id": task.id }),
            )
            .await,
    );

    // An ephemeral session has no stdin to be told anything on, so the same
    // text is the head of the `-p` prompt (ADR 0003).
    wait_for("the launch created a container", || {
        fixture.app.engine().container_id_for_session(id).is_some()
    })
    .await;
    let container_id = fixture
        .app
        .engine()
        .container_id_for_session(id)
        .expect("the container was recorded");
    let cmd = fixture
        .app
        .engine()
        .spec_of(&container_id)
        .expect("the specification was recorded")
        .cmd;

    let prompt = cmd
        .iter()
        .position(|argument| argument == "-p")
        .map(|at| cmd[at + 1].clone())
        .expect("an ephemeral session runs its prompt");
    assert!(
        prompt.contains(&disclosure(&revision, source, "ready for review")),
        "{prompt}",
    );
}

// ---- selection under the lock ----

#[tokio::test]
async fn a_hand_off_superseded_after_the_read_is_never_the_base() {
    let fixture = Fixture::create("launch-superseded").await;
    let user = fixture.signed_in();
    let (source, first) = fixture.session_with_commit("feat: the parser").await;
    let task = fixture.task("continue the parser", "ready").await;

    let stale = fixture
        .publish_revision(&user, &task, source, &first, "first pass")
        .await;

    // The client reads the task, and the board moves on before it launches.
    let read = fixture.read_task(&user, task.id).await;
    assert_eq!(read.task.handoff.map(|current| current.id), Some(stale.id));

    let second = fixture
        .commit_in_work_clone(source, "PARSER.md", "second pass")
        .await;
    // Into a different state, because a publication is a hand-off *and* a
    // move (`SPEC.md`, "Code hand-offs and review").
    let current = fixture
        .publish(
            &user,
            &task,
            "merge",
            revision(source, &second, "second pass"),
        )
        .await;
    assert_ne!(current.commit, stale.commit);

    let body = fixture
        .launch(
            &user,
            &json!({ "profile_id": fixture.profile_id, "task_id": task.id }),
        )
        .await;
    let id = id_of(&body);

    // The hand-off is selected from the locked row, not from what the client
    // read (`ARCHITECTURE.md`, "Task tracker" → "Launching a session for a
    // task").
    assert_eq!(body["handoff_id"], json!(current.id));
    assert_eq!(body["base_ref"], json!(second));

    wait_until_running(&fixture, id).await;
    let (head, _) = checkout(&fixture, id).await;
    assert_eq!(head, second, "the clone started from the superseded commit");

    let text = first_message(&fixture, id).await;
    assert!(
        text.contains(&disclosure(&current, source, "second pass")),
        "{text}",
    );
    assert!(!text.contains(&stale.id.to_string()), "{text}");
}

// ---- the end of the session ----

#[tokio::test]
async fn ending_the_session_releases_the_lease_and_leaves_the_hand_off_alone() {
    let fixture = Fixture::create("launch-end").await;
    let user = fixture.signed_in();
    let (source, commit) = fixture.session_with_commit("feat: the parser").await;
    let task = fixture.task("continue the parser", "ready").await;

    let revision = fixture
        .publish_revision(&user, &task, source, &commit, "first pass")
        .await;
    let forwarded = fixture
        .publish(
            &user,
            &task,
            "merge",
            forward(revision.id, "approved", "looks good"),
        )
        .await;

    let id = id_of(
        &fixture
            .launch(
                &user,
                &json!({ "profile_id": fixture.profile_id, "task_id": task.id }),
            )
            .await,
    );
    wait_until_running(&fixture, id).await;

    let before = fixture.read_task(&user, task.id).await;
    let history_before = before.handoffs.iter().map(|row| row.id).collect::<Vec<_>>();

    let response = end(&fixture, &user, id).await;
    response.assert_status(StatusCode::OK);

    // The lease goes back, and nothing about the hand-off moves: ending a
    // session publishes nothing (`ARCHITECTURE.md`, "Task tracker").
    let deadline = tokio::time::Instant::now() + PATIENCE;
    let released = loop {
        let detail = fixture.read_task(&user, task.id).await;
        if detail.task.lease_holder_session_id.is_none() {
            break detail;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the lease was still held {PATIENCE:?} after the session ended",
        );
        tokio::time::sleep(POLL).await;
    };

    let current = released
        .task
        .handoff
        .expect("the hand-off is still current");
    assert_eq!(current.id, forwarded.id);
    assert_eq!(current.commit, commit);
    assert_eq!(current.review_status, ReviewStatus::Approved);
    assert_eq!(
        released
            .handoffs
            .iter()
            .map(|row| row.id)
            .collect::<Vec<_>>(),
        history_before,
        "ending a session wrote a hand-off",
    );

    // And the session it released kept the hand-off it was launched from.
    assert_eq!(fixture.session(id).await.handoff_id, Some(forwarded.id));
}
