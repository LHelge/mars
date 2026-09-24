//! The round limit (`ARCHITECTURE.md`, "Task tracker" → "Rounds";
//! `docs/data-model.md`, `tasks.rounds` and `projects.max_rounds`; `SPEC.md`,
//! "Code hand-offs and review" and "TaskEvent"; ADR 0046).
//!
//! What is asserted here, each through the verb that composes it:
//!
//! - a revision publication raises `rounds` by one, and a forward does not;
//! - a move out of the human state resets it to zero, and no other move
//!   touches it — and a revision out of the human state is the first round of
//!   the fresh allowance;
//! - a session's forward recording `changes_requested` on a task at the limit
//!   goes to the human state instead: the hand-off still records the
//!   decision, the comment is written, `needs_human_reason` is `round limit
//!   reached (<rounds>/<max_rounds>): <comment>`, the event is `escalated`
//!   and not `state_changed`, and the escalation email goes out through
//!   `commit_and_notify`;
//! - below the limit, and for a user's forward at it, the requested move is
//!   made and nobody is emailed;
//! - `send_back`, the helper the auto-merge job's conflict send-back calls,
//!   applies the same rule for the system.
//!
//! `rounds` is arranged by real revision publications
//! (`tests/common/tracker.rs`, `with_rounds`), and `max_rounds` by the project
//! update `PUT /projects/{id}` writes with, lowered to two so an arrangement
//! is two publications rather than five.
//!
//! Needs a container engine; see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use common::TestApp;
use common::tracker::{Handoff, hold, in_mutation, locked, move_to, publish_handoff, with_rounds};
use mars_orchestrator::events::{TaskActor, TaskEvent, TaskEventKind};
use mars_orchestrator::git::refs;
use mars_orchestrator::models::{
    HandoffCaller, MaxRounds, NewSession, NewTask, ProfileKind, ProjectUpdate, ReviewDecision,
    ReviewStatus, User,
};
use mars_orchestrator::prelude::*;
use mars_orchestrator::repositories::{ProjectRepository, SessionRepository, TaskRepository};
use mars_orchestrator::tracker::handoffs::publish_in_transaction;
use mars_orchestrator::tracker::{
    PreparedHandoff, ReviewCarry, TaskDto, TrackerMutation, UpdateTaskInput, commit_and_notify,
    send_back,
};
use uuid::Uuid;

/// Not a real image: the stub the session tests replay a fixture transcript
/// with.
const TEST_IMAGE: &str = "localhost/mars-session-stub:test";

/// The limit this suite sets, so an arrangement at it is two publications.
const MAX_ROUNDS: i16 = 2;

/// What the reviewer asks for when it sends a task back.
const SEND_BACK: &str = "the migration still drops the index";

/// A project with the documented default states, a lowered round limit, a
/// profile to hang sessions off and an administrator to email.
struct Fixture {
    project_id: Uuid,
    profile_id: Uuid,
    admin: User,
}

async fn seed(app: &TestApp) -> Fixture {
    let pool = &app.pool;

    let project_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id, name, remote_url) VALUES ($1, $2, $3)")
        .bind(project_id)
        .bind(format!("project-{project_id}"))
        // `.invalid` can never resolve (rule 3).
        .bind("https://example.invalid/org/repo.git")
        .execute(pool)
        .await
        .expect("the project seeds");

    let mut tx = pool.begin().await.expect("a transaction begins");
    let updated = ProjectRepository::new(pool)
        .update(
            &mut tx,
            project_id,
            &ProjectUpdate {
                max_rounds: Some(MaxRounds::parse(MAX_ROUNDS).expect("the limit validates")),
                ..ProjectUpdate::default()
            },
        )
        .await
        .expect("the limit writes")
        .expect("the project exists");
    tx.commit().await.expect("the transaction commits");
    assert_eq!(updated.max_rounds, MAX_ROUNDS);

    let profile_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO agent_profiles (id, project_id, name, image, partial_messages)
         VALUES ($1, $2, $3, $4, TRUE)",
    )
    .bind(profile_id)
    .bind(project_id)
    .bind("default")
    .bind(TEST_IMAGE)
    .execute(pool)
    .await
    .expect("the profile seeds");

    let mut mutation = TrackerMutation::begin(pool, project_id, TaskActor::System)
        .await
        .expect("the mutation opens");
    TaskRepository::new(pool)
        .insert_default_states(mutation.conn(), project_id)
        .await
        .expect("the default states insert");
    mutation.commit().await.expect("the mutation commits");

    // The recipient of an unassigned task's escalation.
    let admin = app
        .insert_user("admin", "admin@example.test", true, false)
        .await;

    Fixture {
        project_id,
        profile_id,
        admin,
    }
}

/// A session, so a lease and a hand-off have something to point at.
async fn seed_session(app: &TestApp, fixture: &Fixture) -> Uuid {
    let session = NewSession::new(
        fixture.project_id,
        fixture.profile_id,
        ProfileKind::Ephemeral,
        "main",
        // Not a credential: a fake stand-in for the hashed MCP token (rule 3).
        format!("fake-mcp-token-hash-{}", Uuid::new_v4()),
    );

    let mut tx = app.pool.begin().await.expect("a transaction begins");
    let inserted = SessionRepository::new(&app.pool)
        .insert(&mut tx, &session)
        .await
        .expect("the session inserts");
    tx.commit().await.expect("the transaction commits");

    inserted.id
}

/// A task of this project in the state named `state`.
async fn task(app: &TestApp, fixture: &Fixture, title: &str, state: &str) -> TaskDto {
    let project_id = fixture.project_id;
    let state_id = TaskRepository::new(&app.pool)
        .find_state_by_name(project_id, state)
        .await
        .expect("the state reads")
        .expect("the project has this state")
        .id;

    in_mutation(&app.pool, project_id, TaskActor::System, async |m| {
        let mut new = NewTask::new(project_id, title)?;
        new.state_id = Some(state_id);
        let inserted = TaskRepository::new(m.pool())
            .insert_task(m.conn(), project_id, &new)
            .await?;
        TaskRepository::new(m.pool())
            .load_task_dto_in(m.conn(), project_id, inserted.id)
            .await?
            .ok_or(Error::NotFound)
    })
    .await
    .expect("the task inserts")
}

/// A task in `ready` that has been through `rounds` revisions and now waits in
/// `review`.
async fn reviewed_task(app: &TestApp, fixture: &Fixture, rounds: i16) -> TaskDto {
    let subject = task(app, fixture, "implement it", "ready").await;
    let implementer = seed_session(app, fixture).await;

    with_rounds(
        &app.pool,
        fixture.project_id,
        subject.id,
        implementer,
        rounds,
        "review",
    )
    .await
    .0
}

/// A forward of the task's current hand-off, as the MCP `update` tool and the
/// REST `PUT` make it once the git half is done: its own mutation, committed
/// and notified in one call, which is where an escalation email is sent.
async fn forward(
    app: &TestApp,
    fixture: &Fixture,
    task_id: Uuid,
    caller: HandoffCaller,
    decision: ReviewDecision,
    target: &str,
) -> TaskDto {
    let project_id = fixture.project_id;
    let actor = match caller {
        HandoffCaller::Session { session_id } => TaskActor::Session { session_id },
        HandoffCaller::User { user_id } => TaskActor::User { user_id },
    };

    let mut mutation = TrackerMutation::begin(&app.pool, project_id, actor)
        .await
        .expect("the mutation opens");
    let task = locked(&mut mutation, task_id)
        .await
        .expect("the task reads");
    let current = TaskRepository::new(&app.pool)
        .find_handoff(
            project_id,
            task.current_handoff_id.expect("the task has code waiting"),
        )
        .await
        .expect("the hand-off reads")
        .expect("the hand-off exists");

    let id = Uuid::new_v4();
    let prepared = PreparedHandoff {
        id,
        task_id,
        state_id: task.state_id,
        lease_holder_session_id: task.lease_holder_session_id,
        source_session_id: current.source_session_id,
        source_branch: current.source_branch.clone(),
        commit: current.commit.clone(),
        comment: SEND_BACK.to_string(),
        previous_handoff_id: task.current_handoff_id,
        review: ReviewCarry::Decision(decision),
        ref_name: refs::handoff_ref(id),
    };

    let forwarded = publish_in_transaction(
        &mut mutation,
        &task,
        &prepared,
        &caller,
        UpdateTaskInput {
            state: Some(target.to_string()),
            ..UpdateTaskInput::default()
        },
    )
    .await
    .expect("the forward publishes");
    commit_and_notify(mutation, &app.state)
        .await
        .expect("the mutation commits");

    forwarded
}

/// A reviewer session holding the task, as one does when it forwards it.
async fn reviewer(app: &TestApp, fixture: &Fixture, task_id: Uuid) -> Uuid {
    let session_id = seed_session(app, fixture).await;
    hold(&app.pool, fixture.project_id, task_id, session_id).await;
    session_id
}

/// The task as it is committed.
async fn read(app: &TestApp, project_id: Uuid, task_id: Uuid) -> TaskDto {
    TaskRepository::new(&app.pool)
        .load_task_dto(project_id, task_id)
        .await
        .expect("the task reads")
        .expect("the task is in this project")
}

/// Every committed event of the project after `seq`, oldest first.
async fn events_after(app: &TestApp, project_id: Uuid, seq: i64) -> Vec<TaskEvent> {
    TaskRepository::new(&app.pool)
        .list_task_events_after(project_id, seq, 1000)
        .await
        .expect("the events read")
        .into_iter()
        .map(|row| TaskEvent::from_row(row).expect("the row is a documented event"))
        .collect()
}

/// The highest sequence written so far: the cursor a scenario reads its own
/// events after.
async fn cursor(app: &TestApp, project_id: Uuid) -> i64 {
    events_after(app, project_id, 0)
        .await
        .last()
        .map(|event| event.seq)
        .unwrap_or(0)
}

fn kinds(events: &[TaskEvent]) -> Vec<TaskEventKind> {
    events.iter().map(|event| event.kind).collect()
}

#[tokio::test]
async fn each_revision_is_a_round_and_a_forward_is_not() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;
    let project_id = fixture.project_id;

    let subject = task(&app, &fixture, "implement it", "ready").await;
    assert_eq!(subject.rounds, 0);
    let implementer = seed_session(&app, &fixture).await;

    let (published, _) =
        with_rounds(&app.pool, project_id, subject.id, implementer, 2, "review").await;
    assert_eq!(published.rounds, 2);

    // The `state_changed` of the publication already carries the new count:
    // it is written in the same statement as the move.
    let events = events_after(&app, project_id, 0).await;
    let last_move = events
        .iter()
        .rev()
        .find(|event| event.kind == TaskEventKind::StateChanged)
        .expect("the publication moved the task");
    assert_eq!(last_move.task.as_ref().map(|task| task.rounds), Some(2));

    // Forwards, approving or not, do not count.
    let session_id = reviewer(&app, &fixture, subject.id).await;
    let approved = forward(
        &app,
        &fixture,
        subject.id,
        HandoffCaller::Session { session_id },
        ReviewDecision::Approved,
        "merge",
    )
    .await;
    assert_eq!(approved.state, "merge");
    assert_eq!(approved.rounds, 2);
    assert_eq!(read(&app, project_id, subject.id).await.rounds, 2);
}

#[tokio::test]
async fn leaving_the_human_state_resets_the_count_and_no_other_move_does() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;
    let project_id = fixture.project_id;
    let pool = &app.pool;

    let subject = reviewed_task(&app, &fixture, 2).await;

    // Queue to queue, into terminal and out again, and into the human state:
    // the count stays.
    for state in ["merge", "done", "backlog", "needs_human"] {
        let moved = move_to(pool, project_id, subject.id, state).await;
        assert_eq!(moved.state, state);
        assert_eq!(moved.rounds, 2, "a move into {state} kept the count");
    }

    // Out of the human state: a fresh allowance.
    let handed_back = move_to(pool, project_id, subject.id, "ready").await;
    assert_eq!(handed_back.rounds, 0);
    assert_eq!(read(&app, project_id, subject.id).await.rounds, 0);
}

#[tokio::test]
async fn a_revision_out_of_the_human_state_is_the_first_round_of_the_fresh_allowance() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;
    let project_id = fixture.project_id;
    let pool = &app.pool;

    let subject = reviewed_task(&app, &fixture, 2).await;
    move_to(pool, project_id, subject.id, "needs_human").await;

    // A person publishes new code straight out of the human state.
    let user = app
        .insert_user("person", "person@example.test", false, false)
        .await;
    let current = subject.handoff.as_ref().expect("the task has code waiting");
    let (published, _) = publish_handoff(
        pool,
        project_id,
        subject.id,
        Handoff {
            source_session_id: current.source_session_id,
            source_branch: &current.source_branch,
            commit: &current.commit,
            comment: "fixed it by hand",
            target_state: "review",
            caller: HandoffCaller::User { user_id: user.id },
            review: ReviewCarry::Fresh,
        },
    )
    .await;

    assert_eq!(published.state, "review");
    assert_eq!(published.rounds, 1);
}

#[tokio::test]
async fn a_session_send_back_at_the_limit_goes_to_the_human_state() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;
    let project_id = fixture.project_id;

    let subject = reviewed_task(&app, &fixture, MAX_ROUNDS).await;
    let session_id = reviewer(&app, &fixture, subject.id).await;
    let seq = cursor(&app, project_id).await;
    app.mock_email().clear();

    let sent_back = forward(
        &app,
        &fixture,
        subject.id,
        HandoffCaller::Session { session_id },
        ReviewDecision::ChangesRequested,
        "ready",
    )
    .await;

    // Redirected: the human state, with the reason, the lease gone and the
    // count kept for the person to see.
    let reason = format!("round limit reached ({MAX_ROUNDS}/{MAX_ROUNDS}): {SEND_BACK}");
    assert_eq!(sent_back.state, "needs_human");
    assert_eq!(
        sent_back.needs_human_reason.as_deref(),
        Some(reason.as_str())
    );
    assert_eq!(sent_back.lease_holder_session_id, None);
    assert_eq!(sent_back.rounds, MAX_ROUNDS);
    assert_eq!(read(&app, project_id, subject.id).await, sent_back);

    // The decision is still recorded on the new hand-off, by the reviewer.
    let handoff = sent_back.handoff.as_ref().expect("the forward is current");
    assert_ne!(Some(handoff.id), subject.handoff.as_ref().map(|h| h.id));
    assert_eq!(handoff.review_status, ReviewStatus::ChangesRequested);
    assert_eq!(handoff.reviewed_by_session_id, Some(session_id));

    // The forward's comment is written.
    let comments = TaskRepository::new(&app.pool)
        .list_comments(project_id, subject.id)
        .await
        .expect("the comments read");
    let last = comments.last().expect("the forward wrote a comment");
    assert_eq!(last.body, SEND_BACK);
    assert_eq!(last.author_session_id, Some(session_id));

    // `escalated`, never `state_changed`, then the comment.
    let events = events_after(&app, project_id, seq).await;
    assert_eq!(
        kinds(&events),
        [TaskEventKind::Escalated, TaskEventKind::Commented],
        "{events:?}"
    );
    let escalated = &events[0];
    assert_eq!(escalated.actor, TaskActor::Session { session_id });
    assert_eq!(escalated.from.as_deref(), Some("review"));
    assert_eq!(escalated.to.as_deref(), Some("needs_human"));
    assert_eq!(escalated.reason.as_deref(), Some(reason.as_str()));

    // And the email every escalation owes, to the administrator.
    let sent = app.mock_email().sent();
    assert_eq!(sent.len(), 1, "exactly one message: {sent:?}");
    let message = &sent[0];
    assert_eq!(message.to, fixture.admin.email);
    assert!(message.text.contains(&reason), "{}", message.text);
}

#[tokio::test]
async fn a_session_send_back_below_the_limit_goes_where_it_asked() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;
    let project_id = fixture.project_id;

    let subject = reviewed_task(&app, &fixture, MAX_ROUNDS - 1).await;
    let session_id = reviewer(&app, &fixture, subject.id).await;
    let seq = cursor(&app, project_id).await;
    app.mock_email().clear();

    let sent_back = forward(
        &app,
        &fixture,
        subject.id,
        HandoffCaller::Session { session_id },
        ReviewDecision::ChangesRequested,
        "ready",
    )
    .await;

    assert_eq!(sent_back.state, "ready");
    assert_eq!(sent_back.needs_human_reason, None);
    assert_eq!(sent_back.rounds, MAX_ROUNDS - 1);
    assert_eq!(
        kinds(&events_after(&app, project_id, seq).await),
        [TaskEventKind::StateChanged, TaskEventKind::Commented]
    );
    assert!(app.mock_email().sent().is_empty());
}

#[tokio::test]
async fn a_users_send_back_at_the_limit_is_never_redirected() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;
    let project_id = fixture.project_id;

    let subject = reviewed_task(&app, &fixture, MAX_ROUNDS).await;
    let seq = cursor(&app, project_id).await;
    app.mock_email().clear();

    let sent_back = forward(
        &app,
        &fixture,
        subject.id,
        HandoffCaller::User {
            user_id: fixture.admin.id,
        },
        ReviewDecision::ChangesRequested,
        "ready",
    )
    .await;

    assert_eq!(sent_back.state, "ready");
    assert_eq!(sent_back.needs_human_reason, None);
    assert_eq!(
        sent_back.handoff.as_ref().map(|h| h.review_status),
        Some(ReviewStatus::ChangesRequested)
    );
    assert_eq!(
        kinds(&events_after(&app, project_id, seq).await),
        [TaskEventKind::StateChanged, TaskEventKind::Commented]
    );
    assert!(app.mock_email().sent().is_empty());
}

#[tokio::test]
async fn an_approving_forward_at_the_limit_is_not_a_send_back() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;

    let subject = reviewed_task(&app, &fixture, MAX_ROUNDS).await;
    let session_id = reviewer(&app, &fixture, subject.id).await;

    let approved = forward(
        &app,
        &fixture,
        subject.id,
        HandoffCaller::Session { session_id },
        ReviewDecision::Approved,
        "merge",
    )
    .await;

    assert_eq!(approved.state, "merge");
    assert_eq!(approved.needs_human_reason, None);
}

/// The helper the auto-merge job's conflict send-back calls, with the
/// system's actor.
async fn system_send_back(app: &TestApp, fixture: &Fixture, task_id: Uuid) -> TaskDto {
    let project_id = fixture.project_id;
    let mut mutation = TrackerMutation::begin(&app.pool, project_id, TaskActor::System)
        .await
        .expect("the mutation opens");
    let task = locked(&mut mutation, task_id)
        .await
        .expect("the task reads");
    let requested = TaskRepository::new(&app.pool)
        .find_state_by_name(project_id, "ready")
        .await
        .expect("the state reads")
        .expect("the project has it");

    let moved = send_back(&mut mutation, &task, &requested, "Merge conflicted.")
        .await
        .expect("the send-back succeeds");
    commit_and_notify(mutation, &app.state)
        .await
        .expect("the mutation commits");

    moved.task
}

#[tokio::test]
async fn the_system_send_back_helper_applies_the_same_limit() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;
    let project_id = fixture.project_id;

    // Below the limit: the requested state, announced as a plain move.
    let below = reviewed_task(&app, &fixture, MAX_ROUNDS - 1).await;
    let seq = cursor(&app, project_id).await;
    app.mock_email().clear();
    let moved = system_send_back(&app, &fixture, below.id).await;
    assert_eq!(moved.state, "ready");
    assert_eq!(
        kinds(&events_after(&app, project_id, seq).await),
        [TaskEventKind::StateChanged]
    );
    assert!(app.mock_email().sent().is_empty());

    // At it: the human state, `escalated`, the reason and the email.
    let at = reviewed_task(&app, &fixture, MAX_ROUNDS).await;
    let seq = cursor(&app, project_id).await;
    app.mock_email().clear();
    let moved = system_send_back(&app, &fixture, at.id).await;
    let reason = format!("round limit reached ({MAX_ROUNDS}/{MAX_ROUNDS}): Merge conflicted.");
    assert_eq!(moved.state, "needs_human");
    assert_eq!(moved.needs_human_reason.as_deref(), Some(reason.as_str()));
    let events = events_after(&app, project_id, seq).await;
    assert_eq!(kinds(&events), [TaskEventKind::Escalated]);
    assert_eq!(events[0].actor, TaskActor::System);
    assert_eq!(events[0].reason.as_deref(), Some(reason.as_str()));
    assert_eq!(app.mock_email().sent().len(), 1);
}
