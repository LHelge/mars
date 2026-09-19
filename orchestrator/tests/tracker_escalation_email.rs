//! Who is told when a task is handed to a person, and what they are told
//! (`CLAUDE.md`, "Testing expectations").
//!
//! `ARCHITECTURE.md`, "Task tracker" → "Notification" is the contract: every
//! move into the human state, by `needs_human` or by the reaper, sends one
//! email through `EmailClient` to the task's assignee if it has one, otherwise
//! to every admin, skipping users whose `notify_email` is off. Nothing else
//! the tracker does sends email.
//!
//! The scenarios here are the recipient rules and the failure rule:
//!
//! - an assignee who wants email is the only recipient, and the message names
//!   the project, the task, the reason and the task's link;
//! - an assignee who opted out receives nothing, and the administrators are
//!   *not* fallen back to — the assignee's choice is the answer;
//! - an unassigned task goes to every administrator who wants email, and to
//!   nobody else;
//! - a task already waiting for a person is not escalated twice, so no second
//!   email goes out;
//! - a user moving a task into the human state sends no email at all;
//! - a provider failure is logged and stepped over: the remaining recipients
//!   are still told and the tracker operation has already succeeded.
//!
//! Driven through `TrackerMutation` and `tracker::commit_and_notify` directly,
//! with the mock email client reached through `TestApp`: the MCP tools and the
//! session hooks that will call the same pairing are other tasks'.
//!
//! Needs a container engine; see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use common::TestApp;
use mars_orchestrator::email::EmailMessage;
use mars_orchestrator::events::TaskActor;
use mars_orchestrator::models::{NewSession, NewTask, ProfileKind, Task, User};
use mars_orchestrator::repositories::{SessionRepository, TaskRepository};
use mars_orchestrator::tracker::leases::{claim_for_profile, needs_human, release_by_agent};
use mars_orchestrator::tracker::state::change_state;
use mars_orchestrator::tracker::{
    StateChangeOptions, StateEventKind, TaskDto, TrackerMutation, commit_and_notify,
};
use uuid::Uuid;

/// Not a real image: the stub the session tests replay a fixture transcript
/// with.
const TEST_IMAGE: &str = "localhost/mars-session-stub:test";

/// The project default this suite relies on (`SPEC.md`, "Projects").
const MAX_ATTEMPTS: i16 = 3;

/// The `PUBLIC_URL` the harness configures (`tests/common/app.rs`).
const PUBLIC_URL: &str = "http://localhost";

/// A project with the documented default states and a profile to hang
/// sessions off.
struct Fixture {
    project_id: Uuid,
    project_name: String,
    profile_id: Uuid,
}

async fn seed(app: &TestApp) -> Fixture {
    let pool = &app.pool;

    let project_id = Uuid::new_v4();
    let project_name = format!("project-{project_id}");
    sqlx::query("INSERT INTO projects (id, name, remote_url) VALUES ($1, $2, $3)")
        .bind(project_id)
        .bind(&project_name)
        // `.invalid` can never resolve (rule 3).
        .bind("https://example.invalid/org/repo.git")
        .execute(pool)
        .await
        .expect("the project seeds");

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

    Fixture {
        project_id,
        project_name,
        profile_id,
    }
}

/// A session, so a lease has something to point at.
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

/// A `ready` task of this project, assigned to `assignee` when there is one.
async fn task(app: &TestApp, fixture: &Fixture, title: &str, assignee: Option<&User>) -> Task {
    let project_id = fixture.project_id;
    let mut new = NewTask::new(project_id, title).expect("the title parses");
    new.state_id = Some(state_id(app, project_id, "ready").await);
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

/// Turn a user's `notify_email` off, as `PATCH /users/me` would.
async fn opt_out(app: &TestApp, user: &User) {
    sqlx::query("UPDATE users SET notify_email = FALSE WHERE id = $1")
        .bind(user.id)
        .execute(&app.pool)
        .await
        .expect("the opt-out writes");
}

/// Claim as an agent would, through the tool's own path.
async fn claim(app: &TestApp, fixture: &Fixture, task_id: Uuid, session_id: Uuid) -> TaskDto {
    let project_id = fixture.project_id;
    let served = vec![state_id(app, project_id, "ready").await];

    let mut mutation =
        TrackerMutation::begin(&app.pool, project_id, TaskActor::Session { session_id })
            .await
            .expect("the mutation opens");
    let locked = locked_task(&mut mutation, project_id, task_id).await;
    let claimed = claim_for_profile(&mut mutation, &locked, session_id, &served)
        .await
        .expect("the claim succeeds");
    commit_and_notify(mutation, &app.state)
        .await
        .expect("the mutation commits");

    claimed
}

/// Release as the MCP `release` tool would: its own mutation, committed and
/// notified in one call.
async fn agent_release(
    app: &TestApp,
    fixture: &Fixture,
    task_id: Uuid,
    session_id: Uuid,
    reason: &str,
) -> TaskDto {
    let project_id = fixture.project_id;
    let mut mutation =
        TrackerMutation::begin(&app.pool, project_id, TaskActor::Session { session_id })
            .await
            .expect("the mutation opens");
    let locked = locked_task(&mut mutation, project_id, task_id).await;
    let released = release_by_agent(&mut mutation, &locked, session_id, reason)
        .await
        .expect("the release succeeds");
    commit_and_notify(mutation, &app.state)
        .await
        .expect("the mutation commits");

    released
}

/// The same, for the MCP `needs_human` tool.
async fn hand_to_human(
    app: &TestApp,
    fixture: &Fixture,
    task_id: Uuid,
    session_id: Uuid,
    reason: &str,
) -> TaskDto {
    let project_id = fixture.project_id;
    let mut mutation =
        TrackerMutation::begin(&app.pool, project_id, TaskActor::Session { session_id })
            .await
            .expect("the mutation opens");
    let locked = locked_task(&mut mutation, project_id, task_id).await;
    let handed = needs_human(&mut mutation, &locked, session_id, reason)
        .await
        .expect("the hand-off succeeds");
    commit_and_notify(mutation, &app.state)
        .await
        .expect("the mutation commits");

    handed
}

/// The task row under the mutation's lock.
async fn locked_task(m: &mut TrackerMutation<'_>, project_id: Uuid, task_id: Uuid) -> Task {
    let pool = m.pool();
    TaskRepository::new(pool)
        .find_task_for_update(m.conn(), project_id, task_id.into())
        .await
        .expect("the row reads")
        .expect("the task is in this project")
}

/// The task as it is committed.
async fn read(app: &TestApp, project_id: Uuid, task_id: Uuid) -> TaskDto {
    TaskRepository::new(&app.pool)
        .load_task_dto(project_id, task_id)
        .await
        .expect("the task reads")
        .expect("the task is in this project")
}

/// Run three claim-and-release cycles, which is what the attempt limit takes.
async fn exhaust_attempts(app: &TestApp, fixture: &Fixture, task_id: Uuid) {
    for attempt in 1..=MAX_ATTEMPTS {
        let session_id = seed_session(app, fixture).await;
        claim(app, fixture, task_id, session_id).await;
        agent_release(
            app,
            fixture,
            task_id,
            session_id,
            &format!("attempt {attempt} failed"),
        )
        .await;
    }
}

/// The one message that went out, or a failure naming how many did.
fn only_message(app: &TestApp) -> EmailMessage {
    let sent = app.mock_email().sent();
    assert_eq!(sent.len(), 1, "exactly one message: {sent:?}");
    sent.into_iter().next().expect("the message is there")
}

/// Everything the escalation email has to name (`ARCHITECTURE.md`, "Task
/// tracker" → "Notification").
fn assert_names_everything(
    message: &EmailMessage,
    fixture: &Fixture,
    task: &Task,
    reason_fragment: &str,
) {
    let link = format!(
        "{PUBLIC_URL}/projects/{}/tasks/{}",
        fixture.project_id, task.number,
    );

    assert!(
        message.text.contains(&fixture.project_name),
        "the project: {}",
        message.text,
    );
    assert!(
        message.text.contains(&format!("#{}", task.number)),
        "the number: {}",
        message.text,
    );
    assert!(
        message.text.contains(task.title.as_str()),
        "the title: {}",
        message.text,
    );
    assert!(
        message.text.contains(reason_fragment),
        "the reason: {}",
        message.text,
    );
    assert!(message.text.contains(&link), "the link: {}", message.text);
}

#[tokio::test]
async fn the_assignee_is_the_only_recipient_and_is_told_everything() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;

    let assignee = app
        .insert_user("assignee", "assignee@example.test", false, false)
        .await;
    // An administrator who would be told if the task had no assignee, and is
    // not told because it has one.
    app.insert_user("admin", "admin@example.test", true, false)
        .await;

    let subject = task(&app, &fixture, "rotate the deploy key", Some(&assignee)).await;
    exhaust_attempts(&app, &fixture, subject.id).await;

    assert_eq!(
        read(&app, fixture.project_id, subject.id).await.state,
        "needs_human",
    );

    let message = only_message(&app);
    assert_eq!(message.to, assignee.email);
    assert_names_everything(&message, &fixture, &subject, "attempt 3 failed");
}

#[tokio::test]
async fn an_assignee_who_opted_out_is_told_nothing_and_no_admin_stands_in() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;

    let assignee = app
        .insert_user("assignee", "assignee@example.test", false, false)
        .await;
    opt_out(&app, &assignee).await;
    // Two administrators who both want email, and get none: the assignee's
    // choice is the answer, not a reason to ask somebody else.
    app.insert_user("admin-one", "admin-one@example.test", true, false)
        .await;
    app.insert_user("admin-two", "admin-two@example.test", true, false)
        .await;

    let subject = task(&app, &fixture, "rotate the deploy key", Some(&assignee)).await;
    let session_id = seed_session(&app, &fixture).await;
    hand_to_human(
        &app,
        &fixture,
        subject.id,
        session_id,
        "a person must decide",
    )
    .await;

    assert_eq!(
        read(&app, fixture.project_id, subject.id).await.state,
        "needs_human",
        "the escalation still happened",
    );
    assert!(
        app.mock_email().sent().is_empty(),
        "{:?}",
        app.mock_email().sent(),
    );
}

#[tokio::test]
async fn an_unassigned_escalation_goes_to_the_administrators_who_want_email() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;

    let told = app
        .insert_user("admin-told", "admin-told@example.test", true, false)
        .await;
    let quiet = app
        .insert_user("admin-quiet", "admin-quiet@example.test", true, false)
        .await;
    opt_out(&app, &quiet).await;
    // Wants email and is not an administrator, so it is not this user's
    // business.
    app.insert_user("member", "member@example.test", false, false)
        .await;

    let subject = task(&app, &fixture, "nobody owns this", None).await;
    let session_id = seed_session(&app, &fixture).await;
    hand_to_human(
        &app,
        &fixture,
        subject.id,
        session_id,
        "the build is wedged",
    )
    .await;

    let message = only_message(&app);
    assert_eq!(message.to, told.email);
    assert_names_everything(&message, &fixture, &subject, "the build is wedged");
}

#[tokio::test]
async fn a_task_already_waiting_for_a_person_is_not_emailed_about_twice() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;

    app.insert_user("admin", "admin@example.test", true, false)
        .await;

    let subject = task(&app, &fixture, "nobody owns this", None).await;
    let first = seed_session(&app, &fixture).await;
    hand_to_human(&app, &fixture, subject.id, first, "the build is wedged").await;
    assert_eq!(app.mock_email().sent().len(), 1, "the first escalation");
    app.mock_email().clear();

    // A user launched an agent on the escalated task and it asked for a person
    // again. The reason is recorded, and nobody is told anything new.
    let second = seed_session(&app, &fixture).await;
    hand_to_human(&app, &fixture, subject.id, second, "still wedged").await;

    assert_eq!(
        read(&app, fixture.project_id, subject.id)
            .await
            .needs_human_reason
            .as_deref(),
        Some("still wedged"),
        "the second reason is recorded",
    );
    assert!(
        app.mock_email().sent().is_empty(),
        "{:?}",
        app.mock_email().sent(),
    );
}

#[tokio::test]
async fn a_user_moving_a_task_into_the_human_state_sends_nothing() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;

    let user = app
        .insert_user("admin", "admin@example.test", true, false)
        .await;

    let subject = task(&app, &fixture, "a person will look at this", None).await;

    // This is the `PUT /projects/{pid}/tasks/{id}` move, which has no route
    // yet (Bears `72jr7`): a user's ordinary state change into the human
    // state, as `state_changed` rather than `escalated`. No mutation records
    // an escalation on that path, so `commit_and_notify` has nothing to send.
    let human = TaskRepository::new(&app.pool)
        .find_state_by_name(fixture.project_id, "needs_human")
        .await
        .expect("the state reads")
        .expect("the project has a human state");

    let mut mutation = TrackerMutation::begin(
        &app.pool,
        fixture.project_id,
        TaskActor::User { user_id: user.id },
    )
    .await
    .expect("the mutation opens");
    let locked = locked_task(&mut mutation, fixture.project_id, subject.id).await;
    let result = change_state(
        &mut mutation,
        &locked,
        &human,
        StateChangeOptions {
            event: StateEventKind::StateChanged,
            needs_human_reason: None,
        },
    )
    .await
    .expect("the move succeeds");
    assert!(result.changed);
    commit_and_notify(mutation, &app.state)
        .await
        .expect("the mutation commits");

    assert_eq!(
        read(&app, fixture.project_id, subject.id).await.state,
        "needs_human",
    );
    assert!(
        app.mock_email().sent().is_empty(),
        "{:?}",
        app.mock_email().sent(),
    );
}

#[tokio::test]
async fn a_failed_send_is_stepped_over_and_the_next_recipient_is_still_told() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;

    app.insert_user("admin-one", "admin-one@example.test", true, false)
        .await;
    app.insert_user("admin-two", "admin-two@example.test", true, false)
        .await;

    let subject = task(&app, &fixture, "nobody owns this", None).await;
    let session_id = seed_session(&app, &fixture).await;

    // The provider refuses exactly one message. The tracker operation has
    // already committed, so it cannot fail, and the other administrator is
    // still told.
    app.mock_email().fail_next();
    let handed = hand_to_human(
        &app,
        &fixture,
        subject.id,
        session_id,
        "the build is wedged",
    )
    .await;

    assert_eq!(handed.state, "needs_human");
    assert_eq!(
        read(&app, fixture.project_id, subject.id).await.state,
        "needs_human",
    );

    let message = only_message(&app);
    assert_eq!(
        message.to, "admin-two@example.test",
        "the failed send was the first, by username order",
    );
    assert_names_everything(&message, &fixture, &subject, "the build is wedged");
}
