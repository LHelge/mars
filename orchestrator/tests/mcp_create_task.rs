//! `create_task`: an agent filing work it discovered (`SPEC.md`, "MCP tool
//! contracts" → `create_task`; `CLAUDE.md`, "Testing expectations": MCP tests
//! drive the tool handlers through the `rmcp` server in-process with a session
//! bearer token).
//!
//! The creation rules themselves — the number, the parent, the edges, the
//! `blocked` recomputation and the events — are the tracker's and are asserted
//! in `tests/tasks_api.rs` and `tests/tracker_provenance.rs`. What is pinned
//! here is the *boundary*: that a tool call reaches those rules with the
//! calling session as the creator, that the arguments only MCP has — a task
//! reference for `parent` and `depends_on`, and `discovered_from` — are
//! resolved under the project lock, that each refusal carries the documented
//! code, and that a refused call creates neither a task, nor an edge, nor an
//! event, nor a task number.
//!
//! Scenario by scenario:
//!
//! - a minimal call lands the task in the project's default state with
//!   priority 2, an allocated number, `created_by_session_id` set, one
//!   `created` event with actor `session` and the caller linked to the task;
//! - a named state is used, an unknown one is `invalid_argument` naming the
//!   project's states, and a bad priority or title is refused before the lock;
//! - `parent` accepts `#<number>`, one level deep, and a parent that already
//!   has a parent is refused;
//! - `depends_on` becomes `blocks` edges in order, blocks the new task and
//!   emits `dependency_added` per edge and one `blocked`; an entry naming no
//!   task of the project creates nothing at all;
//! - provenance: inferred from the sole held task, absent when none is held,
//!   refused when several are held and none named, taken from an explicit
//!   origin the caller holds, refused for one it does not, left to the parent
//!   link when the origin *is* the parent, and recorded beside a `blocks` edge
//!   to the same task;
//! - and the REST detail endpoint shows the same `depends_on` list the tool
//!   answered with.
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use chrono::Utc;
use common::TestApp;
use common::mcp::McpClient;
use rmcp::model::ErrorData;
use serde_json::{Value, json};
use uuid::Uuid;

use mars_orchestrator::events::{TaskActor, TaskEvent, TaskEventKind};
use mars_orchestrator::models::{NewTask, SessionState, Task, TaskDependencyKind};
use mars_orchestrator::repositories::TaskRepository;
use mars_orchestrator::repositories::tasks::test_support::{StateFields, TaskRepositoryTestExt};
use mars_orchestrator::tracker::{TaskDetailDto, TaskDto, TrackerMutation};

/// The project's default state: the first `queue` state of the documented
/// defaults (`docs/data-model.md`, `task_states`).
const DEFAULT_STATE: &str = "backlog";

/// A project with the documented default states and a profile serving
/// `ready`.
struct Fixture {
    project_id: Uuid,
    profile_id: Uuid,
}

async fn seed(app: &TestApp) -> Fixture {
    let (project_id, profile_id) = app.seed_mcp_project().await;

    let mut mutation = TrackerMutation::begin(&app.pool, project_id, TaskActor::System)
        .await
        .expect("the mutation opens");
    TaskRepository::new(&app.pool)
        .insert_default_states(mutation.conn(), project_id)
        .await
        .expect("the default states insert");
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

/// A task of this project in `ready`, optionally under a parent.
async fn task(app: &TestApp, fixture: &Fixture, title: &str) -> Task {
    insert(app, fixture, title, None).await
}

async fn child_of(app: &TestApp, fixture: &Fixture, title: &str, parent: &Task) -> Task {
    insert(app, fixture, title, Some(parent.id)).await
}

async fn insert(app: &TestApp, fixture: &Fixture, title: &str, parent: Option<Uuid>) -> Task {
    let project_id = fixture.project_id;
    let mut new = NewTask::new(project_id, title).expect("the title parses");
    new.state_id = Some(state_id(app, project_id, "ready").await);
    new.parent_id = parent;

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

/// Hand the lease to a session without going through a claim, so that "held"
/// is a precondition rather than a second assertion.
async fn hold(app: &TestApp, project_id: Uuid, task_id: Uuid, session_id: Uuid) {
    let mut mutation = TrackerMutation::begin(&app.pool, project_id, TaskActor::System)
        .await
        .expect("the mutation opens");
    TaskRepository::new(&app.pool)
        .set_task_state_fields(
            mutation.conn(),
            project_id,
            task_id,
            &StateFields {
                lease: Some(Some((session_id, Utc::now()))),
                ..StateFields::default()
            },
        )
        .await
        .expect("the lease writes");
    mutation.commit().await.expect("the mutation commits");
}

/// Every committed event of the project, oldest first.
async fn events(app: &TestApp, project_id: Uuid) -> Vec<TaskEvent> {
    TaskRepository::new(&app.pool)
        .list_task_events_after(project_id, 0, 100)
        .await
        .expect("the events read")
        .into_iter()
        .map(|row| TaskEvent::from_row(row).expect("the row is a documented event"))
        .collect()
}

/// The kinds of those events, which is usually the whole assertion.
async fn event_kinds(app: &TestApp, project_id: Uuid) -> Vec<TaskEventKind> {
    events(app, project_id)
        .await
        .into_iter()
        .map(|event| event.kind)
        .collect()
}

/// How many tasks the project has — what a refused creation may not change.
async fn task_count(app: &TestApp, project_id: Uuid) -> i64 {
    // The number of rows in `tasks`: no interface answers "did anything get
    // created at all", only "what is on the board".
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM tasks WHERE project_id = $1")
        .bind(project_id)
        .fetch_one(&app.pool)
        .await
        .expect("the count runs")
}

/// The project's task number counter (`docs/data-model.md`, `projects`).
async fn next_number(app: &TestApp, project_id: Uuid) -> i32 {
    // The counter column itself: the API only ever shows numbers it allocated.
    sqlx::query_scalar::<_, i32>("SELECT next_task_number FROM projects WHERE id = $1")
        .bind(project_id)
        .fetch_one(&app.pool)
        .await
        .expect("the counter reads")
}

/// The creator columns of a task (`docs/data-model.md`, `tasks`).
async fn creator(app: &TestApp, task_id: Uuid) -> (Option<Uuid>, Option<Uuid>) {
    // Neither column is part of the `Task` wire shape, and exactly one of them
    // must be set.
    sqlx::query_as::<_, (Option<Uuid>, Option<Uuid>)>(
        "SELECT created_by_user_id, created_by_session_id FROM tasks WHERE id = $1",
    )
    .bind(task_id)
    .fetch_one(&app.pool)
    .await
    .expect("the row reads")
}

/// How many `task_sessions` links a task has.
async fn links(app: &TestApp, task_id: Uuid) -> i64 {
    // The link table is not exposed by the tool's output.
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM task_sessions WHERE task_id = $1")
        .bind(task_id)
        .fetch_one(&app.pool)
        .await
        .expect("the count runs")
}

/// The `data.code` every tool failure carries (`SPEC.md`, "MCP tool
/// contracts").
fn code(err: &ErrorData) -> String {
    err.data
        .as_ref()
        .and_then(|data| data.get("code"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("every tool error carries data.code: {err:?}"))
        .to_string()
}

/// The failure a call answered with, or a panic naming what came back instead.
fn refused(result: std::result::Result<Value, ErrorData>) -> ErrorData {
    match result {
        Ok(value) => panic!("expected a refusal, got {value}"),
        Err(err) => err,
    }
}

/// The `{ task: Task }` body a tool answered with.
fn task_of(value: &Value) -> TaskDto {
    serde_json::from_value(value["task"].clone()).expect("the output carries a Task")
}

/// The edges of a kind the answer lists, in order.
fn edges(task: &TaskDto, kind: TaskDependencyKind) -> Vec<Uuid> {
    task.depends_on
        .iter()
        .filter(|edge| edge.kind == kind)
        .map(|edge| edge.task_id)
        .collect()
}

// ---- the plain creation ----

#[tokio::test]
async fn a_minimal_creation_lands_in_the_default_state_and_names_the_session() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;
    let (client, session_id) = session(&app, &fixture).await;

    let created = task_of(
        &client
            .call("create_task", json!({ "title": "found along the way" }))
            .await
            .expect("the creation succeeds"),
    );

    assert_eq!(created.title, "found along the way");
    assert_eq!(created.description, "");
    assert_eq!(created.state, DEFAULT_STATE);
    assert_eq!(created.priority, 2);
    assert!(!created.blocked);
    assert!(created.depends_on.is_empty());
    assert!(created.number >= 1, "a number is allocated: {created:?}");

    assert_eq!(
        creator(&app, created.id).await,
        (None, Some(session_id)),
        "exactly one creator column, and it is the calling session",
    );

    let written = events(&app, fixture.project_id).await;
    assert_eq!(written.len(), 1, "a plain creation emits one event");
    let event = written.last().expect("the event is there");
    assert_eq!(event.kind, TaskEventKind::Created);
    assert_eq!(event.task_id, Some(created.id));
    assert_eq!(event.actor, TaskActor::Session { session_id });

    assert_eq!(
        links(&app, created.id).await,
        1,
        "the creating session is linked to the task it filed",
    );
}

#[tokio::test]
async fn a_named_state_is_used_and_an_unknown_one_names_the_project_states() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;
    let (client, _) = session(&app, &fixture).await;

    let created = task_of(
        &client
            .call(
                "create_task",
                json!({ "title": "ready to go", "state": "ready" }),
            )
            .await
            .expect("a project state is accepted"),
    );
    assert_eq!(created.state, "ready");

    let err = refused(
        client
            .call(
                "create_task",
                json!({ "title": "nowhere", "state": "triage" }),
            )
            .await,
    );

    assert_eq!(code(&err), "invalid_argument");
    assert!(
        err.message.starts_with(r#"unknown state "triage""#) && err.message.contains("backlog"),
        "the valid names are listed: {}",
        err.message,
    );
    assert_eq!(task_count(&app, fixture.project_id).await, 1);
}

#[tokio::test]
async fn a_bad_priority_or_title_is_refused_before_anything_is_written() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;
    let (client, _) = session(&app, &fixture).await;

    let before = next_number(&app, fixture.project_id).await;

    let priority = refused(
        client
            .call(
                "create_task",
                json!({ "title": "too urgent", "priority": 4 }),
            )
            .await,
    );
    assert_eq!(code(&priority), "invalid_argument");
    assert_eq!(
        priority.message,
        "priority must be between 0 (critical) and 3 (low)",
    );

    let title = refused(
        client
            .call("create_task", json!({ "title": "t".repeat(201) }))
            .await,
    );
    assert_eq!(code(&title), "invalid_argument");
    assert_eq!(title.message, "title must be 1-200 characters");

    assert_eq!(task_count(&app, fixture.project_id).await, 0);
    assert_eq!(next_number(&app, fixture.project_id).await, before);
    assert!(events(&app, fixture.project_id).await.is_empty());
}

// ---- parents ----

#[tokio::test]
async fn a_parent_is_named_by_number_and_may_already_have_children() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;
    let (client, _) = session(&app, &fixture).await;

    let parent = task(&app, &fixture, "the plan").await;
    child_of(&app, &fixture, "the first step", &parent).await;

    let created = task_of(
        &client
            .call(
                "create_task",
                json!({ "title": "the second step", "parent": format!("#{}", parent.number) }),
            )
            .await
            .expect("a top-level task is a parent"),
    );

    assert_eq!(created.parent_id, Some(parent.id));
}

#[tokio::test]
async fn a_parent_that_has_a_parent_is_refused_and_creates_nothing() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;
    let (client, _) = session(&app, &fixture).await;

    let parent = task(&app, &fixture, "the plan").await;
    let child = child_of(&app, &fixture, "the first step", &parent).await;
    let before = (
        task_count(&app, fixture.project_id).await,
        next_number(&app, fixture.project_id).await,
    );

    let err = refused(
        client
            .call(
                "create_task",
                json!({ "title": "a grandchild", "parent": child.number }),
            )
            .await,
    );

    assert_eq!(code(&err), "invalid_argument", "{}", err.message);
    assert_eq!(
        (
            task_count(&app, fixture.project_id).await,
            next_number(&app, fixture.project_id).await,
        ),
        before,
        "one level only, and the refusal allocates no number",
    );
}

#[tokio::test]
async fn a_parent_naming_no_task_of_the_project_is_not_found() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;
    let (client, _) = session(&app, &fixture).await;

    let err = refused(
        client
            .call(
                "create_task",
                json!({ "title": "an orphan", "parent": 9999 }),
            )
            .await,
    );

    assert_eq!(code(&err), "not_found");
    assert_eq!(err.message, "parent task not found");
    assert_eq!(task_count(&app, fixture.project_id).await, 0);
}

// ---- dependencies ----

#[tokio::test]
async fn depends_on_creates_one_blocks_edge_per_entry_and_blocks_the_task() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;
    let (client, _) = session(&app, &fixture).await;

    let first = task(&app, &fixture, "the migration").await;
    let second = task(&app, &fixture, "the backfill").await;

    let created = task_of(
        &client
            .call(
                "create_task",
                json!({
                    "title": "the cutover",
                    "depends_on": [format!("#{}", first.number), second.number],
                }),
            )
            .await
            .expect("both prerequisites resolve"),
    );

    assert_eq!(
        edges(&created, TaskDependencyKind::Blocks),
        [first.id, second.id],
        "one edge per entry, in the order they were given",
    );
    assert!(created.blocked, "both prerequisites are open");

    assert_eq!(
        event_kinds(&app, fixture.project_id).await,
        [
            TaskEventKind::Created,
            TaskEventKind::DependencyAdded,
            TaskEventKind::DependencyAdded,
            TaskEventKind::Blocked,
        ],
    );
}

#[tokio::test]
async fn a_depends_on_entry_naming_no_task_creates_neither_task_nor_number() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;
    let (client, _) = session(&app, &fixture).await;

    let known = task(&app, &fixture, "the migration").await;
    let before = next_number(&app, fixture.project_id).await;

    let err = refused(
        client
            .call(
                "create_task",
                json!({ "title": "the cutover", "depends_on": [known.number, 9999] }),
            )
            .await,
    );

    assert_eq!(code(&err), "invalid_argument", "{}", err.message);
    assert_eq!(
        task_count(&app, fixture.project_id).await,
        1,
        "only the prerequisite that was there before",
    );
    assert_eq!(next_number(&app, fixture.project_id).await, before);
    assert!(events(&app, fixture.project_id).await.is_empty());
}

// ---- discovery provenance ----

#[tokio::test]
async fn the_sole_held_task_is_the_origin_when_none_is_named() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;
    let (client, session_id) = session(&app, &fixture).await;

    let held = task(&app, &fixture, "implement it").await;
    hold(&app, fixture.project_id, held.id, session_id).await;

    let created = task_of(
        &client
            .call("create_task", json!({ "title": "a bug in the parser" }))
            .await
            .expect("the creation succeeds"),
    );

    assert_eq!(
        edges(&created, TaskDependencyKind::DiscoveredFrom),
        [held.id],
    );
    assert!(!created.blocked, "provenance does not block");
    assert_eq!(
        event_kinds(&app, fixture.project_id).await,
        [TaskEventKind::Created, TaskEventKind::DependencyAdded],
    );
}

#[tokio::test]
async fn a_session_holding_nothing_records_no_provenance() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;
    let (client, _) = session(&app, &fixture).await;

    task(&app, &fixture, "somebody else's work").await;

    let created = task_of(
        &client
            .call("create_task", json!({ "title": "a bug in the parser" }))
            .await
            .expect("the creation succeeds"),
    );

    assert!(created.depends_on.is_empty());
    assert_eq!(
        event_kinds(&app, fixture.project_id).await,
        [TaskEventKind::Created],
    );
}

#[tokio::test]
async fn a_session_holding_several_tasks_must_name_the_origin() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;
    let (client, session_id) = session(&app, &fixture).await;

    let first = task(&app, &fixture, "implement it").await;
    let second = task(&app, &fixture, "document it").await;
    hold(&app, fixture.project_id, first.id, session_id).await;
    hold(&app, fixture.project_id, second.id, session_id).await;
    let before = next_number(&app, fixture.project_id).await;

    let err = refused(
        client
            .call("create_task", json!({ "title": "a bug in the parser" }))
            .await,
    );

    assert_eq!(code(&err), "invalid_argument");
    assert!(
        err.message.contains("discovered_from"),
        "the agent is told what to pass: {}",
        err.message,
    );
    assert_eq!(task_count(&app, fixture.project_id).await, 2);
    assert_eq!(next_number(&app, fixture.project_id).await, before);
    assert!(events(&app, fixture.project_id).await.is_empty());

    // Naming one of them is the way through.
    let created = task_of(
        &client
            .call(
                "create_task",
                json!({ "title": "a bug in the parser", "discovered_from": second.number }),
            )
            .await
            .expect("a held origin is accepted"),
    );
    assert_eq!(
        edges(&created, TaskDependencyKind::DiscoveredFrom),
        [second.id],
    );
}

#[tokio::test]
async fn an_origin_another_session_holds_is_refused() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;
    let (client, _) = session(&app, &fixture).await;
    let (_other, other_session) = session(&app, &fixture).await;

    let theirs = task(&app, &fixture, "implement it").await;
    hold(&app, fixture.project_id, theirs.id, other_session).await;

    let err = refused(
        client
            .call(
                "create_task",
                json!({ "title": "a bug in the parser", "discovered_from": theirs.number }),
            )
            .await,
    );

    assert_eq!(code(&err), "invalid_argument");
    assert!(
        err.message.contains("discovered_from"),
        "the refusal is about the argument: {}",
        err.message,
    );
    assert_eq!(task_count(&app, fixture.project_id).await, 1);
    assert!(events(&app, fixture.project_id).await.is_empty());
}

#[tokio::test]
async fn an_origin_that_is_the_parent_records_no_second_edge() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;
    let (client, session_id) = session(&app, &fixture).await;

    let held = task(&app, &fixture, "the plan").await;
    hold(&app, fixture.project_id, held.id, session_id).await;

    let created = task_of(
        &client
            .call(
                "create_task",
                json!({ "title": "the first step", "parent": held.number }),
            )
            .await
            .expect("the creation succeeds"),
    );

    assert_eq!(created.parent_id, Some(held.id));
    assert!(
        created.depends_on.is_empty(),
        "the parent link already records where it came from",
    );
    // Two events, and neither is a `dependency_added`: the second is the
    // *parent* becoming blocked by the open child it just gained
    // (`ARCHITECTURE.md`, "Task tracker" → "Parents").
    let written = events(&app, fixture.project_id).await;
    assert_eq!(
        written
            .iter()
            .map(|event| (event.kind, event.task_id))
            .collect::<Vec<_>>(),
        [
            (TaskEventKind::Created, Some(created.id)),
            (TaskEventKind::Blocked, Some(held.id)),
        ],
    );
}

#[tokio::test]
async fn a_blocks_edge_to_the_origin_coexists_with_the_provenance_edge() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;
    let (client, session_id) = session(&app, &fixture).await;

    let held = task(&app, &fixture, "implement it").await;
    hold(&app, fixture.project_id, held.id, session_id).await;

    let created = task_of(
        &client
            .call(
                "create_task",
                json!({ "title": "review the implementation", "depends_on": [held.number] }),
            )
            .await
            .expect("the creation succeeds"),
    );

    assert_eq!(edges(&created, TaskDependencyKind::Blocks), [held.id]);
    assert_eq!(
        edges(&created, TaskDependencyKind::DiscoveredFrom),
        [held.id],
        "the same pair carries both kinds",
    );
    assert!(created.blocked, "only the `blocks` edge blocks");

    assert_eq!(
        event_kinds(&app, fixture.project_id).await,
        [
            TaskEventKind::Created,
            TaskEventKind::DependencyAdded,
            TaskEventKind::DependencyAdded,
            TaskEventKind::Blocked,
        ],
    );

    // The same task, read back over REST: one shape, not two (`SPEC.md`,
    // "Tasks").
    let user = app
        .create_admin("boss", "boss@example.test", "correct horse battery")
        .await;
    let detail: TaskDetailDto = app
        .get_as(
            &user,
            &format!("/api/projects/{}/tasks/{}", fixture.project_id, created.id),
        )
        .await
        .json();

    assert_eq!(detail.task.depends_on, created.depends_on);
}
