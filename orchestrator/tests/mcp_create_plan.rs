//! `create_plan`: a parent, its sub-tasks and their edges filed as one change
//! (`SPEC.md`, "MCP tool contracts" → `create_plan`; ADR 0056; `CLAUDE.md`,
//! "Testing expectations": MCP tests drive the tool handlers through the
//! `rmcp` server in-process with a session bearer token).
//!
//! Each task's own creation rules are `create_task`'s and are asserted in
//! `tests/mcp_create_task.rs` and `tests/tasks_api.rs`; the transaction and
//! notification behaviour is asserted at the `TrackerMutation` seam in
//! `tests/tracker_mutation.rs`. What is pinned here is the batch:
//!
//! - a plan under a new parent lands whole: the answer in input order with the
//!   ref map, prerequisites created (and numbered) first, local and existing
//!   `blocks` edges, every dependant blocked, one batch of events, every task
//!   linked to the caller and the dispatcher's candidate read offering only
//!   what is really startable;
//! - a plan under an existing parent, named by number;
//! - provenance recorded once, on the new parent;
//! - every refusal — a cycle, a duplicated or unknown ref, both parents, a
//!   prerequisite of another project, and a failure part-way through the
//!   creation — creates no task, no edge, no event and no task number.
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use std::collections::BTreeMap;

use common::TestApp;
use common::mcp::{McpClient, code, refused};
use serde_json::{Value, json};
use uuid::Uuid;

use mars_orchestrator::events::{TaskActor, TaskEvent, TaskEventKind};
use mars_orchestrator::models::{NewTask, SessionState, Task, TaskDependencyKind};
use mars_orchestrator::repositories::TaskRepository;
use mars_orchestrator::tracker::leases::ready_candidates;
use mars_orchestrator::tracker::{TaskDto, TrackerMutation};

/// A project with the documented default states and a profile.
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

/// An existing top-level task of the fixture's project, in `ready`.
async fn task(app: &TestApp, fixture: &Fixture, title: &str) -> Task {
    let project_id = fixture.project_id;
    let mut new = NewTask::new(project_id, title).expect("the title parses");
    new.state_id = Some(state_id(app, project_id, "ready").await);

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

async fn state_id(app: &TestApp, project_id: Uuid, name: &str) -> Uuid {
    TaskRepository::new(&app.pool)
        .find_state_by_name(project_id, name)
        .await
        .expect("the state reads")
        .expect("the project has this state")
        .id
}

/// Where the event stream stands right now.
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

/// What a refused plan may not change: the tasks, the edges, the events and
/// the number counter.
async fn footprint(app: &TestApp, project_id: Uuid) -> (i64, i64, i64, i32) {
    // Row counts and the counter column: no interface answers "did anything
    // get written at all".
    sqlx::query_as::<_, (i64, i64, i64, i32)>(
        "SELECT
             (SELECT COUNT(*) FROM tasks WHERE project_id = $1),
             (SELECT COUNT(*) FROM task_dependencies d JOIN tasks t ON t.id = d.task_id
               WHERE t.project_id = $1),
             (SELECT COUNT(*) FROM task_events WHERE project_id = $1),
             (SELECT next_task_number FROM projects WHERE id = $1)",
    )
    .bind(project_id)
    .fetch_one(&app.pool)
    .await
    .expect("the footprint reads")
}

/// The creator column of a task, which the `Task` wire shape does not carry.
async fn created_by_session(app: &TestApp, task_id: Uuid) -> Option<Uuid> {
    // `created_by_session_id` is not part of the `Task` shape.
    sqlx::query_scalar::<_, Option<Uuid>>("SELECT created_by_session_id FROM tasks WHERE id = $1")
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

/// The `{ parent, tasks, refs }` body.
struct Plan {
    parent: Option<TaskDto>,
    tasks: Vec<TaskDto>,
    refs: BTreeMap<String, Uuid>,
}

#[track_caller]
fn plan_of(value: &Value) -> Plan {
    Plan {
        parent: serde_json::from_value(value["parent"].clone()).expect("parent is a Task or null"),
        tasks: serde_json::from_value(value["tasks"].clone()).expect("tasks is a Task list"),
        refs: serde_json::from_value(value["refs"].clone()).expect("refs maps refs to ids"),
    }
}

/// The edges of a kind a task lists, sorted (`Task.depends_on` has no
/// meaningful order).
fn edges(task: &TaskDto, kind: TaskDependencyKind) -> Vec<Uuid> {
    let mut edges: Vec<Uuid> = task
        .depends_on
        .iter()
        .filter(|edge| edge.kind == kind)
        .map(|edge| edge.task_id)
        .collect();
    edges.sort();

    edges
}

// ---- a plan that lands ----

#[tokio::test]
async fn a_plan_under_a_new_parent_lands_whole_with_its_edges_and_one_batch_of_events() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;
    let (client, session_id) = session(&app, &fixture).await;
    let existing = task(&app, &fixture, "the existing migration").await;
    let cursor = since(&app, fixture.project_id).await;

    let plan = plan_of(
        &client
            .call(
                "create_plan",
                json!({
                    "new_parent": { "title": "the epic", "labels": ["epic"] },
                    "tasks": [
                        // Listed before its prerequisite: created after it.
                        { "ref": "api", "title": "the api", "depends_on": ["schema"], "state": "ready" },
                        { "ref": "schema", "title": "the schema", "state": "ready", "priority": 1 },
                        {
                            "ref": "ui",
                            "title": "the ui",
                            "state": "ready",
                            "depends_on": ["api", format!("#{}", existing.number), "api"],
                        },
                    ],
                }),
            )
            .await
            .expect("the plan is filed"),
    );

    let parent = plan.parent.expect("a new parent is answered");
    let [api, schema, ui] = <[TaskDto; 3]>::try_from(plan.tasks).expect("three sub-tasks");

    // The answer is in input order, and the ref map names the same tasks.
    assert_eq!(
        (api.title.as_str(), schema.title.as_str(), ui.title.as_str()),
        ("the api", "the schema", "the ui"),
    );
    assert_eq!(
        plan.refs,
        BTreeMap::from([
            ("api".to_string(), api.id),
            ("schema".to_string(), schema.id),
            ("ui".to_string(), ui.id),
        ]),
    );

    // The parent first, then each sub-task after its prerequisites.
    assert_eq!(parent.number, existing.number + 1);
    assert_eq!(schema.number, existing.number + 2);
    assert_eq!(api.number, existing.number + 3);
    assert_eq!(ui.number, existing.number + 4);

    for sub_task in [&api, &schema, &ui] {
        assert_eq!(sub_task.parent_id, Some(parent.id), "{}", sub_task.title);
    }
    assert_eq!(parent.labels, ["epic"]);
    assert_eq!(parent.state, "backlog", "the project's default state");
    assert_eq!(schema.priority, 1);

    // The edges, local and existing, one per distinct prerequisite.
    assert!(schema.depends_on.is_empty());
    assert_eq!(edges(&api, TaskDependencyKind::Blocks), [schema.id]);
    let mut expected = vec![api.id, existing.id];
    expected.sort();
    assert_eq!(edges(&ui, TaskDependencyKind::Blocks), expected);

    // Every dependant is blocked, and the parent by its open children.
    assert!(!schema.blocked);
    assert!(api.blocked);
    assert!(ui.blocked);
    assert!(parent.blocked);

    // What `create_task` one by one would have written, in creation order, as
    // one batch: the sequences follow each other with nothing in between.
    let written = events(&app, fixture.project_id, cursor).await;
    let kinds: Vec<(TaskEventKind, Option<Uuid>)> = written
        .iter()
        .map(|event| (event.kind, event.task_id))
        .collect();
    assert_eq!(
        kinds,
        [
            (TaskEventKind::Created, Some(parent.id)),
            (TaskEventKind::Created, Some(schema.id)),
            (TaskEventKind::Blocked, Some(parent.id)),
            (TaskEventKind::Created, Some(api.id)),
            (TaskEventKind::DependencyAdded, Some(api.id)),
            (TaskEventKind::Blocked, Some(api.id)),
            (TaskEventKind::Created, Some(ui.id)),
            (TaskEventKind::DependencyAdded, Some(ui.id)),
            (TaskEventKind::DependencyAdded, Some(ui.id)),
            (TaskEventKind::Blocked, Some(ui.id)),
        ],
    );
    let seqs: Vec<i64> = written.iter().map(|event| event.seq).collect();
    assert_eq!(seqs, ((cursor + 1)..=(cursor + 10)).collect::<Vec<_>>());
    assert!(
        written
            .iter()
            .all(|event| event.actor == TaskActor::Session { session_id }),
    );

    for created in [&parent, &api, &schema, &ui] {
        assert_eq!(
            created_by_session(&app, created.id).await,
            Some(session_id),
            "{}",
            created.title,
        );
        assert_eq!(links(&app, created.id).await, 1, "{}", created.title);
    }

    // What the dispatcher's candidate read offers `ready`: the prerequisites
    // that are really startable, never a dependant of the plan.
    let ready = state_id(&app, fixture.project_id, "ready").await;
    let offered: Vec<Uuid> = ready_candidates(&app.pool, fixture.project_id, &[ready], 100)
        .await
        .expect("the candidates read")
        .into_iter()
        .map(|candidate| candidate.summary.id)
        .collect();
    let mut startable = vec![existing.id, schema.id];
    startable.sort();
    let mut offered_sorted = offered.clone();
    offered_sorted.sort();
    assert_eq!(offered_sorted, startable, "{offered:?}");
}

#[tokio::test]
async fn a_plan_hangs_its_sub_tasks_under_an_existing_parent_named_by_number() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;
    let (client, _) = session(&app, &fixture).await;
    let epic = task(&app, &fixture, "the task the planner was launched for").await;

    let plan = plan_of(
        &client
            .call(
                "create_plan",
                json!({
                    "parent": format!("#{}", epic.number),
                    "tasks": [
                        { "ref": "one", "title": "step one" },
                        { "ref": "two", "title": "step two", "depends_on": ["one"] },
                    ],
                }),
            )
            .await
            .expect("an existing top-level task is a parent"),
    );

    let parent = plan.parent.expect("the existing parent is answered");
    assert_eq!(parent.id, epic.id);
    assert!(parent.blocked, "the parent now has open children");
    assert!(
        plan.tasks
            .iter()
            .all(|task| task.parent_id == Some(epic.id))
    );
    assert!(plan.tasks[1].blocked);
}

#[tokio::test]
async fn provenance_is_recorded_once_on_the_new_parent() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;
    let (client, session_id) = session(&app, &fixture).await;
    let held = task(&app, &fixture, "the task being planned").await;
    common::tracker::hold(&app.pool, fixture.project_id, held.id, session_id).await;

    let plan = plan_of(
        &client
            .call(
                "create_plan",
                json!({
                    "new_parent": { "title": "the follow-up" },
                    "tasks": [{ "ref": "a", "title": "a" }, { "ref": "b", "title": "b" }],
                }),
            )
            .await
            .expect("the sole held task is the origin"),
    );

    let parent = plan.parent.expect("a new parent");
    assert_eq!(
        edges(&parent, TaskDependencyKind::DiscoveredFrom),
        [held.id]
    );
    for sub_task in &plan.tasks {
        assert!(
            edges(sub_task, TaskDependencyKind::DiscoveredFrom).is_empty(),
            "a sub-task reaches the origin through its parent",
        );
    }
}

// ---- refusals, each of which leaves nothing ----

/// Call the tool, expect `invalid_argument` with `message`, and prove the
/// project is exactly as it was.
async fn assert_refused(app: &TestApp, fixture: &Fixture, args: Value, message: &str) {
    let (client, _) = session(app, fixture).await;
    let before = footprint(app, fixture.project_id).await;

    let err = refused(client.call("create_plan", args).await);

    assert_eq!(code(&err), "invalid_argument", "{}", err.message);
    assert_eq!(err.message, message);
    assert_eq!(
        footprint(app, fixture.project_id).await,
        before,
        "a refused plan writes no task, edge, event or number",
    );
}

#[tokio::test]
async fn a_cycle_inside_the_batch_is_refused_and_named() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;

    assert_refused(
        &app,
        &fixture,
        json!({
            "tasks": [
                { "ref": "a", "title": "a", "depends_on": ["b"] },
                { "ref": "b", "title": "b", "depends_on": ["a"] },
                { "ref": "c", "title": "c", "depends_on": ["b"] },
            ],
        }),
        "dependency would create a cycle among refs a, b",
    )
    .await;
}

#[tokio::test]
async fn a_duplicated_or_unknown_ref_is_refused() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;

    assert_refused(
        &app,
        &fixture,
        json!({
            "tasks": [{ "ref": "a", "title": "a" }, { "ref": "a", "title": "again" }],
        }),
        r#"ref "a" is used by more than one task"#,
    )
    .await;

    assert_refused(
        &app,
        &fixture,
        json!({
            "tasks": [{ "ref": "a", "title": "a", "depends_on": ["missing"] }],
        }),
        r#"depends_on names "missing", which is neither a ref of this plan nor a task reference"#,
    )
    .await;

    assert_refused(
        &app,
        &fixture,
        json!({ "tasks": [{ "ref": "12", "title": "a number" }] }),
        r#"ref "12" must be 1-64 letters, digits, ".", "_" or "-", and not a task number or UUID"#,
    )
    .await;

    assert_refused(
        &app,
        &fixture,
        json!({ "tasks": [] }),
        "tasks must list 1 to 50 sub-tasks",
    )
    .await;
}

#[tokio::test]
async fn both_parents_at_once_are_refused() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;
    let epic = task(&app, &fixture, "an epic").await;

    assert_refused(
        &app,
        &fixture,
        json!({
            "parent": epic.number,
            "new_parent": { "title": "another epic" },
            "tasks": [{ "ref": "a", "title": "a" }],
        }),
        "pass parent or new_parent, not both",
    )
    .await;
}

#[tokio::test]
async fn a_prerequisite_of_another_project_is_refused_and_creates_nothing() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;
    let elsewhere = seed(&app).await;
    let foreign = task(&app, &elsewhere, "a task of another project").await;

    assert_refused(
        &app,
        &fixture,
        json!({
            "new_parent": { "title": "the epic" },
            "tasks": [
                { "ref": "a", "title": "a" },
                { "ref": "b", "title": "b", "depends_on": ["a", foreign.id.to_string()] },
            ],
        }),
        "dependency must reference tasks of the same project",
    )
    .await;
}

#[tokio::test]
async fn a_failure_part_way_through_rolls_back_every_task_edge_and_event() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;
    let existing = task(&app, &fixture, "an existing task").await;

    // The parent and the first two sub-tasks are created, with an edge and
    // events, before the third's state is found not to exist.
    let (client, _) = session(&app, &fixture).await;
    let before = footprint(&app, fixture.project_id).await;

    let err = refused(
        client
            .call(
                "create_plan",
                json!({
                    "new_parent": { "title": "the epic" },
                    "tasks": [
                        { "ref": "a", "title": "a", "depends_on": [existing.number] },
                        { "ref": "b", "title": "b", "depends_on": ["a"] },
                        { "ref": "c", "title": "c", "depends_on": ["b"], "state": "triage" },
                    ],
                }),
            )
            .await,
    );

    assert_eq!(code(&err), "invalid_argument");
    assert!(
        err.message.starts_with(r#"unknown state "triage""#),
        "{}",
        err.message
    );
    assert_eq!(
        footprint(&app, fixture.project_id).await,
        before,
        "no task, edge, event or number survives the failed plan",
    );
}
