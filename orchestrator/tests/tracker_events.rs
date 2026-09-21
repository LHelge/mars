//! The tracker's event matrix and its history hygiene, end to end
//! (`CLAUDE.md`, "Testing expectations").
//!
//! The per-task suites each assert the events of one operation. This file is
//! the regression net over all of them: three scripted scenarios read the
//! *whole* `task_events` stream of a project afterwards and assert it against
//! a literal list of `(seq, kind, actor, task number, from, to, reason)`
//! tuples, so a change anywhere in `tracker::` that adds, drops or reorders a
//! row shows up here as a readable diff of the contract rather than as a
//! passing test somewhere else.
//!
//! Between them the three scenarios cover all thirteen kinds of `SPEC.md`,
//! "TaskEvent": `created`, `updated`, `state_changed`, `claimed`, `released`,
//! `escalated`, `blocked`, `unblocked`, `commented`, `dependency_added`,
//! `dependency_removed`, `deleted` and `states_changed`.
//!
//! The negative half is what `docs/data-model.md` and ADR 0030 promise about
//! history: a rejected operation and a no-op update write **nothing** — not an
//! event, not a `task_sessions` row and not a new `last_touched_at` — and the
//! reads (`ready_summaries`, `load_task_detail`) write nothing either. Every
//! refusal below is bracketed by [`Hygiene`], which is the count of the
//! project's events together with every `task_sessions` link and its
//! timestamp; it must come back byte for byte identical.
//!
//! Three more invariants are asserted on every scenario's stream, by
//! [`assert_stream_invariants`]:
//!
//! - the sequences are `1..=n` with no gaps, because they are allocated as
//!   `MAX(seq) + 1` under the project lock (`docs/data-model.md`);
//! - every event carrying a `task` carries the task it is about
//!   (`task.id == task_id`) as it is *after* the change, which for
//!   `state_changed` and `escalated` means `task.state == to`;
//! - `deleted` keeps the original task UUID in `task_id` and has no `task`
//!   key in the stored payload at all — asserted on the `payload` column, not
//!   only on the deserialised struct, since the absence is what `SPEC.md`
//!   writes `task?` (ADR 0022).
//!
//! **Scenario A runs over HTTP** through `TestApp`, so the route wiring is
//! part of what is covered; **scenarios B and C drive the domain functions**
//! through the small facade below, each operation in its own
//! `TrackerMutation` exactly as a real caller makes it, so no MCP transport is
//! needed.
//!
//! The expected lists are literal on purpose (no loop generates them): a
//! reviewer reads the contract off the test. Where a per-task suite and this
//! one would disagree, the per-task suite wins and this file is corrected with
//! the fix.
//!
//! Every credential-shaped value is an obviously fake stand-in (rule 3).
//!
//! Needs a container engine; see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use common::{AuthenticatedUser, TestApp};
use mars_orchestrator::events::{TaskActor, TaskEvent, TaskEventKind};
use mars_orchestrator::models::{NewSession, ProfileKind, Task, TaskEventRow, TaskRef};
use mars_orchestrator::prelude::*;
use mars_orchestrator::projects::{NewProjectRequest, create_project};
use mars_orchestrator::repositories::{SessionRepository, TaskRepository};
use mars_orchestrator::tracker::{
    CreateTaskInput, CreatedBy, ReleaseReason, TaskDetailDto, TaskDto, TaskSummary,
    TrackerMutation, UpdateTaskInput, claim_for_launch, claim_for_profile, create_task,
    needs_human, ready_summaries, release_by_agent, release_by_user, release_leases_for_session,
    update_task,
};
use serde_json::{Value, json};
use uuid::Uuid;

/// Not a real remote: `.invalid` can never resolve (rule 3).
const TEST_REMOTE: &str = "https://example.invalid/org/repo.git";

/// The project's `max_attempts` for the session scenario, so that the second
/// release is the one that runs out of attempts.
const MAX_ATTEMPTS: i16 = 2;

/// Wider than any scenario here, so a missing event is a shorter list rather
/// than a truncated page.
const WHOLE_STREAM: u32 = 10_000;

// ---- the assertion vocabulary ----

/// One event as this suite reads it: the six facts the contract is written in,
/// plus its sequence.
///
/// `task` is the task's *per-project number* rather than its UUID, because a
/// number is readable in an expectation and a UUID is not; `None` is the
/// `states_changed` event, whose `task_id` is NULL and which the helper
/// therefore must not filter out.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Row {
    seq: i64,
    kind: String,
    actor: String,
    task: Option<i32>,
    from: Option<String>,
    to: Option<String>,
    reason: Option<String>,
}

/// One expected row, spelled the way the scenarios spell it.
fn row(
    seq: i64,
    kind: &str,
    actor: &str,
    task: Option<i32>,
    from: Option<&str>,
    to: Option<&str>,
    reason: Option<&str>,
) -> Row {
    Row {
        seq,
        kind: kind.to_string(),
        actor: actor.to_string(),
        task,
        from: from.map(str::to_string),
        to: to.map(str::to_string),
        reason: reason.map(str::to_string),
    }
}

/// Assert a described stream against a literal list of tuples.
///
/// The whole list at once rather than row by row, so a missing or extra event
/// is one diff naming every row that moved.
macro_rules! expect {
    ($actual:expr, [ $( ( $seq:expr, $kind:expr, $actor:expr, $task:expr, $from:expr, $to:expr, $reason:expr ) ),* $(,)? ]) => {{
        let expected: Vec<Row> = vec![
            $( row($seq, $kind, $actor, $task, $from, $to, $reason) ),*
        ];
        assert_eq!($actual, expected);
    }};
}

/// The per-project numbers of the tasks a scenario made, so an event's
/// `task_id` can be described as the number a reader knows it by.
type Numbers = Vec<(Uuid, i32)>;

/// The actor's kind, as `SPEC.md` tags it.
fn actor_kind(actor: TaskActor) -> &'static str {
    match actor {
        TaskActor::User { .. } => "user",
        TaskActor::Session { .. } => "session",
        TaskActor::System => "system",
    }
}

/// A reason up to and including its first colon.
///
/// Every reason the tracker writes is a fixed word — `user`, `given_back`,
/// `session_ended`, `stalled` — except the escalation's, which is
/// `attempt limit reached (n/max): <the last release's own words>`. Cutting
/// at the colon keeps the generated half literal in the expectation without
/// repeating the agent's words there; the full text is asserted separately in
/// the scenario that produces it.
fn reason_head(reason: &str) -> String {
    match reason.find(':') {
        Some(at) => reason[..=at].to_string(),
        None => reason.to_string(),
    }
}

/// Every event of the project, oldest first, through the documented reader.
async fn events_of(pool: &PgPool, project_id: Uuid) -> Vec<TaskEvent> {
    rows_of(pool, project_id)
        .await
        .into_iter()
        .map(|row| TaskEvent::from_row(row).expect("the payload is the documented shape"))
        .collect()
}

/// The same events as stored rows, for the assertions that are about the
/// `payload` column itself.
async fn rows_of(pool: &PgPool, project_id: Uuid) -> Vec<TaskEventRow> {
    TaskRepository::new(pool)
        .list_task_events_after(project_id, 0, WHOLE_STREAM)
        .await
        .expect("the stream reads")
}

/// The stream as `(seq, kind, actor, number, from, to, reason)` rows.
fn described(events: &[TaskEvent], numbers: &Numbers) -> Vec<Row> {
    events
        .iter()
        .map(|event| Row {
            seq: event.seq,
            kind: event.kind.to_string(),
            actor: actor_kind(event.actor).to_string(),
            task: event.task_id.map(|id| {
                numbers
                    .iter()
                    .find(|(task_id, _)| *task_id == id)
                    .map(|(_, number)| *number)
                    .unwrap_or_else(|| panic!("event {} is about an unknown task", event.seq))
            }),
            from: event.from.clone(),
            to: event.to.clone(),
            reason: event.reason.as_deref().map(reason_head),
        })
        .collect()
}

/// The three invariants every scenario's stream owes, whatever it did.
///
/// Run on the rows and on the deserialised events together: the `deleted`
/// case is about a key being *absent* from the stored JSON, which a struct
/// with `Option` fields cannot tell apart from `null`.
fn assert_stream_invariants(rows: &[TaskEventRow], events: &[TaskEvent]) {
    let seqs: Vec<i64> = events.iter().map(|event| event.seq).collect();
    let expected: Vec<i64> = (1..=events.len() as i64).collect();
    assert_eq!(seqs, expected, "the sequences are 1..n with no gaps");

    for event in events {
        if let Some(task) = event.task.as_ref() {
            assert_eq!(
                Some(task.id),
                event.task_id,
                "event {} carries a task that is not the one it is about",
                event.seq,
            );

            if matches!(
                event.kind,
                TaskEventKind::StateChanged | TaskEventKind::Escalated
            ) {
                assert_eq!(
                    Some(task.state.as_str()),
                    event.to.as_deref(),
                    "event {} does not carry the task as it is after the move",
                    event.seq,
                );
            }
        }

        match event.kind {
            TaskEventKind::Deleted | TaskEventKind::StatesChanged => {
                assert!(
                    event.task.is_none(),
                    "event {} carries a task it should not",
                    event.seq,
                );
            }
            _ => {}
        }
    }

    for row in rows {
        let object = row.payload.as_object().expect("a payload object");

        if row.kind == TaskEventKind::Deleted.as_str() {
            assert!(
                row.task_id.is_some(),
                "a deleted event lost the original task id",
            );
            assert!(
                !object.contains_key("task"),
                "a deleted event's stored payload still has a task key",
            );
        }

        if row.kind == TaskEventKind::StatesChanged.as_str() {
            assert!(
                row.task_id.is_none(),
                "a states_changed event is not project-wide",
            );
            assert!(
                object.contains_key("states"),
                "a states_changed event carries no state list",
            );
        }
    }
}

// ---- history hygiene ----

/// What a rejected or read-only operation must leave exactly as it found it.
///
/// The event count and every `task_sessions` link of the project with its
/// `last_touched_at`: the two tables `docs/data-model.md` and ADR 0030 make
/// promises about.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Hygiene {
    events: i64,
    touches: Vec<(Uuid, Uuid, DateTime<Utc>)>,
}

async fn hygiene(pool: &PgPool, project_id: Uuid) -> Hygiene {
    let events =
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM task_events WHERE project_id = $1")
            .bind(project_id)
            .fetch_one(pool)
            .await
            .expect("the event count reads");

    let touches = sqlx::query_as::<_, (Uuid, Uuid, DateTime<Utc>)>(
        "SELECT ts.task_id, ts.session_id, ts.last_touched_at
         FROM task_sessions AS ts
         JOIN tasks AS t ON t.id = ts.task_id
         WHERE t.project_id = $1
         ORDER BY ts.task_id, ts.session_id",
    )
    .bind(project_id)
    .fetch_all(pool)
    .await
    .expect("the session links read");

    Hygiene { events, touches }
}

// ---- fixtures ----

/// An obviously fake password of the length `POST /api/test/users` requires
/// (rule 3).
fn password(name: &str) -> String {
    format!("fake-password-{name}")
}

/// A signed-in ordinary user to make requests as.
async fn signed_in(app: &TestApp, name: &str) -> AuthenticatedUser {
    app.create_user(name, &format!("{name}@example.test"), &password(name))
        .await
}

/// A project with the seven default states and the default agent profile.
///
/// Through `create_project` rather than `POST /api/projects`: what these
/// scenarios need is the states and the task-number counter, not a clone.
async fn project(app: &TestApp, user: &AuthenticatedUser, name: &str) -> Uuid {
    create_project(
        &app.state,
        NewProjectRequest {
            name: name.to_string(),
            remote_url: TEST_REMOTE.to_string(),
            default_branch: None,
            credential: None,
        },
        user.user.id,
    )
    .await
    .expect("the project is created")
    .id
}

/// The project's default agent profile, to hang sessions off.
///
/// By the flag: a project is seeded with four role profiles and this is the
/// one a launch defaults to (`SPEC.md`, "Role profile templates").
async fn default_profile(pool: &PgPool, project_id: Uuid) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM agent_profiles WHERE project_id = $1 AND is_default",
    )
    .bind(project_id)
    .fetch_one(pool)
    .await
    .expect("the project has its default profile")
}

/// A session of this project, inserted through the repository.
async fn session(pool: &PgPool, project_id: Uuid, profile_id: Uuid) -> Uuid {
    let new = NewSession::new(
        project_id,
        profile_id,
        ProfileKind::Conversational,
        "main",
        // Not a credential: a fake stand-in for the hashed MCP token (rule 3).
        format!("fake-mcp-token-hash-{}", Uuid::new_v4()),
    );

    let mut tx = pool.begin().await.expect("a transaction begins");
    let inserted = SessionRepository::new(pool)
        .insert(&mut tx, &new)
        .await
        .expect("the session inserts");
    tx.commit().await.expect("the transaction commits");

    inserted.id
}

/// The ids of the named states of this project, which is what a served-state
/// policy is.
async fn state_ids(pool: &PgPool, project_id: Uuid, names: &[&str]) -> Vec<Uuid> {
    let repository = TaskRepository::new(pool);
    let mut ids = Vec::with_capacity(names.len());
    for name in names {
        ids.push(
            repository
                .find_state_by_name(project_id, name)
                .await
                .expect("the state reads")
                .expect("the project has this state")
                .id,
        );
    }
    ids
}

/// The task's state name, straight from the row.
async fn state_of(pool: &PgPool, task_id: Uuid) -> String {
    sqlx::query_scalar::<_, String>(
        "SELECT s.name FROM tasks AS t JOIN task_states AS s ON s.id = t.state_id WHERE t.id = $1",
    )
    .bind(task_id)
    .fetch_one(pool)
    .await
    .expect("the task's state reads")
}

// ---- the tracker facade ----
//
// One mutation per operation, opened and finished here exactly as a REST
// handler or an MCP tool finishes one: committed when the operation succeeded
// and dropped — therefore rolled back — when it did not. That is what makes
// the negative assertions below meaningful rather than an artefact of sharing
// one transaction.

/// The row as it is under this mutation's lock, which every write takes.
async fn locked(m: &mut TrackerMutation<'_>, pool: &PgPool, task_id: Uuid) -> Task {
    let project_id = m.project_id();

    TaskRepository::new(pool)
        .find_task_for_update(m.conn(), project_id, TaskRef::Id(task_id))
        .await
        .expect("the task reads")
        .expect("the project has this task")
}

/// Create a task as `user`, committing the creation.
async fn create(
    pool: &PgPool,
    project_id: Uuid,
    user_id: Uuid,
    title: &str,
    state: Option<&str>,
    parent: Option<Uuid>,
    depends_on: Vec<Uuid>,
) -> TaskDto {
    let mut m = TrackerMutation::begin(pool, project_id, TaskActor::User { user_id })
        .await
        .expect("the mutation opens");

    let created = create_task(
        &mut m,
        CreateTaskInput {
            title: title.to_string(),
            description: None,
            state: state.map(str::to_string),
            priority: None,
            labels: Vec::new(),
            parent,
            depends_on: depends_on.into_iter().map(TaskRef::Id).collect(),
            // Provenance is a session's, and every creation here is a user's.
            discovered_from: None,
            created_by: CreatedBy::User(user_id),
        },
    )
    .await
    .expect("the task is created");

    m.commit().await.expect("the mutation commits");

    created
}

/// Move a task to a state as `user`, through the update path.
async fn set_state(pool: &PgPool, project_id: Uuid, user_id: Uuid, task_id: Uuid, state: &str) {
    let mut m = TrackerMutation::begin(pool, project_id, TaskActor::User { user_id })
        .await
        .expect("the mutation opens");

    let task = locked(&mut m, pool, task_id).await;
    let outcome = update_task(
        &mut m,
        &task,
        UpdateTaskInput {
            state: Some(state.to_string()),
            ..UpdateTaskInput::default()
        },
    )
    .await
    .expect("the update applies");

    assert!(outcome.changed, "the move was expected to change something");
    m.commit().await.expect("the mutation commits");
}

/// A session claiming a task from the states its profile serves (MCP `claim`).
async fn claim(
    pool: &PgPool,
    project_id: Uuid,
    task_id: Uuid,
    session_id: Uuid,
    served: &[Uuid],
) -> Result<TaskDto> {
    let mut m = TrackerMutation::begin(pool, project_id, TaskActor::Session { session_id }).await?;
    let task = locked(&mut m, pool, task_id).await;
    let claimed = claim_for_profile(&mut m, &task, session_id, served).await?;
    m.commit().await?;

    Ok(claimed)
}

/// A user launching a session on a task (`POST /projects/{pid}/sessions`).
async fn launch(
    pool: &PgPool,
    project_id: Uuid,
    task_id: Uuid,
    session_id: Uuid,
    user_id: Uuid,
) -> Result<TaskDto> {
    let mut m = TrackerMutation::begin(pool, project_id, TaskActor::User { user_id }).await?;
    let task = locked(&mut m, pool, task_id).await;
    let claimed = claim_for_launch(&mut m, &task, session_id).await?;
    m.commit().await?;

    Ok(claimed)
}

/// A session giving a task back with its reason (MCP `release`).
async fn release_agent(
    pool: &PgPool,
    project_id: Uuid,
    task_id: Uuid,
    session_id: Uuid,
    reason: &str,
) -> Result<TaskDto> {
    let mut m = TrackerMutation::begin(pool, project_id, TaskActor::Session { session_id }).await?;
    let task = locked(&mut m, pool, task_id).await;
    let released = release_by_agent(&mut m, &task, session_id, reason).await?;
    m.commit().await?;

    Ok(released)
}

/// A user clearing a lease (`POST /projects/{pid}/tasks/{id}/release`).
async fn release_user(
    pool: &PgPool,
    project_id: Uuid,
    task_id: Uuid,
    user_id: Uuid,
) -> Result<TaskDto> {
    let mut m = TrackerMutation::begin(pool, project_id, TaskActor::User { user_id }).await?;
    let task = locked(&mut m, pool, task_id).await;
    let released = release_by_user(&mut m, &task).await?;
    m.commit().await?;

    Ok(released)
}

/// A session asking for a person (MCP `needs_human`).
async fn ask_for_human(
    pool: &PgPool,
    project_id: Uuid,
    task_id: Uuid,
    session_id: Uuid,
    reason: &str,
) -> Result<TaskDto> {
    let mut m = TrackerMutation::begin(pool, project_id, TaskActor::Session { session_id }).await?;
    let task = locked(&mut m, pool, task_id).await;
    let answered = needs_human(&mut m, &task, session_id, reason).await?;
    m.commit().await?;

    Ok(answered)
}

// ---- Scenario A: a user's whole working day, over HTTP ----

/// `/api/projects/{pid}/tasks`.
fn tasks_path(pid: Uuid) -> String {
    format!("/api/projects/{pid}/tasks")
}

/// `/api/projects/{pid}/tasks/{id}`.
fn task_path(pid: Uuid, id: Uuid) -> String {
    format!("/api/projects/{pid}/tasks/{id}")
}

/// The `id` of a task response body.
fn id_of(task: &Value) -> Uuid {
    task["id"].as_str().expect("an id").parse().expect("a UUID")
}

/// The `number` of a task response body.
fn number_of(task: &Value) -> i32 {
    i32::try_from(task["number"].as_i64().expect("a number")).expect("a per-project number")
}

#[tokio::test]
async fn scenario_a_a_user_drives_every_rest_write() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "ada").await;
    let pid = project(&app, &user, "apollo").await;

    assert!(
        events_of(&app.pool, pid).await.is_empty(),
        "creating a project writes no task event",
    );

    // (1) A, straight into `ready`.
    let response = app
        .post_as(&user, &tasks_path(pid))
        .json(&json!({ "title": "Rotate the deploy key", "state": "ready" }))
        .await;
    response.assert_status(StatusCode::CREATED);
    let a = response.json::<Value>();
    let (a_id, a_no) = (id_of(&a), number_of(&a));

    // (2) B, behind A: `created`, `dependency_added`, and the `blocked` the
    // open prerequisite makes true.
    let response = app
        .post_as(&user, &tasks_path(pid))
        .json(&json!({ "title": "Redeploy", "depends_on": [a_id.to_string()] }))
        .await;
    response.assert_status(StatusCode::CREATED);
    let b = response.json::<Value>();
    let (b_id, b_no) = (id_of(&b), number_of(&b));

    // (3) The board's columns change: `review` becomes `qa`.
    let response = app
        .put_as(&user, &format!("/api/projects/{pid}/task-states/review"))
        .json(&json!({ "name": "qa" }))
        .await;
    response.assert_status(StatusCode::OK);

    // (4) A comment on A.
    let response = app
        .post_as(&user, &format!("{}/comments", task_path(pid, a_id)))
        .json(&json!({ "body": "the key is in the vault" }))
        .await;
    response.assert_status(StatusCode::CREATED);

    // (5) A second, non-blocking edge B → A.
    let response = app
        .post_as(&user, &format!("{}/dependencies", task_path(pid, b_id)))
        .json(&json!({ "depends_on": a_id.to_string(), "kind": "related" }))
        .await;
    response.assert_status(StatusCode::OK);

    // (6) A closes: the hand-off, and the `unblocked` it owes B.
    let response = app
        .put_as(&user, &task_path(pid, a_id))
        .json(&json!({ "state": "done" }))
        .await;
    response.assert_status(StatusCode::OK);

    // (7) A reopens: the other direction, and B is behind it again.
    let response = app
        .put_as(&user, &task_path(pid, a_id))
        .json(&json!({ "state": "ready" }))
        .await;
    response.assert_status(StatusCode::OK);

    // (8) Two refusals and a no-op, each of which must write nothing at all.
    let before = hygiene(&app.pool, pid).await;

    let response = app
        .post_as(&user, &format!("{}/release", task_path(pid, a_id)))
        .await;
    response.assert_status(StatusCode::CONFLICT);
    assert_eq!(hygiene(&app.pool, pid).await, before, "a 409 release wrote");

    let response = app
        .put_as(&user, &task_path(pid, a_id))
        .json(&json!({ "state": "ready" }))
        .await;
    response.assert_status(StatusCode::OK);
    assert_eq!(
        hygiene(&app.pool, pid).await,
        before,
        "a PUT naming the current state wrote",
    );

    // (9) A is deleted: one `dependency_removed` per edge that went with it,
    // the `unblocked` that leaves B free, and `deleted` last.
    let response = app.delete_as(&user, &task_path(pid, a_id)).await;
    response.assert_status(StatusCode::NO_CONTENT);

    let rows = rows_of(&app.pool, pid).await;
    let events = events_of(&app.pool, pid).await;
    assert_stream_invariants(&rows, &events);

    let numbers: Numbers = vec![(a_id, a_no), (b_id, b_no)];
    expect!(
        described(&events, &numbers),
        [
            (1, "created", "user", Some(a_no), None, None, None),
            (2, "created", "user", Some(b_no), None, None, None),
            (3, "dependency_added", "user", Some(b_no), None, None, None),
            (4, "blocked", "user", Some(b_no), None, None, None),
            (5, "states_changed", "user", None, None, None, None),
            (6, "commented", "user", Some(a_no), None, None, None),
            (7, "dependency_added", "user", Some(b_no), None, None, None),
            (
                8,
                "state_changed",
                "user",
                Some(a_no),
                Some("ready"),
                Some("done"),
                None
            ),
            (9, "unblocked", "user", Some(b_no), None, None, None),
            (
                10,
                "state_changed",
                "user",
                Some(a_no),
                Some("done"),
                Some("ready"),
                None
            ),
            (11, "blocked", "user", Some(b_no), None, None, None),
            (
                12,
                "dependency_removed",
                "user",
                Some(b_no),
                None,
                None,
                None
            ),
            (
                13,
                "dependency_removed",
                "user",
                Some(b_no),
                None,
                None,
                None
            ),
            (14, "unblocked", "user", Some(b_no), None, None, None),
            (15, "deleted", "user", Some(a_no), None, None, None),
        ]
    );

    // The board renamed a column while A was still moving through the others:
    // the names in `from` and `to` are the ones captured at emission time, and
    // `states_changed` carries the list as it then was.
    let states = events[4].states.as_ref().expect("the state list");
    let names: Vec<&str> = states.iter().map(|state| state.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "backlog",
            "ready",
            "qa",
            "merge",
            "needs_human",
            "done",
            "cancelled"
        ],
    );

    // The comment travels with its event, and the `deleted` payload is the
    // actor and nothing else.
    assert_eq!(
        events[5].comment.as_ref().expect("the comment").body,
        "the key is in the vault",
    );
    let deleted = rows.last().expect("the last row");
    assert_eq!(deleted.task_id, Some(a_id));
    assert_eq!(
        deleted.payload.as_object().expect("an object").len(),
        1,
        "the deleted payload carries more than its actor",
    );
}

// ---- Scenario B: two agents, a reaper and a person ----

#[tokio::test]
async fn scenario_b_sessions_claim_release_and_escalate() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "grace").await;
    let pid = project(&app, &user, "gemini").await;
    let user_id = user.user.id;

    sqlx::query("UPDATE projects SET max_attempts = $2 WHERE id = $1")
        .bind(pid)
        .bind(MAX_ATTEMPTS)
        .execute(&app.pool)
        .await
        .expect("the attempt limit is set");

    let profile = default_profile(&app.pool, pid).await;
    let s1 = session(&app.pool, pid, profile).await;
    let s2 = session(&app.pool, pid, profile).await;
    let s3 = session(&app.pool, pid, profile).await;
    let s4 = session(&app.pool, pid, profile).await;
    let served = state_ids(&app.pool, pid, &["ready"]).await;

    let task = create(
        &app.pool,
        pid,
        user_id,
        "Port the importer",
        Some("ready"),
        None,
        Vec::new(),
    )
    .await;
    let (task_id, number) = (task.id, task.number);
    let numbers: Numbers = vec![(task_id, number)];

    // The creation is a user's, so the stream starts with it and this scenario
    // counts from there.
    assert_eq!(events_of(&app.pool, pid).await.len(), 1);

    // S1 takes it.
    claim(&app.pool, pid, task_id, s1, &served)
        .await
        .expect("the first claim wins");

    // Everything a second session could try while S1 holds it writes nothing:
    // a losing claim, a release by a non-holder, and the two reads.
    let before = hygiene(&app.pool, pid).await;

    let conflict = claim(&app.pool, pid, task_id, s2, &served).await;
    assert!(matches!(conflict, Err(Error::Conflict(_))), "{conflict:?}");
    assert_eq!(hygiene(&app.pool, pid).await, before, "a lost claim wrote");

    let conflict = release_agent(&app.pool, pid, task_id, s2, "not mine").await;
    assert!(matches!(conflict, Err(Error::Conflict(_))), "{conflict:?}");
    assert_eq!(
        hygiene(&app.pool, pid).await,
        before,
        "a non-holder's release wrote",
    );

    let summaries: Vec<TaskSummary> = ready_summaries(&app.pool, pid, &served, 20)
        .await
        .expect("the ready read runs");
    assert!(summaries.is_empty(), "a held task is not claimable");
    assert_eq!(hygiene(&app.pool, pid).await, before, "a ready read wrote");

    let detail: Option<TaskDetailDto> = TaskRepository::new(&app.pool)
        .load_task_detail(pid, TaskRef::Id(task_id))
        .await
        .expect("the detail read runs");
    assert!(detail.is_some());
    assert_eq!(hygiene(&app.pool, pid).await, before, "a detail read wrote");

    // S1 gives it back below the limit: a comment and an ordinary release.
    release_agent(
        &app.pool,
        pid,
        task_id,
        s1,
        "the importer needs the new schema",
    )
    .await
    .expect("the first release succeeds");

    // S2 takes it and gives it back *at* the limit, which hands it to a person.
    claim(&app.pool, pid, task_id, s2, &served)
        .await
        .expect("the second claim wins");
    release_agent(&app.pool, pid, task_id, s2, "still no schema")
        .await
        .expect("the second release succeeds");

    // Nobody holds it now, so a user's release is the documented 409.
    let before = hygiene(&app.pool, pid).await;
    let conflict = release_user(&app.pool, pid, task_id, user_id).await;
    assert!(matches!(conflict, Err(Error::Conflict(_))), "{conflict:?}");
    assert_eq!(
        hygiene(&app.pool, pid).await,
        before,
        "a release of an unheld task wrote",
    );

    // A user launches S3 on the escalated task — a launch is not bound by the
    // profile's served states — and S3 then ends.
    launch(&app.pool, pid, task_id, s3, user_id)
        .await
        .expect("the launch claims the task");
    release_leases_for_session(&app.pool, s3, ReleaseReason::SessionEnded)
        .await
        .expect("the orchestrator releases what S3 held");

    // S4 asks for a person on a task already waiting for one: the reason is
    // recorded, and there is no second escalation and no release to announce.
    ask_for_human(
        &app.pool,
        pid,
        task_id,
        s4,
        "the schema decision is not mine",
    )
    .await
    .expect("needs_human records the reason");

    let rows = rows_of(&app.pool, pid).await;
    let events = events_of(&app.pool, pid).await;
    assert_stream_invariants(&rows, &events);

    expect!(
        described(&events, &numbers),
        [
            (1, "created", "user", Some(number), None, None, None),
            (2, "claimed", "session", Some(number), None, None, None),
            (3, "commented", "session", Some(number), None, None, None),
            (
                4,
                "released",
                "session",
                Some(number),
                None,
                None,
                Some("given_back")
            ),
            (5, "claimed", "session", Some(number), None, None, None),
            (6, "commented", "session", Some(number), None, None, None),
            (7, "commented", "system", Some(number), None, None, None),
            (
                8,
                "escalated",
                "session",
                Some(number),
                Some("ready"),
                Some("needs_human"),
                Some("attempt limit reached (2/2):")
            ),
            (9, "claimed", "user", Some(number), None, None, None),
            (10, "commented", "system", Some(number), None, None, None),
            (
                11,
                "released",
                "system",
                Some(number),
                None,
                None,
                Some("session_ended")
            ),
            (12, "commented", "session", Some(number), None, None, None),
            (13, "updated", "session", Some(number), None, None, None),
        ]
    );

    // The escalation's reason is the count *and* the words of the release that
    // reached it, and the same text is on the task.
    let escalated = &events[7];
    assert_eq!(
        escalated.reason.as_deref(),
        Some("attempt limit reached (2/2): still no schema"),
    );
    assert_eq!(
        escalated
            .task
            .as_ref()
            .expect("the task")
            .needs_human_reason,
        escalated.reason,
    );

    // The events name the sessions that caused them.
    assert_eq!(events[1].actor, TaskActor::Session { session_id: s1 });
    assert_eq!(events[4].actor, TaskActor::Session { session_id: s2 });
    assert_eq!(events[8].actor, TaskActor::User { user_id });
    assert_eq!(events[11].actor, TaskActor::Session { session_id: s4 });

    // Every session that worked on the task is linked to it, and only those:
    // the orchestrator's own release is not session work (ADR 0030).
    let links = hygiene(&app.pool, pid).await.touches;
    let mut sessions: Vec<Uuid> = links.iter().map(|(_, session, _)| *session).collect();
    sessions.sort();
    let mut expected = vec![s1, s2, s3, s4];
    expected.sort();
    assert_eq!(sessions, expected);
}

// ---- Scenario C: a parent closes with its last child ----

#[tokio::test]
async fn scenario_c_a_parent_closes_and_stays_closed() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "edsger").await;
    let pid = project(&app, &user, "mercury").await;
    let user_id = user.user.id;

    let parent = create(
        &app.pool,
        pid,
        user_id,
        "Ship the importer",
        None,
        None,
        Vec::new(),
    )
    .await;
    let c1 = create(
        &app.pool,
        pid,
        user_id,
        "Read the CSV",
        None,
        Some(parent.id),
        Vec::new(),
    )
    .await;
    let c2 = create(
        &app.pool,
        pid,
        user_id,
        "Write the rows",
        None,
        Some(parent.id),
        Vec::new(),
    )
    .await;
    // A dependant of the parent, so the parent's own closure has a flip to
    // announce downstream.
    let dependant = create(
        &app.pool,
        pid,
        user_id,
        "Announce the importer",
        None,
        None,
        vec![parent.id],
    )
    .await;

    let numbers: Numbers = vec![
        (parent.id, parent.number),
        (c1.id, c1.number),
        (c2.id, c2.number),
        (dependant.id, dependant.number),
    ];

    // The first child closes: the parent still has an open one, so nothing
    // moves but the child.
    set_state(&app.pool, pid, user_id, c1.id, "done").await;
    assert_eq!(state_of(&app.pool, parent.id).await, "backlog");

    // The last one closes: the parent loses its blocker, then the
    // orchestrator closes it, and the parent's own dependant comes free.
    set_state(&app.pool, pid, user_id, c2.id, "done").await;
    assert_eq!(state_of(&app.pool, parent.id).await, "done");

    // A child reopening never reopens the parent: the parent only learns that
    // it is blocked again.
    set_state(&app.pool, pid, user_id, c1.id, "backlog").await;
    assert_eq!(state_of(&app.pool, parent.id).await, "done");

    let rows = rows_of(&app.pool, pid).await;
    let events = events_of(&app.pool, pid).await;
    assert_stream_invariants(&rows, &events);

    let (p, one, two, d) = (parent.number, c1.number, c2.number, dependant.number);
    expect!(
        described(&events, &numbers),
        [
            (1, "created", "user", Some(p), None, None, None),
            (2, "created", "user", Some(one), None, None, None),
            (3, "blocked", "user", Some(p), None, None, None),
            (4, "created", "user", Some(two), None, None, None),
            (5, "created", "user", Some(d), None, None, None),
            (6, "dependency_added", "user", Some(d), None, None, None),
            (7, "blocked", "user", Some(d), None, None, None),
            (
                8,
                "state_changed",
                "user",
                Some(one),
                Some("backlog"),
                Some("done"),
                None
            ),
            (
                9,
                "state_changed",
                "user",
                Some(two),
                Some("backlog"),
                Some("done"),
                None
            ),
            (10, "unblocked", "user", Some(p), None, None, None),
            (
                11,
                "state_changed",
                "system",
                Some(p),
                Some("backlog"),
                Some("done"),
                None
            ),
            (12, "unblocked", "system", Some(d), None, None, None),
            (
                13,
                "state_changed",
                "user",
                Some(one),
                Some("done"),
                Some("backlog"),
                None
            ),
            (14, "blocked", "user", Some(p), None, None, None),
        ]
    );
}

// ---- the refusals that are not about a lease ----

#[tokio::test]
async fn rejections_and_no_ops_leave_the_stream_alone() {
    let app = TestApp::spawn().await;
    let user = signed_in(&app, "barbara").await;
    let pid = project(&app, &user, "vanguard").await;

    let response = app
        .post_as(&user, &tasks_path(pid))
        .json(&json!({ "title": "Read the manifest" }))
        .await;
    response.assert_status(StatusCode::CREATED);
    let a = response.json::<Value>();
    let a_id = id_of(&a);

    let response = app
        .post_as(&user, &tasks_path(pid))
        .json(&json!({ "title": "Write the manifest", "depends_on": [a_id.to_string()] }))
        .await;
    response.assert_status(StatusCode::CREATED);
    let b_id = id_of(&response.json::<Value>());

    // A child, so that a second level of nesting has something to be refused
    // against.
    let response = app
        .post_as(&user, &tasks_path(pid))
        .json(&json!({ "title": "Parse a line", "parent_id": a_id }))
        .await;
    response.assert_status(StatusCode::CREATED);
    let child_id = id_of(&response.json::<Value>());

    let before = hygiene(&app.pool, pid).await;

    // A `blocks` edge that would close a ring: 409.
    let response = app
        .post_as(&user, &format!("{}/dependencies", task_path(pid, a_id)))
        .json(&json!({ "depends_on": b_id.to_string() }))
        .await;
    response.assert_status(StatusCode::CONFLICT);
    assert_eq!(hygiene(&app.pool, pid).await, before, "a cycle 409 wrote");

    // Nesting is one level: a grandchild is 400.
    let response = app
        .put_as(&user, &task_path(pid, b_id))
        .json(&json!({ "parent_id": child_id }))
        .await;
    response.assert_status(StatusCode::BAD_REQUEST);
    assert_eq!(
        hygiene(&app.pool, pid).await,
        before,
        "a parent-rule 400 wrote",
    );

    // A state the project does not have: 400.
    let response = app
        .put_as(&user, &task_path(pid, b_id))
        .json(&json!({ "state": "reticulating" }))
        .await;
    response.assert_status(StatusCode::BAD_REQUEST);
    assert_eq!(
        hygiene(&app.pool, pid).await,
        before,
        "an unknown-state 400 wrote",
    );

    // An edge that is not there: 404 `dependency not found`.
    let response = app
        .delete_as(
            &user,
            &format!("{}/dependencies/{b_id}?kind=related", task_path(pid, a_id)),
        )
        .await;
    response.assert_status(StatusCode::NOT_FOUND);
    assert_eq!(
        hygiene(&app.pool, pid).await,
        before,
        "a missing-edge 404 wrote",
    );

    // A body that names only values the task already has: 200 and no event.
    let response = app
        .put_as(&user, &task_path(pid, b_id))
        .json(&json!({ "title": "Write the manifest", "state": "backlog" }))
        .await;
    response.assert_status(StatusCode::OK);
    assert_eq!(
        hygiene(&app.pool, pid).await,
        before,
        "a PUT with no effective change wrote",
    );

    // A list and a detail read, which take no part in anyone's mutation.
    app.get_as(&user, &tasks_path(pid))
        .await
        .assert_status(StatusCode::OK);
    app.get_as(&user, &task_path(pid, b_id))
        .await
        .assert_status(StatusCode::OK);
    assert_eq!(hygiene(&app.pool, pid).await, before, "a read wrote");

    let rows = rows_of(&app.pool, pid).await;
    let events = events_of(&app.pool, pid).await;
    assert_stream_invariants(&rows, &events);
}
