//! `update`, the agent's hand-off tool (`SPEC.md`, "MCP tool contracts" →
//! `update`; "Code hand-offs and review").
//!
//! The tracker's own rules — what a state change does to the lease and the
//! counters, what closing a task does to its dependants, the one-level parent
//! rules, the cycle check — are asserted against the tracker in
//! `tests/tracker_tasks.rs` and `tests/tracker_events.rs`, and the publication
//! protocol against the service in `tests/handoffs_service.rs`. What is pinned
//! here is the half this tool owns:
//!
//! - **authority**, which is the transport's and nobody else's: the holder may
//!   change anything, the creator of a task nobody holds may change the five
//!   fields the contract names, and everything else is the refusal the other
//!   lease tools give, in the same words;
//! - **the arguments an agent may write** — a state by name, a parent by
//!   number, `"#7"` in `add_depends_on`, `labels: []`, `parent: null` — read
//!   the way the document says;
//! - **the codes**, including the one refusal whose code differs from REST: a
//!   dependency cycle is `invalid_argument` here and 409 there;
//! - **the promise the intro paragraph makes about every tool**: a successful
//!   call commits its events and its `task_sessions` link together, a rejected
//!   one writes nothing, and an update with no effective change is not a
//!   rejection but writes nothing either (ADR 0030);
//! - **that the hand-off path is the service's**, reached with the calling
//!   session as the caller, so the source session of a revision is the caller
//!   by construction and the publication needs no git-tool permission.
//!
//! The hand-off scenarios use real repositories and real work clones
//! (`common::handoffs::Fixture`); git is never mocked (`CLAUDE.md`, "Testing
//! expectations").
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use chrono::Utc;
use common::TestApp;
use common::handoffs::Fixture as GitFixture;
use common::mcp::McpClient;
use rmcp::model::ErrorData;
use serde_json::{Value, json};
use uuid::Uuid;

use mars_orchestrator::events::{TaskActor, TaskEvent, TaskEventKind};
use mars_orchestrator::git::testutil::test_identity;
use mars_orchestrator::git::{create_work_clone, resolve_base};
use mars_orchestrator::models::{
    NewTask, ReviewStatus, SessionState, Task, TaskDependencyKind, TaskHandoff,
};
use mars_orchestrator::repositories::TaskRepository;
use mars_orchestrator::repositories::tasks::test_support::{StateFields, TaskRepositoryTestExt};
use mars_orchestrator::tracker::graph::recompute_blocked;
use mars_orchestrator::tracker::{TaskDto, TrackerMutation};

/// The refusal a caller that neither holds nor created the task gets
/// (`tracker::leases::NOT_HELD_BY_SESSION`, shared with `release`).
const NOT_HELD_BY_SESSION: &str = "task is not held by this session";

/// The project's default states, in `position` order: what an unknown state
/// name is answered with.
const ALL_STATES: &str = "backlog, ready, review, merge, needs_human, done, cancelled";

// ---- the tracker-only fixture ----

/// A project with the default states and a profile serving `ready`, plus the
/// calling session and its client.
///
/// No repository: everything up to the hand-off scenarios is tracker work, and
/// a project without one is exactly what those scenarios never touch.
struct Board {
    app: TestApp,
    project_id: Uuid,
    profile_id: Uuid,
    client: McpClient,
    caller: Uuid,
}

impl Board {
    async fn create() -> Board {
        let app = TestApp::spawn().await;
        let (project_id, profile_id) = app.seed_mcp_project().await;

        let mut mutation = TrackerMutation::begin(&app.pool, project_id, TaskActor::System)
            .await
            .expect("the mutation opens");
        TaskRepository::new(&app.pool)
            .insert_default_states(mutation.conn(), project_id)
            .await
            .expect("the default states insert");
        mutation.commit().await.expect("the mutation commits");

        let seeded = app
            .seed_mcp_session(project_id, profile_id, SessionState::Running)
            .await;
        let client = McpClient::connect(&app, &seeded.token)
            .await
            .expect("a running session's token authenticates");

        Board {
            app,
            project_id,
            profile_id,
            client,
            caller: seeded.session_id,
        }
    }

    /// A second session of the same project, with a client of its own.
    async fn other_session(&self) -> (McpClient, Uuid) {
        let seeded = self
            .app
            .seed_mcp_session(self.project_id, self.profile_id, SessionState::Running)
            .await;
        let client = McpClient::connect(&self.app, &seeded.token)
            .await
            .expect("a running session's token authenticates");

        (client, seeded.session_id)
    }

    async fn update(&self, args: Value) -> Result<Value, ErrorData> {
        self.client.call("update", args).await
    }

    /// A task of this project in `state`, created by nobody in particular.
    async fn task(&self, title: &str, state: &str) -> Task {
        self.insert(title, state, None, None).await
    }

    /// A task the calling session created, which is the creator exception's
    /// precondition.
    async fn task_created_by(&self, title: &str, state: &str, session_id: Uuid) -> Task {
        self.insert(title, state, Some(session_id), None).await
    }

    async fn insert(
        &self,
        title: &str,
        state: &str,
        created_by_session_id: Option<Uuid>,
        parent_id: Option<Uuid>,
    ) -> Task {
        let mut new = NewTask::new(self.project_id, title).expect("the title parses");
        new.state_id = Some(self.state_id(state).await);
        new.created_by_session_id = created_by_session_id;
        new.parent_id = parent_id;

        let mut mutation =
            TrackerMutation::begin(&self.app.pool, self.project_id, TaskActor::System)
                .await
                .expect("the mutation opens");
        let inserted = TaskRepository::new(&self.app.pool)
            .insert_task(mutation.conn(), self.project_id, &new)
            .await
            .expect("the task inserts");
        mutation.commit().await.expect("the mutation commits");

        inserted
    }

    async fn state_id(&self, name: &str) -> Uuid {
        TaskRepository::new(&self.app.pool)
            .find_state_by_name(self.project_id, name)
            .await
            .expect("the state reads")
            .expect("the project has this state")
            .id
    }

    /// Put the lease on a session without going through a claim, so that
    /// "held" is a precondition rather than a second assertion.
    async fn hold(&self, task_id: Uuid, session_id: Uuid) {
        self.set_fields(
            task_id,
            StateFields {
                lease: Some(Some((session_id, Utc::now()))),
                ..StateFields::default()
            },
        )
        .await;
    }

    async fn set_fields(&self, task_id: Uuid, fields: StateFields) {
        let mut mutation =
            TrackerMutation::begin(&self.app.pool, self.project_id, TaskActor::System)
                .await
                .expect("the mutation opens");
        TaskRepository::new(&self.app.pool)
            .set_task_state_fields(mutation.conn(), self.project_id, task_id, &fields)
            .await
            .expect("the fields write");
        mutation.commit().await.expect("the mutation commits");
    }

    /// One edge of a kind, with the `blocked` flag it implies.
    async fn edge(&self, dependant: Uuid, prerequisite: Uuid, kind: TaskDependencyKind) {
        let mut mutation =
            TrackerMutation::begin(&self.app.pool, self.project_id, TaskActor::System)
                .await
                .expect("the mutation opens");
        TaskRepository::new(&self.app.pool)
            .insert_dependency(
                mutation.conn(),
                self.project_id,
                dependant,
                prerequisite,
                kind,
            )
            .await
            .expect("the edge inserts");
        recompute_blocked(&mut mutation, &[dependant])
            .await
            .expect("the flag is recomputed");
        mutation.commit().await.expect("the mutation commits");
    }

    async fn read(&self, task_id: Uuid) -> TaskDto {
        read(&self.app, self.project_id, task_id).await
    }

    async fn events(&self) -> Vec<TaskEvent> {
        events(&self.app, self.project_id).await
    }

    async fn event_kinds(&self) -> Vec<TaskEventKind> {
        self.event_kinds_after(0).await
    }

    /// The kinds after the first `arranged` events.
    ///
    /// Arranging a `blocks` edge recomputes the flag and therefore emits a
    /// `blocked` of its own; a scenario about what the *tool* emitted counts
    /// from after its arrangement rather than from the start of the project.
    async fn event_kinds_after(&self, arranged: usize) -> Vec<TaskEventKind> {
        self.events()
            .await
            .into_iter()
            .skip(arranged)
            .map(|event| event.kind)
            .collect()
    }

    async fn links(&self, task_id: Uuid) -> i64 {
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM task_sessions WHERE task_id = $1")
            .bind(task_id)
            .fetch_one(&self.app.pool)
            .await
            .expect("the count reads")
    }
}

// ---- shared assertions ----

async fn read(app: &TestApp, project_id: Uuid, task_id: Uuid) -> TaskDto {
    TaskRepository::new(&app.pool)
        .load_task_dto(project_id, task_id)
        .await
        .expect("the task reads")
        .expect("the task is in this project")
}

async fn events(app: &TestApp, project_id: Uuid) -> Vec<TaskEvent> {
    TaskRepository::new(&app.pool)
        .list_task_events_after(project_id, 0, 200)
        .await
        .expect("the events read")
        .into_iter()
        .map(|row| TaskEvent::from_row(row).expect("the row is a documented event"))
        .collect()
}

/// The `data.code` every tool failure carries (`SPEC.md`, "MCP tool
/// contracts").
#[track_caller]
fn code(err: &ErrorData) -> String {
    err.data
        .as_ref()
        .and_then(|data| data.get("code"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("every tool error carries data.code: {err:?}"))
        .to_string()
}

/// The `{ task }` body of a successful call.
#[track_caller]
fn task_of(value: &Value) -> TaskDto {
    serde_json::from_value(value["task"].clone()).expect("the output is { task: Task }")
}

// ---- path A: the ordinary update ----

#[tokio::test]
async fn the_holder_changes_fields_and_gets_one_updated_event_and_a_link() {
    let board = Board::create().await;
    let task = board.task("Wire the tracker", "ready").await;
    board.hold(task.id, board.caller).await;

    let output = board
        .update(json!({
            "task": task.number,
            "title": "Wire the tracker properly",
            "priority": 1,
        }))
        .await
        .expect("the holder may change fields");

    let returned = task_of(&output);
    assert_eq!(returned.title, "Wire the tracker properly");
    assert_eq!(returned.priority, 1);

    assert_eq!(board.event_kinds().await, vec![TaskEventKind::Updated]);
    assert_eq!(
        board.events().await[0].actor,
        TaskActor::Session {
            session_id: board.caller
        },
    );
    assert_eq!(board.links(task.id).await, 1);
}

#[tokio::test]
async fn a_session_that_neither_holds_nor_created_the_task_is_refused() {
    let board = Board::create().await;
    let (_other_client, other) = board.other_session().await;
    let task = board.task("Somebody else's", "ready").await;
    board.hold(task.id, other).await;

    let before = board.read(task.id).await;
    let err = board
        .update(json!({ "task": task.number, "title": "mine now" }))
        .await
        .expect_err("only the holder may change a held task");

    assert_eq!(code(&err), "conflict");
    assert_eq!(err.message, NOT_HELD_BY_SESSION);
    assert_eq!(board.events().await.len(), 0);
    assert_eq!(board.links(task.id).await, 0);
    assert_eq!(board.read(task.id).await, before);
}

#[tokio::test]
async fn an_unheld_task_that_nobody_created_is_not_the_callers_to_edit() {
    let board = Board::create().await;
    let task = board.task("Nobody's", "ready").await;

    let err = board
        .update(json!({ "task": task.number, "labels": ["backend"] }))
        .await
        .expect_err("the creator exception needs a creator");

    assert_eq!(code(&err), "conflict");
    assert_eq!(err.message, NOT_HELD_BY_SESSION);
    assert_eq!(board.events().await.len(), 0);
}

#[tokio::test]
async fn the_creator_of_an_unheld_task_edits_the_five_fields_and_nothing_else() {
    let board = Board::create().await;
    let task = board
        .task_created_by("Found along the way", "backlog", board.caller)
        .await;
    let prerequisite = board.task("Do this first", "ready").await;

    let output = board
        .update(json!({
            "task": task.number,
            "labels": ["backend"],
            "description": "what I found",
            "add_depends_on": [prerequisite.number],
        }))
        .await
        .expect("the creator may edit the five fields of a task nobody holds");

    let returned = task_of(&output);
    assert_eq!(returned.labels, vec!["backend".to_string()]);
    assert_eq!(returned.depends_on.len(), 1);

    // The state is not one of the five.
    let err = board
        .update(json!({ "task": task.number, "state": "ready" }))
        .await
        .expect_err("the creator exception does not reach the state");
    assert_eq!(code(&err), "conflict");
    assert_eq!(err.message, NOT_HELD_BY_SESSION);

    // Nor is the priority, nor the parent.
    for field in [json!({ "priority": 0 }), json!({ "parent": null })] {
        let mut args = json!({ "task": task.number });
        for (key, value) in field.as_object().expect("an object") {
            args[key] = value.clone();
        }
        let err = board
            .update(args)
            .await
            .expect_err("the creator exception is exactly five fields");
        assert_eq!(code(&err), "conflict", "{field}");
    }

    assert_eq!(board.read(task.id).await.state, "backlog");
}

#[tokio::test]
async fn a_held_task_is_not_its_creators_to_edit() {
    let board = Board::create().await;
    let (_other_client, other) = board.other_session().await;
    let task = board
        .task_created_by("Mine, once", "ready", board.caller)
        .await;
    board.hold(task.id, other).await;

    let err = board
        .update(json!({ "task": task.number, "labels": [] }))
        .await
        .expect_err("the creator exception needs the task to be unheld");

    assert_eq!(code(&err), "conflict");
    assert_eq!(err.message, NOT_HELD_BY_SESSION);
}

#[tokio::test]
async fn an_unknown_state_lists_the_projects_states_in_position_order() {
    let board = Board::create().await;
    let task = board.task("Wire the tracker", "ready").await;
    board.hold(task.id, board.caller).await;

    let err = board
        .update(json!({ "task": task.number, "state": "in_progress" }))
        .await
        .expect_err("the state must be one of the project's");

    assert_eq!(code(&err), "invalid_argument");
    assert_eq!(
        err.message,
        format!("unknown state \"in_progress\"; valid states are: {ALL_STATES}"),
    );
    assert_eq!(board.events().await.len(), 0);
}

#[tokio::test]
async fn a_different_state_releases_the_lease_and_resets_the_attempts() {
    let board = Board::create().await;
    let task = board.task("Wire the tracker", "ready").await;
    board.hold(task.id, board.caller).await;
    board
        .set_fields(
            task.id,
            StateFields {
                attempts: Some(2),
                ..StateFields::default()
            },
        )
        .await;

    let output = board
        .update(json!({ "task": task.number, "state": "review" }))
        .await
        .expect("the holder hands the task off");

    let returned = task_of(&output);
    assert_eq!(returned.state, "review");
    assert_eq!(returned.lease_holder_session_id, None);
    assert_eq!(returned.attempts, 0);

    let events = board.events().await;
    assert_eq!(
        events.iter().map(|e| e.kind).collect::<Vec<_>>(),
        vec![TaskEventKind::StateChanged],
    );
    assert_eq!(events[0].from.as_deref(), Some("ready"));
    assert_eq!(events[0].to.as_deref(), Some("review"));
}

#[tokio::test]
async fn a_terminal_state_closes_the_task_and_unblocks_what_waited_on_it() {
    let board = Board::create().await;
    let prerequisite = board.task("Do this first", "ready").await;
    let dependant = board.task("Then this", "ready").await;
    board
        .edge(dependant.id, prerequisite.id, TaskDependencyKind::Blocks)
        .await;
    assert!(board.read(dependant.id).await.blocked);
    let arranged = board.events().await.len();

    board.hold(prerequisite.id, board.caller).await;

    let output = board
        .update(json!({ "task": prerequisite.number, "state": "done" }))
        .await
        .expect("the holder closes the task");

    assert!(task_of(&output).closed_at.is_some());
    assert!(!board.read(dependant.id).await.blocked);
    assert_eq!(
        board.event_kinds_after(arranged).await,
        vec![TaskEventKind::StateChanged, TaskEventKind::Unblocked],
    );
}

#[tokio::test]
async fn reopening_a_closed_task_clears_closed_at() {
    let board = Board::create().await;
    let task = board.task("Done too soon", "ready").await;
    board.hold(task.id, board.caller).await;
    board
        .update(json!({ "task": task.number, "state": "done" }))
        .await
        .expect("the holder closes the task");
    assert!(board.read(task.id).await.closed_at.is_some());
    board.hold(task.id, board.caller).await;

    let output = board
        .update(json!({ "task": task.number, "state": "ready" }))
        .await
        .expect("a terminal task can be reopened");

    assert_eq!(task_of(&output).closed_at, None);
}

#[tokio::test]
async fn the_current_state_is_a_no_op_that_keeps_the_lease_and_the_counters() {
    let board = Board::create().await;
    let task = board.task("Wire the tracker", "ready").await;
    board.hold(task.id, board.caller).await;
    board
        .set_fields(
            task.id,
            StateFields {
                attempts: Some(2),
                ..StateFields::default()
            },
        )
        .await;

    let output = board
        .update(json!({
            "task": task.number,
            "state": "ready",
            "title": "Wire the tracker properly",
        }))
        .await
        .expect("assigning the current state is not a refusal");

    let returned = task_of(&output);
    assert_eq!(returned.title, "Wire the tracker properly");
    assert_eq!(returned.state, "ready");
    assert_eq!(returned.lease_holder_session_id, Some(board.caller));
    assert_eq!(returned.attempts, 2);
    assert_eq!(board.event_kinds().await, vec![TaskEventKind::Updated]);
}

#[tokio::test]
async fn a_parent_that_already_has_a_parent_is_refused_and_null_detaches() {
    let board = Board::create().await;
    let grandparent = board.task("The epic", "backlog").await;
    let parent = board
        .insert("The sub-epic", "backlog", None, Some(grandparent.id))
        .await;
    let task = board.task("The work", "ready").await;
    board.hold(task.id, board.caller).await;

    let err = board
        .update(json!({ "task": task.number, "parent": parent.number }))
        .await
        .expect_err("nesting is one level deep");
    assert_eq!(code(&err), "invalid_argument");
    assert_eq!(err.message, "a task with a parent cannot receive children");
    assert_eq!(board.events().await.len(), 0);

    // A task cannot be its own parent either.
    let err = board
        .update(json!({ "task": task.number, "parent": task.number }))
        .await
        .expect_err("a task cannot be its own parent");
    assert_eq!(code(&err), "invalid_argument");
    assert_eq!(err.message, "a task cannot be its own parent");

    // A parent that names no task of this project is not found.
    let err = board
        .update(json!({ "task": task.number, "parent": 9999 }))
        .await
        .expect_err("an unknown parent is not found");
    assert_eq!(code(&err), "not_found");

    // The top level one is allowed, and `null` puts it back.
    board
        .update(json!({ "task": task.number, "parent": grandparent.number }))
        .await
        .expect("a top-level task may become a parent");
    assert_eq!(
        board.read(task.id).await.parent_id,
        Some(grandparent.id),
        "the re-parenting landed",
    );

    board
        .update(json!({ "task": task.number, "parent": null }))
        .await
        .expect("null detaches");
    assert_eq!(board.read(task.id).await.parent_id, None);
}

#[tokio::test]
async fn a_dependency_that_would_close_a_cycle_is_invalid_argument_here() {
    let board = Board::create().await;
    let first = board.task("First", "ready").await;
    let second = board.task("Second", "ready").await;
    board
        .edge(second.id, first.id, TaskDependencyKind::Blocks)
        .await;
    board.hold(first.id, board.caller).await;

    let before = board.read(first.id).await;
    let err = board
        .update(json!({ "task": first.number, "add_depends_on": [second.number] }))
        .await
        .expect_err("the reciprocal edge would close a cycle");

    // 409 over HTTP, `invalid_argument` here (`SPEC.md`, `update`).
    assert_eq!(code(&err), "invalid_argument");
    assert_eq!(err.message, "dependency would create a cycle");
    assert_eq!(board.read(first.id).await, before);

    // A self-edge is the model's own rejection, not the cycle walk's.
    let err = board
        .update(json!({ "task": first.number, "add_depends_on": [first.number] }))
        .await
        .expect_err("a task cannot depend on itself");
    assert_eq!(code(&err), "invalid_argument");
    assert_eq!(err.message, "a task cannot depend on itself");
}

#[tokio::test]
async fn adding_a_prerequisite_takes_a_hash_number_and_blocks_the_task() {
    let board = Board::create().await;
    let prerequisite = board.task("Do this first", "ready").await;
    let task = board.task("Then this", "ready").await;
    board.hold(task.id, board.caller).await;

    let output = board
        .update(json!({
            "task": task.number,
            "add_depends_on": [format!("#{}", prerequisite.number)],
        }))
        .await
        .expect("an agent may write a task number with a hash");

    let returned = task_of(&output);
    assert_eq!(returned.depends_on.len(), 1);
    assert_eq!(returned.depends_on[0].task_id, prerequisite.id);
    assert!(returned.blocked);
    assert_eq!(
        board.event_kinds().await,
        vec![TaskEventKind::DependencyAdded, TaskEventKind::Blocked],
    );
}

#[tokio::test]
async fn removing_a_prerequisite_takes_the_blocks_edge_and_leaves_the_provenance() {
    let board = Board::create().await;
    let origin = board.task("Where it came from", "ready").await;
    let task = board.task("What came of it", "ready").await;
    board
        .edge(task.id, origin.id, TaskDependencyKind::Blocks)
        .await;
    board
        .edge(task.id, origin.id, TaskDependencyKind::DiscoveredFrom)
        .await;
    let arranged = board.events().await.len();
    board.hold(task.id, board.caller).await;

    let output = board
        .update(json!({ "task": task.number, "remove_depends_on": [origin.number] }))
        .await
        .expect("the blocker may go");

    let kinds: Vec<_> = task_of(&output)
        .depends_on
        .into_iter()
        .map(|edge| edge.kind)
        .collect();
    assert_eq!(kinds, vec![TaskDependencyKind::DiscoveredFrom]);
    assert_eq!(
        board.event_kinds_after(arranged).await,
        vec![TaskEventKind::DependencyRemoved, TaskEventKind::Unblocked],
    );

    // An edge that is not there is the REST message, so the agent can tell it
    // apart from a task it named wrong (`SPEC.md`, `update`).
    let err = board
        .update(json!({ "task": task.number, "remove_depends_on": [origin.number] }))
        .await
        .expect_err("the blocker is already gone");
    assert_eq!(code(&err), "not_found");
    assert_eq!(err.message, "dependency not found");
}

#[tokio::test]
async fn an_update_with_no_effective_change_writes_nothing() {
    let board = Board::create().await;
    let task = board.task("Wire the tracker", "ready").await;
    board.hold(task.id, board.caller).await;
    let before = board.read(task.id).await;

    let output = board
        .update(json!({
            "task": task.number,
            "title": before.title,
            "priority": before.priority,
            "labels": before.labels,
            "state": "ready",
        }))
        .await
        .expect("a no-op is not a refusal");

    assert_eq!(task_of(&output).updated_at, before.updated_at);
    assert_eq!(board.read(task.id).await, before);
    assert_eq!(board.events().await.len(), 0);
    assert_eq!(board.links(task.id).await, 0);
}

#[tokio::test]
async fn clearing_the_labels_is_an_effective_change() {
    let board = Board::create().await;
    let task = board.task("Wire the tracker", "ready").await;
    board.hold(task.id, board.caller).await;
    board
        .update(json!({ "task": task.number, "labels": ["backend"] }))
        .await
        .expect("the labels are set");

    let output = board
        .update(json!({ "task": task.number, "labels": [] }))
        .await
        .expect("an empty list clears them");

    assert!(task_of(&output).labels.is_empty());
    assert_eq!(
        board.event_kinds().await,
        vec![TaskEventKind::Updated, TaskEventKind::Updated],
    );
}

#[tokio::test]
async fn an_update_that_names_no_field_and_one_with_a_bad_priority_are_refused() {
    let board = Board::create().await;
    let task = board.task("Wire the tracker", "ready").await;
    board.hold(task.id, board.caller).await;

    let err = board
        .update(json!({ "task": task.number }))
        .await
        .expect_err("an update has to ask for something");
    assert_eq!(code(&err), "invalid_argument");
    assert_eq!(err.message, "update requires at least one field");

    let err = board
        .update(json!({ "task": task.number, "priority": 7 }))
        .await
        .expect_err("the priority is 0 through 3");
    assert_eq!(code(&err), "invalid_argument");

    let err = board
        .update(json!({ "task": 9999, "title": "nothing there" }))
        .await
        .expect_err("the task has to exist");
    assert_eq!(code(&err), "not_found");
    assert_eq!(err.message, "task not found");

    assert_eq!(board.events().await.len(), 0);
}

// ---- path B: the hand-off ----

/// A ready project with real repositories, an MCP caller with a work clone,
/// and a reviewer session to forward with.
struct Handoffs {
    fixture: GitFixture,
    client: McpClient,
    caller: Uuid,
}

impl Handoffs {
    /// A caller whose profile allows exactly `mcp_tools`; publishing a
    /// revision needs none of them.
    async fn create(name: &str, mcp_tools: &[&str]) -> Handoffs {
        let fixture = GitFixture::create(name).await;
        let profile_id = fixture
            .app
            .seed_mcp_profile(fixture.project.id, mcp_tools)
            .await;
        let seeded = fixture
            .app
            .seed_mcp_session(fixture.project.id, profile_id, SessionState::Running)
            .await;
        work_clone(&fixture, seeded.session_id).await;

        let client = McpClient::connect(&fixture.app, &seeded.token)
            .await
            .expect("a running session's token authenticates");

        Handoffs {
            fixture,
            client,
            caller: seeded.session_id,
        }
    }

    /// The usual caller: a profile that names no git tool at all, because a
    /// revision publication needs no git-tool permission.
    async fn create_without_git_tools(name: &str) -> Handoffs {
        Self::create(name, &[]).await
    }

    /// A second MCP session of the project: the reviewer that forwards.
    async fn reviewer(&self) -> (McpClient, Uuid) {
        let profile_id = self
            .fixture
            .app
            .seed_mcp_profile(self.fixture.project.id, &[])
            .await;
        let seeded = self
            .fixture
            .app
            .seed_mcp_session(self.fixture.project.id, profile_id, SessionState::Running)
            .await;
        let client = McpClient::connect(&self.fixture.app, &seeded.token)
            .await
            .expect("a running session's token authenticates");

        (client, seeded.session_id)
    }

    async fn update(&self, args: Value) -> Result<Value, ErrorData> {
        self.client.call("update", args).await
    }

    /// A task of this project in `state`, held by the caller.
    async fn held_task(&self, title: &str, state: &str) -> Task {
        let task = self.fixture.task(title, state).await;

        self.fixture.claim(task.id, self.caller).await
    }

    async fn commit(&self, file: &str, content: &str) -> String {
        self.fixture
            .commit_in_work_clone(self.caller, file, content)
            .await
    }

    async fn read(&self, task_id: Uuid) -> TaskDto {
        read(&self.fixture.app, self.fixture.project.id, task_id).await
    }

    async fn event_kinds(&self) -> Vec<TaskEventKind> {
        events(&self.fixture.app, self.fixture.project.id)
            .await
            .into_iter()
            .map(|event| event.kind)
            .collect()
    }

    async fn handoff(&self, id: Uuid) -> TaskHandoff {
        TaskRepository::new(&self.fixture.app.pool)
            .find_handoff(self.fixture.project.id, id)
            .await
            .expect("the hand-off reads")
            .expect("the hand-off is in this project")
    }
}

/// A work clone made from `main`, as a launching session's would be.
async fn work_clone(fixture: &GitFixture, session_id: Uuid) {
    let guard = fixture.guard().await;
    let paths = fixture.paths();
    let base = resolve_base(&guard, &paths, None, "main")
        .await
        .expect("the base resolves");
    create_work_clone(&guard, &paths, session_id, &base, &test_identity())
        .await
        .expect("the work clone is created");
}

#[tokio::test]
async fn a_revision_pins_the_commit_and_hands_the_task_to_review() {
    let handoffs = Handoffs::create_without_git_tools("mcp-revision").await;
    let task = handoffs.held_task("Wire the tracker", "ready").await;
    let commit = handoffs.commit("NOTES.md", "the work so far").await;

    let output = handoffs
        .update(json!({
            "task": task.number,
            "state": "review",
            "handoff": {
                "kind": "revision",
                "commit": commit,
                "comment": "Implemented and tested.",
            },
        }))
        .await
        .expect("a profile with no git tools may still publish a revision");

    let returned = task_of(&output);
    assert_eq!(returned.state, "review");
    assert_eq!(returned.lease_holder_session_id, None);
    let published = returned.handoff.expect("the task carries its new hand-off");

    let row = handoffs.handoff(published.id).await;
    assert_eq!(row.source_session_id, Some(handoffs.caller));
    assert_eq!(row.created_by_session_id, Some(handoffs.caller));
    assert_eq!(row.review_status, ReviewStatus::Unreviewed);
    assert_eq!(row.commit, commit);

    assert_eq!(
        handoffs.fixture.handoff_refs().await,
        vec![(published.id, commit.clone())],
        "`refs/handoffs/<id>` resolves to the published commit",
    );
    assert_eq!(
        handoffs.read(task.id).await.handoff.map(|h| h.id),
        Some(published.id),
    );
    assert_eq!(
        handoffs.event_kinds().await,
        vec![TaskEventKind::StateChanged, TaskEventKind::Commented],
    );
}

#[tokio::test]
async fn a_revision_at_anything_but_the_branch_tip_is_a_conflict() {
    let handoffs = Handoffs::create_without_git_tools("mcp-revision-tip").await;
    let task = handoffs.held_task("Wire the tracker", "ready").await;
    let first = handoffs.commit("NOTES.md", "the first pass").await;
    handoffs.commit("NOTES.md", "the second pass").await;

    let before = handoffs.read(task.id).await;
    let err = handoffs
        .update(json!({
            "task": task.number,
            "state": "review",
            "handoff": {
                "kind": "revision",
                "commit": first,
                "comment": "Implemented and tested.",
            },
        }))
        .await
        .expect_err("only the tip of the session branch may be published");

    assert_eq!(code(&err), "conflict");
    assert!(
        err.message.contains("does not match commit"),
        "the service's message: {}",
        err.message,
    );
    assert_eq!(handoffs.read(task.id).await, before);
    assert!(handoffs.fixture.handoff_refs().await.is_empty());
    assert_eq!(handoffs.event_kinds().await, Vec::<TaskEventKind>::new());
}

#[tokio::test]
async fn a_revision_that_names_its_own_source_session_is_refused_before_any_git_work() {
    let handoffs = Handoffs::create_without_git_tools("mcp-revision-source").await;
    let task = handoffs.held_task("Wire the tracker", "ready").await;
    let commit = handoffs.commit("NOTES.md", "the work so far").await;

    let err = handoffs
        .update(json!({
            "task": task.number,
            "state": "review",
            "handoff": {
                "kind": "revision",
                "source_session_id": handoffs.caller,
                "commit": commit,
                "comment": "Implemented and tested.",
            },
        }))
        .await
        .expect_err("an MCP caller never names a source session, not even its own");

    assert_eq!(code(&err), "invalid_argument");
    assert_eq!(
        err.message,
        "source_session_id is derived from the calling session",
    );
    // Nothing was synced: the caller's branch is still unknown to the mirror.
    assert!(handoffs.fixture.handoff_refs().await.is_empty());
    assert_eq!(handoffs.event_kinds().await, Vec::<TaskEventKind>::new());
}

#[tokio::test]
async fn a_forward_reviews_the_same_commit_and_keeps_the_original_source() {
    let handoffs = Handoffs::create_without_git_tools("mcp-forward").await;
    let task = handoffs.held_task("Wire the tracker", "ready").await;
    let commit = handoffs.commit("NOTES.md", "the work so far").await;

    let published = task_of(
        &handoffs
            .update(json!({
                "task": task.number,
                "state": "review",
                "handoff": {
                    "kind": "revision",
                    "commit": commit,
                    "comment": "Implemented and tested.",
                },
            }))
            .await
            .expect("the revision publishes"),
    )
    .handoff
    .expect("the task carries its new hand-off");

    let (reviewer_client, reviewer) = handoffs.reviewer().await;
    handoffs.fixture.claim(task.id, reviewer).await;

    let output = reviewer_client
        .call(
            "update",
            json!({
                "task": task.number,
                "state": "merge",
                "handoff": {
                    "kind": "forward",
                    "handoff_id": published.id,
                    "comment": "Looks right.",
                    "review": "approved",
                },
            }),
        )
        .await
        .expect("the reviewer forwards the hand-off");

    let forwarded = task_of(&output)
        .handoff
        .expect("the task carries the forwarded hand-off");
    assert_ne!(forwarded.id, published.id, "a forward is a new record");

    let row = handoffs.handoff(forwarded.id).await;
    assert_eq!(
        row.commit, commit,
        "the reviewer never substitutes a branch"
    );
    assert_eq!(
        row.source_session_id,
        Some(handoffs.caller),
        "the source is still the session that wrote the code",
    );
    assert_eq!(row.review_status, ReviewStatus::Approved);
    assert_eq!(row.reviewed_by_session_id, Some(reviewer));
    assert_eq!(row.created_by_session_id, Some(reviewer));

    // A second forward of the record that is no longer current is stale. The
    // reviewer has to hold the task again: its own forward released the lease.
    handoffs.fixture.claim(task.id, reviewer).await;
    let err = reviewer_client
        .call(
            "update",
            json!({
                "task": task.number,
                "state": "done",
                "handoff": {
                    "kind": "forward",
                    "handoff_id": published.id,
                    "comment": "Again.",
                },
            }),
        )
        .await
        .expect_err("only the current hand-off may be forwarded");
    assert_eq!(code(&err), "conflict");
    assert_eq!(err.message, "handoff_id is not the task's current hand-off");
}

#[tokio::test]
async fn a_handoff_that_does_not_move_the_task_is_refused_either_way() {
    let handoffs = Handoffs::create_without_git_tools("mcp-handoff-state").await;
    let task = handoffs.held_task("Wire the tracker", "ready").await;
    let commit = handoffs.commit("NOTES.md", "the work so far").await;

    for state in [None, Some("ready")] {
        let mut args = json!({
            "task": task.number,
            "handoff": {
                "kind": "revision",
                "commit": commit,
                "comment": "Implemented and tested.",
            },
        });
        if let Some(state) = state {
            args["state"] = json!(state);
        }

        let err = handoffs
            .update(args)
            .await
            .expect_err("a hand-off travels with a move to a different state");

        assert_eq!(code(&err), "invalid_argument", "{state:?}");
        assert_eq!(
            err.message, "handoff requires a different target state",
            "{state:?}",
        );
    }

    assert!(handoffs.fixture.handoff_refs().await.is_empty());
    assert_eq!(handoffs.event_kinds().await, Vec::<TaskEventKind>::new());
}

#[tokio::test]
async fn a_session_that_does_not_hold_the_task_cannot_publish() {
    let handoffs = Handoffs::create_without_git_tools("mcp-handoff-holder").await;
    let task = handoffs.fixture.task("Somebody else's", "ready").await;
    let commit = handoffs.commit("NOTES.md", "the work so far").await;

    let before = handoffs.read(task.id).await;
    let err = handoffs
        .update(json!({
            "task": task.number,
            "state": "review",
            "handoff": {
                "kind": "revision",
                "commit": commit,
                "comment": "Implemented and tested.",
            },
        }))
        .await
        .expect_err("an agent hands off only work it holds");

    assert_eq!(code(&err), "conflict");
    // The publication protocol's own wording (`SPEC.md`, "Code hand-offs and
    // review"), which is not the tracker's `task is not held by this session`.
    assert_eq!(err.message, "task is not held by the calling session");
    assert_eq!(handoffs.read(task.id).await, before);
    assert!(handoffs.fixture.handoff_refs().await.is_empty());
}
