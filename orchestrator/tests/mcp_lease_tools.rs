//! The four tools that change who holds a task or add to its conversation:
//! `claim`, `release`, `comment` and `needs_human` (`SPEC.md`, "MCP tool
//! contracts"; `CLAUDE.md`, "Testing expectations": MCP tests drive the tool
//! handlers through the `rmcp` server in-process with a session bearer token).
//!
//! The tracker's own rules are asserted in `tests/tracker_leases.rs` and
//! `tests/tracker_escalation_email.rs`. What is pinned here is the *boundary*:
//! that an agent calling over MCP reaches those rules with the right actor,
//! gets the documented message and code for each refusal, and — the promise
//! the intro paragraph of "MCP tool contracts" makes about every tool — that
//! a successful call commits its events and its `task_sessions` link together
//! while a rejected one writes nothing at all (ADR 0030).
//!
//! Scenario by scenario:
//!
//! - a claim in a served state takes the lease, counts the attempt, emits one
//!   `claimed` with actor `session`, links the session and answers with the
//!   task's current hand-off, which nothing resets;
//! - a second session's claim, a claim of a blocked task and a claim of a
//!   task in an unserved state are the two documented conflicts, told apart
//!   by their messages;
//! - two clients claiming at once leave exactly one winner and one
//!   `claimed` event;
//! - a release by a non-holder is refused; a release by the holder comments,
//!   clears the lease and keeps the state; a release at `max_attempts` moves
//!   the task to the human state and sends exactly one email, to the assignee
//!   or to the administrators who want one;
//! - a comment needs no lease and still links the session;
//! - `needs_human` refuses somebody else's task, escalates an unheld one, and
//!   on a task already waiting for a person records the reason without a
//!   second `escalated` event, a second email or a lost attempt count;
//! - and every rejection above leaves the events, the links, the comments,
//!   `attempts` and `updated_at` exactly as they were.
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use chrono::{DateTime, Utc};
use common::TestApp;
use common::mcp::{McpClient, code, refused, task_of};
use serde_json::{Value, json};
use uuid::Uuid;

use mars_orchestrator::email::EmailMessage;
use mars_orchestrator::events::{TaskActor, TaskEvent, TaskEventKind};
use mars_orchestrator::models::{
    HandoffCaller, NewTask, SessionState, Task, TaskDependencyKind, User,
};
use mars_orchestrator::repositories::TaskRepository;
use mars_orchestrator::tracker::graph::recompute_blocked;
use mars_orchestrator::tracker::{ReviewCarry, TaskDto, TrackerMutation};

/// The project default the escalation scenarios count against (`SPEC.md`,
/// "Projects").
const MAX_ATTEMPTS: i16 = 3;

/// Not a real revision: an obviously fake full object id for the hand-off a
/// claim has to carry back (`CLAUDE.md`, rule 3).
const HANDOFF_COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

/// The two conflicts a claim can answer with (`SPEC.md`, `claim`).
const NOT_CLAIMABLE: &str = "task is not claimable";
const NOT_SERVED: &str = "task is not in a state this profile serves";
/// The conflict a release by a non-holder answers with.
const NOT_HELD_BY_SESSION: &str = "task is not held by this session";
/// The conflict `needs_human` answers on somebody else's task.
const HELD_BY_ANOTHER: &str = "task is held by another session";

/// A project with the documented default states and a profile serving
/// `ready`, which is what an agent's queue looks like.
struct Fixture {
    project_id: Uuid,
    profile_id: Uuid,
}

async fn seed(app: &TestApp) -> Fixture {
    let (project_id, profile_id) = app.seed_mcp_project().await;

    let repository = TaskRepository::new(&app.pool);
    let mut mutation = TrackerMutation::begin(&app.pool, project_id, TaskActor::System)
        .await
        .expect("the mutation opens");
    repository
        .insert_default_states(mutation.conn(), project_id)
        .await
        .expect("the default states insert");
    repository
        .set_profile_states_by_name(
            mutation.conn(),
            project_id,
            profile_id,
            &["ready".to_string()],
        )
        .await
        .expect("the profile serves ready");
    mutation.commit().await.expect("the mutation commits");

    Fixture {
        project_id,
        profile_id,
    }
}

/// A live session of the fixture's profile, and a client holding its token.
async fn session(app: &TestApp, fixture: &Fixture) -> (McpClient, Uuid) {
    let seeded = app
        .seed_mcp_session(
            fixture.project_id,
            fixture.profile_id,
            SessionState::Running,
        )
        .await;
    let client = McpClient::connect(app, &seeded.token)
        .await
        .expect("a running session's token authenticates");

    (client, seeded.session_id)
}

/// A task of this project in the named state.
async fn task(app: &TestApp, fixture: &Fixture, title: &str, state: &str) -> Task {
    task_for(app, fixture, title, state, None).await
}

/// The same, assigned to a user, which is who an escalation email goes to.
async fn task_for(
    app: &TestApp,
    fixture: &Fixture,
    title: &str,
    state: &str,
    assignee: Option<&User>,
) -> Task {
    let project_id = fixture.project_id;
    let mut new = NewTask::new(project_id, title).expect("the title parses");
    new.state_id = Some(state_id(app, project_id, state).await);
    new.assignee_user_id = assignee.map(|user| user.id);

    let mut mutation = TrackerMutation::begin(&app.pool, project_id, TaskActor::System)
        .await
        .expect("the mutation opens");
    let inserted = TaskRepository::new(&app.pool)
        .insert_task(mutation.conn(), project_id, &new)
        .await
        .expect("the task inserts");
    mutation.commit().await.expect("the mutation commits");

    inserted
}

/// The id of a project's state by name.
async fn state_id(app: &TestApp, project_id: Uuid, name: &str) -> Uuid {
    TaskRepository::new(&app.pool)
        .find_state_by_name(project_id, name)
        .await
        .expect("the state reads")
        .expect("the project has this state")
        .id
}

/// A hand-off record pointed at by `tasks.current_handoff_id`, so a claim has
/// something to carry back.
///
/// Published the way an agent publishes one: the session takes the lease,
/// hands its work off, and the task comes back to the column it was in
/// (`common::tracker::handoff_in_place`). The lease is gone again afterwards,
/// because a publication ends the holder's hold.
async fn with_handoff(app: &TestApp, fixture: &Fixture, task_id: Uuid, session_id: Uuid) -> Uuid {
    common::tracker::hold(&app.pool, fixture.project_id, task_id, session_id).await;

    let (_, handoff_id) = common::tracker::handoff_in_place(
        &app.pool,
        fixture.project_id,
        task_id,
        common::tracker::Handoff {
            source_session_id: Some(session_id),
            source_branch: "session/one",
            commit: HANDOFF_COMMIT,
            comment: "the work so far",
            target_state: "",
            caller: HandoffCaller::Session { session_id },
            review: ReviewCarry::Fresh,
        },
    )
    .await;

    handoff_id
}

/// Put `attempts` where a release is the one that runs out of them.
///
/// A claim is the only thing that raises the counter, so the count is that
/// many claims, each given back by a user (`common::tracker::with_attempts`).
async fn set_attempts(
    app: &TestApp,
    project_id: Uuid,
    task_id: Uuid,
    session_id: Uuid,
    attempts: i16,
) {
    common::tracker::with_attempts(&app.pool, project_id, task_id, session_id, attempts).await;
}

/// Make `dependant` wait for `prerequisite`, and store the flag that implies.
async fn depends_on(app: &TestApp, project_id: Uuid, dependant: Uuid, prerequisite: Uuid) {
    let mut mutation = TrackerMutation::begin(&app.pool, project_id, TaskActor::System)
        .await
        .expect("the mutation opens");
    TaskRepository::new(&app.pool)
        .insert_dependency(
            mutation.conn(),
            project_id,
            dependant,
            prerequisite,
            TaskDependencyKind::Blocks,
        )
        .await
        .expect("the edge inserts");
    recompute_blocked(&mut mutation, &[dependant])
        .await
        .expect("the flag is recomputed");
    mutation.commit().await.expect("the mutation commits");
}

/// Turn a user's `notify_email` off, as `PATCH /users/me` would.
async fn opt_out(app: &TestApp, user: &User) {
    sqlx::query("UPDATE users SET notify_email = FALSE WHERE id = $1")
        .bind(user.id)
        .execute(&app.pool)
        .await
        .expect("the opt-out writes");
}

/// The task as it is committed.
async fn read(app: &TestApp, project_id: Uuid, task_id: Uuid) -> TaskDto {
    TaskRepository::new(&app.pool)
        .load_task_dto(project_id, task_id)
        .await
        .expect("the task reads")
        .expect("the task is in this project")
}

/// Every committed event of the project, oldest first.
/// Where the stream stands right now.
///
/// The cursor an assertion about "what this call emitted" starts from: a
/// lease is a claim's and `attempts` is what claims left behind
/// (`common::tracker`), so an arrangement really writes `claimed` and
/// `released` events of its own.
async fn since(app: &TestApp, project_id: Uuid) -> i64 {
    TaskRepository::new(&app.pool)
        .max_task_event_seq(project_id)
        .await
        .expect("the cursor reads")
}

async fn events(app: &TestApp, project_id: Uuid, after: i64) -> Vec<TaskEvent> {
    TaskRepository::new(&app.pool)
        .list_task_events_after(project_id, after, 100)
        .await
        .expect("the events read")
        .into_iter()
        .map(|row| TaskEvent::from_row(row).expect("the row is a documented event"))
        .collect()
}

/// The kinds of those events, which is usually the whole assertion.
async fn event_kinds(app: &TestApp, project_id: Uuid, after: i64) -> Vec<TaskEventKind> {
    events(app, project_id, after)
        .await
        .into_iter()
        .map(|event| event.kind)
        .collect()
}

/// What nothing may change when a call is rejected (ADR 0030).
#[derive(Debug, PartialEq)]
struct Audit {
    events: usize,
    links: i64,
    comments: i64,
    attempts: i16,
    updated_at: DateTime<Utc>,
    lease_holder_session_id: Option<Uuid>,
    state: String,
}

async fn audit(app: &TestApp, project_id: Uuid, task_id: Uuid) -> Audit {
    let task = read(app, project_id, task_id).await;

    Audit {
        events: events(app, project_id, 0).await.len(),
        links: count(
            app,
            "SELECT COUNT(*) FROM task_sessions WHERE task_id = $1",
            task_id,
        )
        .await,
        comments: count(
            app,
            "SELECT COUNT(*) FROM task_comments WHERE task_id = $1",
            task_id,
        )
        .await,
        attempts: task.attempts,
        updated_at: task.updated_at,
        lease_holder_session_id: task.lease_holder_session_id,
        state: task.state,
    }
}

/// One counting query over a task id. The rows are what no interface exposes:
/// the number of `task_sessions` links and of comments on a task.
async fn count(app: &TestApp, sql: &'static str, task_id: Uuid) -> i64 {
    sqlx::query_scalar::<_, i64>(sql)
        .bind(task_id)
        .fetch_one(&app.pool)
        .await
        .expect("the count runs")
}

/// The one message that went out, or a failure naming how many did.
fn only_message(app: &TestApp) -> EmailMessage {
    let sent = app.mock_email().sent();
    assert_eq!(sent.len(), 1, "exactly one message: {sent:?}");
    sent.into_iter().next().expect("the message is there")
}

// ---- claim ----

#[tokio::test]
async fn a_claim_takes_the_lease_links_the_session_and_carries_the_handoff() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;
    let (client, session_id) = session(&app, &fixture).await;

    let subject = task(&app, &fixture, "implement it", "ready").await;
    let handoff_id = with_handoff(&app, &fixture, subject.id, session_id).await;
    let before = events(&app, fixture.project_id, 0).await.len();

    let claimed = task_of(
        &client
            .call("claim", json!({ "task": subject.number }))
            .await
            .expect("the claim succeeds"),
    );

    assert_eq!(claimed.lease_holder_session_id, Some(session_id));
    assert_eq!(claimed.attempts, 1);
    assert_eq!(claimed.state, "ready");
    // "The returned task includes its hand-off", so the agent can fetch
    // `refs/handoffs/<handoff.id>` — and claiming reset nothing.
    let carried = claimed.handoff.as_ref().expect("the hand-off is carried");
    assert_eq!(carried.id, handoff_id);
    assert_eq!(carried.commit, HANDOFF_COMMIT);

    let written = events(&app, fixture.project_id, 0).await;
    assert_eq!(written.len(), before + 1, "a claim emits exactly one event");
    let claimed_event = written.last().expect("the event is there");
    assert_eq!(claimed_event.kind, TaskEventKind::Claimed);
    assert_eq!(claimed_event.task_id, Some(subject.id));
    assert_eq!(claimed_event.actor, TaskActor::Session { session_id });

    // The claim is the session working on the task, so it is linked to it.
    let holder: Uuid =
        sqlx::query_scalar("SELECT session_id FROM task_sessions WHERE task_id = $1")
            .bind(subject.id)
            .fetch_one(&app.pool)
            .await
            .expect("exactly one session is linked");
    assert_eq!(holder, session_id);
}

#[tokio::test]
async fn a_task_another_session_holds_is_not_claimable() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;
    let (first, holder) = session(&app, &fixture).await;
    let (second, _) = session(&app, &fixture).await;

    let subject = task(&app, &fixture, "implement it", "ready").await;
    first
        .call("claim", json!({ "task": subject.number }))
        .await
        .expect("the first claim succeeds");

    let before = audit(&app, fixture.project_id, subject.id).await;
    let err = refused(
        second
            .call("claim", json!({ "task": subject.number }))
            .await,
    );

    assert_eq!(code(&err), "conflict");
    assert_eq!(err.message, NOT_CLAIMABLE);
    assert_eq!(audit(&app, fixture.project_id, subject.id).await, before);
    assert_eq!(before.lease_holder_session_id, Some(holder));
}

#[tokio::test]
async fn a_task_outside_the_profiles_served_states_says_so() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;
    let (client, _) = session(&app, &fixture).await;

    // The profile serves `ready`, and this task is still being planned.
    let subject = task(&app, &fixture, "not planned yet", "backlog").await;
    let before = audit(&app, fixture.project_id, subject.id).await;

    let err = refused(
        client
            .call("claim", json!({ "task": subject.number }))
            .await,
    );

    assert_eq!(code(&err), "conflict");
    assert_eq!(err.message, NOT_SERVED);
    assert_eq!(audit(&app, fixture.project_id, subject.id).await, before);
}

#[tokio::test]
async fn a_blocked_task_is_not_claimable() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;
    let (client, _) = session(&app, &fixture).await;

    let prerequisite = task(&app, &fixture, "do this first", "ready").await;
    let subject = task(&app, &fixture, "then this", "ready").await;
    depends_on(&app, fixture.project_id, subject.id, prerequisite.id).await;

    let before = audit(&app, fixture.project_id, subject.id).await;
    let err = refused(
        client
            .call("claim", json!({ "task": subject.number }))
            .await,
    );

    // In its served state, and still refused: the statement decides.
    assert_eq!(code(&err), "conflict");
    assert_eq!(err.message, NOT_CLAIMABLE);
    assert_eq!(audit(&app, fixture.project_id, subject.id).await, before);
}

#[tokio::test]
async fn two_concurrent_claims_leave_exactly_one_winner() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;
    let (first, first_session) = session(&app, &fixture).await;
    let (second, second_session) = session(&app, &fixture).await;

    let subject = task(&app, &fixture, "implement it", "ready").await;
    let args = json!({ "task": subject.number });

    let (one, two) = tokio::join!(
        first.call("claim", args.clone()),
        second.call("claim", args.clone()),
    );

    let (winner, loser) = match (one, two) {
        (Ok(won), Err(lost)) => (task_of(&won), lost),
        (Err(lost), Ok(won)) => (task_of(&won), lost),
        (Ok(a), Ok(b)) => panic!("both claims won: {a} and {b}"),
        (Err(a), Err(b)) => panic!("both claims lost: {a:?} and {b:?}"),
    };

    assert_eq!(code(&loser), "conflict");
    assert_eq!(loser.message, NOT_CLAIMABLE);
    assert!(
        [first_session, second_session]
            .contains(&winner.lease_holder_session_id.expect("the winner holds it")),
    );
    assert_eq!(winner.attempts, 1);

    let claims = event_kinds(&app, fixture.project_id, 0)
        .await
        .into_iter()
        .filter(|kind| *kind == TaskEventKind::Claimed)
        .count();
    assert_eq!(claims, 1, "exactly one claim was written");
    assert_eq!(
        count(
            &app,
            "SELECT COUNT(*) FROM task_sessions WHERE task_id = $1",
            subject.id,
        )
        .await,
        1,
    );
}

// ---- release ----

#[tokio::test]
async fn a_release_by_a_session_that_does_not_hold_the_task_is_a_conflict() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;
    let (holder_client, _) = session(&app, &fixture).await;
    let (other, _) = session(&app, &fixture).await;

    let subject = task(&app, &fixture, "implement it", "ready").await;
    holder_client
        .call("claim", json!({ "task": subject.number }))
        .await
        .expect("the claim succeeds");

    let before = audit(&app, fixture.project_id, subject.id).await;
    let err = refused(
        other
            .call(
                "release",
                json!({ "task": subject.number, "reason": "not mine" }),
            )
            .await,
    );

    assert_eq!(code(&err), "conflict");
    assert_eq!(err.message, NOT_HELD_BY_SESSION);
    // In particular no comment was written: the refusal precedes the row.
    assert_eq!(audit(&app, fixture.project_id, subject.id).await, before);
}

#[tokio::test]
async fn a_release_comments_clears_the_lease_and_keeps_the_state() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;
    let (client, session_id) = session(&app, &fixture).await;

    let subject = task(&app, &fixture, "implement it", "ready").await;
    client
        .call("claim", json!({ "task": subject.number }))
        .await
        .expect("the claim succeeds");

    let released = task_of(
        &client
            .call(
                "release",
                json!({ "task": subject.number, "reason": "the credentials are missing" }),
            )
            .await
            .expect("the release succeeds"),
    );

    assert_eq!(released.lease_holder_session_id, None);
    assert_eq!(
        released.state, "ready",
        "the state is the queue, not the lease"
    );
    // `attempts` survives a release: only a state change resets it.
    assert_eq!(released.attempts, 1);

    let kinds = event_kinds(&app, fixture.project_id, 0).await;
    assert_eq!(
        kinds,
        [
            TaskEventKind::Claimed,
            TaskEventKind::Commented,
            TaskEventKind::Released,
        ],
    );
    let last = events(&app, fixture.project_id, 0)
        .await
        .pop()
        .expect("the release is there");
    assert_eq!(last.reason.as_deref(), Some("given_back"));
    assert_eq!(last.actor, TaskActor::Session { session_id });

    // The reason is the session's own comment, so the next worker reads it.
    let body: String = sqlx::query_scalar(
        "SELECT body FROM task_comments WHERE task_id = $1 AND author_session_id = $2",
    )
    .bind(subject.id)
    .bind(session_id)
    .fetch_one(&app.pool)
    .await
    .expect("the comment is there");
    assert_eq!(body, "the credentials are missing");
}

#[tokio::test]
async fn a_release_at_the_attempt_limit_escalates_and_tells_the_assignee() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;
    let (client, session_id) = session(&app, &fixture).await;

    let assignee = app
        .insert_user("assignee", "assignee@example.test", false, false)
        .await;
    // Would be told if the task had no assignee, and is not, because it has.
    app.insert_user("admin", "admin@example.test", true, false)
        .await;

    let subject = task_for(&app, &fixture, "rotate the key", "ready", Some(&assignee)).await;
    // One short of the limit; the claim below is the attempt that reaches it.
    set_attempts(
        &app,
        fixture.project_id,
        subject.id,
        session_id,
        MAX_ATTEMPTS - 1,
    )
    .await;
    client
        .call("claim", json!({ "task": subject.number }))
        .await
        .expect("the claim succeeds");
    app.mock_email().clear();

    let released = task_of(
        &client
            .call(
                "release",
                json!({ "task": subject.number, "reason": "I cannot get past the login" }),
            )
            .await
            .expect("the release succeeds"),
    );

    assert_eq!(released.state, "needs_human");
    assert_eq!(released.lease_holder_session_id, None);
    let reason = released
        .needs_human_reason
        .as_deref()
        .expect("the reason is stored");
    assert!(reason.contains("I cannot get past the login"), "{reason}");

    let kinds = event_kinds(&app, fixture.project_id, 0).await;
    assert!(
        kinds.contains(&TaskEventKind::Escalated),
        "the move to a person is announced: {kinds:?}",
    );

    let message = only_message(&app);
    assert_eq!(message.to, assignee.email.as_str());
    assert!(
        message.text.contains("I cannot get past the login"),
        "the reason travels with the email: {}",
        message.text,
    );
}

#[tokio::test]
async fn an_unassigned_escalation_goes_to_the_admins_who_want_email() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;
    let (client, session_id) = session(&app, &fixture).await;

    let told = app
        .insert_user("told", "told@example.test", true, false)
        .await;
    let quiet = app
        .insert_user("quiet", "quiet@example.test", true, false)
        .await;
    opt_out(&app, &quiet).await;
    // Not an administrator, so not a recipient however much they want email.
    app.insert_user("member", "member@example.test", false, false)
        .await;

    let subject = task(&app, &fixture, "decide the schema", "ready").await;
    set_attempts(
        &app,
        fixture.project_id,
        subject.id,
        session_id,
        MAX_ATTEMPTS - 1,
    )
    .await;
    client
        .call("claim", json!({ "task": subject.number }))
        .await
        .expect("the claim succeeds");
    app.mock_email().clear();

    client
        .call(
            "release",
            json!({ "task": subject.number, "reason": "this needs a product decision" }),
        )
        .await
        .expect("the release succeeds");

    let message = only_message(&app);
    assert_eq!(message.to, told.email.as_str());
}

#[tokio::test]
async fn a_release_without_a_reason_is_an_invalid_argument_and_writes_nothing() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;
    let (client, _) = session(&app, &fixture).await;

    let subject = task(&app, &fixture, "implement it", "ready").await;
    client
        .call("claim", json!({ "task": subject.number }))
        .await
        .expect("the claim succeeds");

    let before = audit(&app, fixture.project_id, subject.id).await;
    for reason in ["", "   \n "] {
        let err = refused(
            client
                .call(
                    "release",
                    json!({ "task": subject.number, "reason": reason }),
                )
                .await,
        );

        assert_eq!(code(&err), "invalid_argument");
        assert_eq!(err.message, "reason must not be empty");
    }

    assert_eq!(audit(&app, fixture.project_id, subject.id).await, before);
}

// ---- comment ----

#[tokio::test]
async fn any_session_in_the_project_may_comment_and_is_linked_to_the_task() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;
    let (holder_client, _) = session(&app, &fixture).await;
    let (other, other_session) = session(&app, &fixture).await;

    let subject = task(&app, &fixture, "implement it", "ready").await;
    holder_client
        .call("claim", json!({ "task": subject.number }))
        .await
        .expect("the claim succeeds");

    let answered = other
        .call(
            "comment",
            json!({ "task": subject.number, "body": "the migration is already applied" }),
        )
        .await
        .expect("a comment needs no lease");

    let comment = &answered["comment"];
    assert_eq!(comment["task_id"], json!(subject.id));
    assert_eq!(comment["author_session_id"], json!(other_session));
    assert_eq!(comment["author_user_id"], Value::Null);
    assert_eq!(comment["system"], json!(false));
    assert_eq!(comment["body"], json!("the migration is already applied"));

    let kinds = event_kinds(&app, fixture.project_id, 0).await;
    assert_eq!(kinds, [TaskEventKind::Claimed, TaskEventKind::Commented]);

    // Both sessions worked on it: the holder by claiming, this one by saying
    // something about it (ADR 0030).
    assert_eq!(
        count(
            &app,
            "SELECT COUNT(*) FROM task_sessions WHERE task_id = $1",
            subject.id,
        )
        .await,
        2,
    );

    // The task itself did not change, not even `updated_at`.
    assert_eq!(read(&app, fixture.project_id, subject.id).await.attempts, 1);
}

#[tokio::test]
async fn an_empty_comment_body_is_an_invalid_argument_and_writes_nothing() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;
    let (client, _) = session(&app, &fixture).await;

    let subject = task(&app, &fixture, "implement it", "ready").await;
    let before = audit(&app, fixture.project_id, subject.id).await;

    let err = refused(
        client
            .call("comment", json!({ "task": subject.number, "body": "  " }))
            .await,
    );

    assert_eq!(code(&err), "invalid_argument");
    assert_eq!(err.message, "body must not be empty");
    assert_eq!(audit(&app, fixture.project_id, subject.id).await, before);
}

// ---- needs_human ----

#[tokio::test]
async fn needs_human_on_a_task_another_session_holds_is_a_conflict() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;
    let (holder_client, _) = session(&app, &fixture).await;
    let (other, _) = session(&app, &fixture).await;

    let subject = task(&app, &fixture, "implement it", "ready").await;
    holder_client
        .call("claim", json!({ "task": subject.number }))
        .await
        .expect("the claim succeeds");

    let before = audit(&app, fixture.project_id, subject.id).await;
    let err = refused(
        other
            .call(
                "needs_human",
                json!({ "task": subject.number, "reason": "somebody else is on it" }),
            )
            .await,
    );

    assert_eq!(code(&err), "conflict");
    assert_eq!(err.message, HELD_BY_ANOTHER);
    assert_eq!(audit(&app, fixture.project_id, subject.id).await, before);
    assert!(app.mock_email().sent().is_empty());
}

#[tokio::test]
async fn needs_human_on_an_unheld_task_hands_it_over_and_sends_one_email() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;
    let (client, session_id) = session(&app, &fixture).await;
    let (_earlier, earlier_session) = session(&app, &fixture).await;

    let assignee = app
        .insert_user("assignee", "assignee@example.test", false, false)
        .await;
    let subject = task_for(
        &app,
        &fixture,
        "choose the vendor",
        "ready",
        Some(&assignee),
    )
    .await;
    // Two earlier attempts, by somebody else: this session never claimed the
    // task, and an unheld task may still be handed over.
    set_attempts(&app, fixture.project_id, subject.id, earlier_session, 2).await;
    // Those claims are the arrangement; the stream is read from here on.
    let after = since(&app, fixture.project_id).await;
    app.mock_email().clear();

    let handed = task_of(
        &client
            .call(
                "needs_human",
                json!({ "task": subject.number, "reason": "this is a budget decision" }),
            )
            .await
            .expect("an unheld task may be handed over"),
    );

    assert_eq!(handed.state, "needs_human");
    assert_eq!(
        handed.needs_human_reason.as_deref(),
        Some("this is a budget decision"),
    );
    assert_eq!(handed.lease_holder_session_id, None);
    // "releases the lease and resets `attempts`".
    assert_eq!(handed.attempts, 0);

    let kinds = event_kinds(&app, fixture.project_id, after).await;
    assert_eq!(
        kinds,
        [TaskEventKind::Commented, TaskEventKind::Escalated],
        "the reason is recorded and the move announced",
    );
    let escalated = events(&app, fixture.project_id, after)
        .await
        .pop()
        .expect("the escalation is there");
    assert_eq!(escalated.actor, TaskActor::Session { session_id });
    assert_eq!(escalated.from.as_deref(), Some("ready"));
    assert_eq!(escalated.to.as_deref(), Some("needs_human"));

    let message = only_message(&app);
    assert_eq!(message.to, assignee.email.as_str());

    // The row-level fact no interface answers: the calling session is linked
    // to the task although it never held it, beside the session whose two
    // earlier attempts arranged the counter.
    let linked: Vec<Uuid> =
        sqlx::query_scalar("SELECT session_id FROM task_sessions WHERE task_id = $1 ORDER BY 1")
            .bind(subject.id)
            .fetch_all(&app.pool)
            .await
            .expect("the links read");
    let mut expected = vec![session_id, earlier_session];
    expected.sort();
    assert_eq!(
        linked, expected,
        "the session worked on the task it handed over",
    );
}

#[tokio::test]
async fn needs_human_on_a_task_already_with_a_person_records_the_reason_only() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;
    let (client, session_id) = session(&app, &fixture).await;

    app.insert_user("admin", "admin@example.test", true, false)
        .await;

    // A task a person was already asked about, which a user then launched this
    // session on: it holds the lease in the human state.
    let subject = task(&app, &fixture, "approve the migration", "needs_human").await;
    common::tracker::hold_with_attempts(&app.pool, fixture.project_id, subject.id, session_id, 2)
        .await;
    // Those claims are the arrangement; the stream is read from here on.
    let after = since(&app, fixture.project_id).await;
    app.mock_email().clear();

    let handed = task_of(
        &client
            .call(
                "needs_human",
                json!({ "task": subject.number, "reason": "still blocked on the approval" }),
            )
            .await
            .expect("recording a second reason succeeds"),
    );

    assert_eq!(handed.state, "needs_human");
    assert_eq!(
        handed.needs_human_reason.as_deref(),
        Some("still blocked on the approval"),
    );
    assert_eq!(handed.lease_holder_session_id, None);
    // Preserved: this was not a state change, so nothing reset the counter.
    assert_eq!(handed.attempts, 2);

    let kinds = event_kinds(&app, fixture.project_id, after).await;
    assert_eq!(
        kinds,
        [
            TaskEventKind::Commented,
            TaskEventKind::Updated,
            TaskEventKind::Released,
        ],
        "no second escalation",
    );
    let released = events(&app, fixture.project_id, after)
        .await
        .pop()
        .expect("the release is there");
    assert_eq!(released.reason.as_deref(), Some("given_back"));

    assert!(
        app.mock_email().sent().is_empty(),
        "a second email would tell nobody anything new",
    );
}

// ---- ADR 0030 ----

#[tokio::test]
async fn a_rejected_call_leaves_no_event_link_or_comment_behind() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;
    let (client, _) = session(&app, &fixture).await;

    let subject = task(&app, &fixture, "implement it", "ready").await;
    client
        .call("claim", json!({ "task": subject.number }))
        .await
        .expect("the claim succeeds");

    let before = audit(&app, fixture.project_id, subject.id).await;

    // A task of no project, a claim of a task this session already holds, a
    // release of the wrong task and an empty reason: four refusals, one
    // audit.
    let unknown = refused(client.call("claim", json!({ "task": 9999 })).await);
    assert_eq!(code(&unknown), "not_found");
    assert_eq!(unknown.message, "task not found");

    let again = refused(
        client
            .call("claim", json!({ "task": subject.number }))
            .await,
    );
    assert_eq!(again.message, NOT_CLAIMABLE);

    let malformed = refused(client.call("claim", json!({ "task": -1 })).await);
    assert_eq!(code(&malformed), "invalid_argument");

    let blank = refused(
        client
            .call(
                "needs_human",
                json!({ "task": subject.number, "reason": "" }),
            )
            .await,
    );
    assert_eq!(code(&blank), "invalid_argument");

    assert_eq!(audit(&app, fixture.project_id, subject.id).await, before);
    assert!(app.mock_email().sent().is_empty());
}
