//! The two read-only MCP tools, `ready` and `get_task` (`SPEC.md`, "MCP tool
//! contracts"; ADR 0030).
//!
//! Driven through a real `rmcp` client on a real listener, as every MCP suite
//! is, against tasks seeded through the REST API — so what is asserted is the
//! whole path an agent walks, from the bearer token to the JSON it reads.
//!
//! What is pinned here:
//!
//! - `ready` lists the claimable tasks of the profile's served states, ordered
//!   by `priority` then `number`, and excludes a blocked task, a held task, a
//!   task in a state the profile does not serve and a task of another project;
//! - its `limit` defaults to 20, accepts 1 through 100 and refuses everything
//!   else as `invalid_argument` rather than clamping it;
//! - a profile that serves no state gets an empty list;
//! - `description_excerpt` is cut at 200 *scalar values*, not bytes;
//! - `get_task` accepts a UUID, a number, `"12"` and `"#12"` alike, answers
//!   `not_found` for a task this project does not have, and returns byte for
//!   byte what the REST detail endpoint returns for the same task;
//! - **neither tool writes anything** (ADR 0030): six calls leave
//!   `task_events`, `task_sessions` and every `tasks.updated_at` exactly as
//!   they were.
//!
//! The excerpt *rule* itself is unit-tested on `tracker::dto::description_excerpt`,
//! where it is written; this suite asserts that `ready` is the thing that
//! applies it.
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use common::mcp::{McpClient, code};
use common::{AuthenticatedUser, TestApp};
use mars_orchestrator::events::TaskActor;
use mars_orchestrator::models::{
    NewSession, NewTaskComment, NewTaskHandoff, ProfileKind, SessionState, TaskRef,
};
use mars_orchestrator::projects::{NewProjectRequest, create_project};
use mars_orchestrator::repositories::tasks::test_support::{StateFields, TaskRepositoryTestExt};
use mars_orchestrator::repositories::{ProjectRepository, SessionRepository, TaskRepository};
use mars_orchestrator::tracker::{TrackerMutation, claim_for_launch};
use rmcp::model::ErrorData;
use serde_json::{Value, json};
use uuid::Uuid;

/// Not a real remote: `.invalid` can never resolve (rule 3).
const TEST_REMOTE: &str = "https://example.invalid/org/repo.git";

/// An obviously fake but well-formed SHA-1 object id (rule 3).
const HANDOFF_COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

/// The documented rejection of a `task` argument that is not one.
const TASK_ARG_MESSAGE: &str = r##"task must be a UUID, a number, or "#<number>""##;

// ---- fixture ----

/// A project with the default states, its default profile (which serves
/// `ready`), and a live session of that profile to call as.
struct Fixture {
    user: AuthenticatedUser,
    project_id: Uuid,
    session_id: Uuid,
    client: McpClient,
}

async fn seed(app: &TestApp) -> Fixture {
    let user = signed_in(app, "agent-owner").await;
    let project_id = project(app, &user, "mars").await;
    let profile_id = default_profile(app, project_id).await;

    let seeded = app
        .seed_mcp_session(project_id, profile_id, SessionState::Running)
        .await;
    let client = McpClient::connect(app, &seeded.token)
        .await
        .expect("a running session's token authenticates");

    Fixture {
        user,
        project_id,
        session_id: seeded.session_id,
        client,
    }
}

/// An obviously fake password of the length `POST /api/test/users` requires
/// (rule 3).
fn password(name: &str) -> String {
    format!("fake-password-{name}")
}

/// A signed-in ordinary user to seed tasks as.
async fn signed_in(app: &TestApp, name: &str) -> AuthenticatedUser {
    app.create_user(name, &format!("{name}@example.test"), &password(name))
        .await
}

/// A project with its seven default states and its `default` profile.
async fn project(app: &TestApp, user: &AuthenticatedUser, name: &str) -> Uuid {
    create_project(
        &app.state,
        NewProjectRequest {
            name: format!("{name}-{}", &Uuid::new_v4().simple().to_string()[..8]),
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

/// The profile `create_project` seeded, which serves `ready`
/// (`SPEC.md`, "Agent profiles").
async fn default_profile(app: &TestApp, project_id: Uuid) -> Uuid {
    ProjectRepository::new(&app.pool)
        .list_profiles(project_id)
        .await
        .expect("the profiles read")
        .first()
        .expect("a new project has its default profile")
        .id
}

// ---- seeding tasks ----

/// `POST /api/projects/{pid}/tasks` → the created task.
async fn create_task(app: &TestApp, user: &AuthenticatedUser, pid: Uuid, body: Value) -> Value {
    let response = app
        .post_as(user, &format!("/api/projects/{pid}/tasks"))
        .json(&body)
        .await;
    response.assert_status(StatusCode::CREATED);
    response.json::<Value>()
}

/// A task in `state` with this priority and description.
async fn task(
    app: &TestApp,
    user: &AuthenticatedUser,
    pid: Uuid,
    title: &str,
    state: &str,
    priority: i16,
) -> Value {
    create_task(
        app,
        user,
        pid,
        json!({ "title": title, "state": state, "priority": priority }),
    )
    .await
}

/// The `id` of a task response.
fn id(task: &Value) -> Uuid {
    task["id"]
        .as_str()
        .expect("a task carries its id")
        .parse()
        .expect("the id is a UUID")
}

/// The `number` of a task response.
fn number(task: &Value) -> i64 {
    task["number"].as_i64().expect("a task carries its number")
}

/// Put a lease on a task without claiming it, so "held" is a precondition
/// rather than a second assertion.
async fn hold(app: &TestApp, pid: Uuid, task_id: Uuid, session_id: Uuid) {
    let mut mutation = TrackerMutation::begin(&app.pool, pid, TaskActor::System)
        .await
        .expect("the mutation opens");
    TaskRepository::new(&app.pool)
        .set_task_state_fields(
            mutation.conn(),
            pid,
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

/// A second session of the project, for a lease or a link to point at.
async fn seed_session(app: &TestApp, pid: Uuid, profile_id: Uuid) -> Uuid {
    let session = NewSession::new(
        pid,
        profile_id,
        ProfileKind::Conversational,
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

// ---- calling ----

/// `tools/call`, expecting it to succeed.
async fn call(client: &McpClient, tool: &str, args: Value) -> Value {
    client
        .call(tool, args)
        .await
        .unwrap_or_else(|err| panic!("{tool} answers: {err:?}"))
}

/// `tools/call`, expecting the documented refusal.
async fn call_err(client: &McpClient, tool: &str, args: Value) -> ErrorData {
    client
        .call(tool, args)
        .await
        .expect_err("the call is refused")
}

/// The `number` of each summary `ready` answered, in the order it answered
/// them.
fn ready_numbers(output: &Value) -> Vec<i64> {
    output["tasks"]
        .as_array()
        .expect("ready answers a tasks array")
        .iter()
        .map(number)
        .collect()
}

// ---- ADR 0030: what a read must not change ----

/// Everything a read is forbidden to touch, for one project.
#[derive(Debug, PartialEq, Eq)]
struct Footprint {
    events: i64,
    links: i64,
    updated_at: Option<DateTime<Utc>>,
}

async fn footprint(app: &TestApp, pid: Uuid) -> Footprint {
    // Row-level facts no interface answers: the tools under test are precisely
    // the ones that must write none of these rows.
    let events =
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM task_events WHERE project_id = $1")
            .bind(pid)
            .fetch_one(&app.pool)
            .await
            .expect("the event count runs");

    let links = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM task_sessions AS l
         JOIN tasks AS t ON t.id = l.task_id
         WHERE t.project_id = $1",
    )
    .bind(pid)
    .fetch_one(&app.pool)
    .await
    .expect("the link count runs");

    let updated_at = sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
        "SELECT MAX(updated_at) FROM tasks WHERE project_id = $1",
    )
    .bind(pid)
    .fetch_one(&app.pool)
    .await
    .expect("the timestamp reads");

    Footprint {
        events,
        links,
        updated_at,
    }
}

// ---- ready ----

#[tokio::test]
async fn ready_orders_by_priority_then_number() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;
    let (pid, user) = (fixture.project_id, &fixture.user);

    let normal = task(&app, user, pid, "normal", "ready", 2).await;
    let critical = task(&app, user, pid, "critical", "ready", 0).await;
    let also_normal = task(&app, user, pid, "also normal", "ready", 2).await;
    let high = task(&app, user, pid, "high", "ready", 1).await;

    let output = call(&fixture.client, "ready", json!({})).await;

    assert_eq!(
        ready_numbers(&output),
        [
            number(&critical),
            number(&high),
            number(&normal),
            number(&also_normal),
        ],
    );

    let first = &output["tasks"][0];
    assert_eq!(first["title"], json!("critical"));
    assert_eq!(first["state"], json!("ready"));
    assert_eq!(first["priority"], json!(0));
    assert_eq!(first["labels"], json!([]));
    assert_eq!(first["attempts"], json!(0));
    assert_eq!(first["depends_on_count"], json!(0));
    assert_eq!(first["description_excerpt"], json!(""));
}

#[tokio::test]
async fn ready_excludes_blocked_held_unserved_and_foreign_tasks() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;
    let (pid, user) = (fixture.project_id, &fixture.user);

    let listed = task(&app, user, pid, "claimable", "ready", 2).await;

    // Blocked by an open prerequisite, and so excluded in every state.
    let prerequisite = task(&app, user, pid, "prerequisite", "backlog", 2).await;
    let blocked = create_task(
        &app,
        user,
        pid,
        json!({
            "title": "blocked",
            "state": "ready",
            "depends_on": [prerequisite["id"].as_str().unwrap()],
        }),
    )
    .await;
    assert_eq!(blocked["blocked"], json!(true));
    assert_eq!(blocked["depends_on"].as_array().unwrap().len(), 1);

    // Held by somebody else.
    let held = task(&app, user, pid, "held", "ready", 2).await;
    let profile_id = default_profile(&app, pid).await;
    let other_session = seed_session(&app, pid, profile_id).await;
    hold(&app, pid, id(&held), other_session).await;

    // In a queue state the profile does not serve.
    task(&app, user, pid, "backlogged", "backlog", 2).await;

    // A task of another project entirely, claimable there.
    let elsewhere = project(&app, user, "other").await;
    task(&app, user, elsewhere, "not ours", "ready", 0).await;

    let output = call(&fixture.client, "ready", json!({})).await;

    assert_eq!(ready_numbers(&output), [number(&listed)]);
}

#[tokio::test]
async fn ready_counts_outgoing_dependencies_of_every_kind() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;
    let (pid, user) = (fixture.project_id, &fixture.user);

    // Two prerequisites, both closed, so the dependant is countable and
    // claimable at the same time.
    let first = task(&app, user, pid, "first", "done", 2).await;
    let second = task(&app, user, pid, "second", "done", 2).await;
    create_task(
        &app,
        user,
        pid,
        json!({
            "title": "dependant",
            "state": "ready",
            "depends_on": [first["id"].as_str().unwrap(), second["id"].as_str().unwrap()],
        }),
    )
    .await;

    let output = call(&fixture.client, "ready", json!({})).await;
    let tasks = output["tasks"].as_array().unwrap();

    assert_eq!(tasks.len(), 1, "{output}");
    assert_eq!(tasks[0]["depends_on_count"], json!(2));
}

#[tokio::test]
async fn a_limit_defaults_to_twenty_is_accepted_from_one_to_a_hundred_and_is_never_clamped() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;
    let (pid, user) = (fixture.project_id, &fixture.user);

    for n in 0..21 {
        task(&app, user, pid, &format!("task {n}"), "ready", 2).await;
    }

    let default = call(&fixture.client, "ready", json!({})).await;
    assert_eq!(ready_numbers(&default).len(), 20);

    let explicit = call(&fixture.client, "ready", json!({ "limit": 2 })).await;
    assert_eq!(ready_numbers(&explicit).len(), 2);
    assert_eq!(ready_numbers(&explicit)[..], ready_numbers(&default)[..2]);

    let one = call(&fixture.client, "ready", json!({ "limit": 1 })).await;
    assert_eq!(ready_numbers(&one).len(), 1);

    let hundred = call(&fixture.client, "ready", json!({ "limit": 100 })).await;
    assert_eq!(ready_numbers(&hundred).len(), 21);

    for limit in [json!(0), json!(101), json!(1.5), json!(20.5)] {
        let err = call_err(&fixture.client, "ready", json!({ "limit": limit })).await;
        assert_eq!(code(&err), "invalid_argument", "{limit}");
    }

    // The documented message, for the two that are integers in range-check
    // terms; a fractional one is refused by the input type first.
    let err = call_err(&fixture.client, "ready", json!({ "limit": 0 })).await;
    assert!(
        err.message.contains("limit must be between 1 and 100"),
        "{err:?}",
    );
}

#[tokio::test]
async fn a_profile_that_serves_no_state_gets_an_empty_list() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;
    let (pid, user) = (fixture.project_id, &fixture.user);

    task(&app, user, pid, "claimable", "ready", 0).await;

    // A second profile with no `profile_states` rows at all.
    let serves_nothing = app.seed_mcp_profile(pid, &[]).await;
    let seeded = app
        .seed_mcp_session(pid, serves_nothing, SessionState::Running)
        .await;
    let client = McpClient::connect(&app, &seeded.token)
        .await
        .expect("the session authenticates");

    assert_eq!(
        call(&client, "ready", json!({})).await,
        json!({ "tasks": [] })
    );

    // The same project, from a profile that does serve a state.
    assert_eq!(
        ready_numbers(&call(&fixture.client, "ready", json!({})).await).len(),
        1
    );
}

#[tokio::test]
async fn an_excerpt_is_two_hundred_scalar_values_of_one_line() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;
    let (pid, user) = (fixture.project_id, &fixture.user);

    // Multi-byte throughout: a cut counted in bytes would land mid-character
    // and a cut counted in bytes' worth of characters would keep far too few.
    let description = format!("  första raden\nandra raden\r\n{}  ", "é".repeat(250));
    create_task(
        &app,
        user,
        pid,
        json!({ "title": "long", "state": "ready", "description": description }),
    )
    .await;

    let output = call(&fixture.client, "ready", json!({})).await;
    let excerpt = output["tasks"][0]["description_excerpt"]
        .as_str()
        .expect("a summary carries its excerpt")
        .to_string();

    assert_eq!(excerpt.chars().count(), 200);
    assert!(
        excerpt.starts_with("första raden andra raden é"),
        "{excerpt}"
    );
    assert!(!excerpt.contains('\n'), "{excerpt}");
    assert!(!excerpt.ends_with('…'), "{excerpt}");

    let short = create_task(
        &app,
        user,
        pid,
        json!({ "title": "short", "state": "ready", "description": "  Wire it up.  " }),
    )
    .await;
    let output = call(&fixture.client, "ready", json!({})).await;
    let summary = output["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| number(task) == number(&short))
        .expect("the short task is listed");

    assert_eq!(summary["description_excerpt"], json!("Wire it up."));
}

// ---- get_task ----

#[tokio::test]
async fn get_task_accepts_every_documented_reference_and_matches_the_rest_detail() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;
    let (pid, user) = (fixture.project_id, &fixture.user);

    let parent = task(&app, user, pid, "parent", "ready", 1).await;
    let parent_id = id(&parent);

    // A comment, a hand-off and a session link, so every arm of `TaskDetail`
    // carries something. The link goes on before the child does: an open child
    // blocks its parent, and a blocked task cannot be claimed.
    let profile_id = default_profile(&app, pid).await;
    let worker = seed_session(&app, pid, profile_id).await;
    publish_handoff(&app, pid, parent_id, worker).await;
    link_session(&app, pid, parent_id, worker, user.user.id).await;

    create_task(
        &app,
        user,
        pid,
        json!({ "title": "child", "state": "backlog", "parent_id": parent_id }),
    )
    .await;

    let rest = app
        .get_as(user, &format!("/api/projects/{pid}/tasks/{parent_id}"))
        .await;
    rest.assert_status(StatusCode::OK);
    let rest = rest.json::<Value>();

    assert!(!rest["comments"].as_array().unwrap().is_empty());
    assert!(!rest["handoffs"].as_array().unwrap().is_empty());
    assert_eq!(rest["children"].as_array().unwrap().len(), 1);
    assert_eq!(rest["sessions"].as_array().unwrap().len(), 1);

    let n = number(&parent);
    for reference in [
        json!(parent_id.to_string()),
        json!(n),
        json!(n.to_string()),
        json!(format!("#{n}")),
    ] {
        let output = call(&fixture.client, "get_task", json!({ "task": reference })).await;
        assert_eq!(output["task"], rest, "{reference}");
    }
}

#[tokio::test]
async fn get_task_answers_not_found_for_a_task_this_project_does_not_have() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;
    let (pid, user) = (fixture.project_id, &fixture.user);

    task(&app, user, pid, "ours", "ready", 2).await;

    let elsewhere = project(&app, user, "other").await;
    let foreign = task(&app, user, elsewhere, "theirs", "ready", 2).await;

    for reference in [
        json!(9_999),
        json!("#9999"),
        json!(Uuid::new_v4().to_string()),
        json!(id(&foreign).to_string()),
    ] {
        let err = call_err(&fixture.client, "get_task", json!({ "task": reference })).await;
        assert_eq!(code(&err), "not_found", "{reference}");
        assert!(err.message.contains("task not found"), "{err:?}");
    }
}

#[tokio::test]
async fn get_task_refuses_a_reference_that_is_not_one() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;

    for reference in [
        json!("#abc"),
        json!("abc"),
        json!(""),
        json!(-1),
        json!(1.5),
    ] {
        let err = call_err(&fixture.client, "get_task", json!({ "task": reference })).await;
        assert_eq!(code(&err), "invalid_argument", "{reference}");
        assert!(err.message.contains(TASK_ARG_MESSAGE), "{err:?}");
    }
}

// ---- ADR 0030 ----

#[tokio::test]
async fn neither_read_writes_an_event_a_link_or_a_touch_timestamp() {
    let app = TestApp::spawn().await;
    let fixture = seed(&app).await;
    let (pid, user) = (fixture.project_id, &fixture.user);

    let listed = task(&app, user, pid, "claimable", "ready", 0).await;
    let held = task(&app, user, pid, "held", "ready", 1).await;
    // A claim is what a link and a touch look like when something *does*
    // write them, so the baseline is not trivially zero.
    claim(&app, pid, id(&held), fixture.session_id, user.user.id).await;

    let before = footprint(&app, pid).await;
    assert!(before.events > 0);
    assert_eq!(before.links, 1);

    for _ in 0..3 {
        call(&fixture.client, "ready", json!({})).await;
    }
    for reference in [
        json!(number(&listed)),
        json!(number(&held)),
        json!(id(&listed).to_string()),
    ] {
        call(&fixture.client, "get_task", json!({ "task": reference })).await;
    }

    assert_eq!(footprint(&app, pid).await, before);
}

// ---- seeding helpers that need the tracker ----

/// Publish one hand-off record with its comment, in one mutation — the shape
/// the hand-off path itself writes.
async fn publish_handoff(app: &TestApp, pid: Uuid, task_id: Uuid, session_id: Uuid) {
    let repository = TaskRepository::new(&app.pool);
    let mut mutation = TrackerMutation::begin(&app.pool, pid, TaskActor::System)
        .await
        .expect("the mutation opens");

    let comment = NewTaskComment::from_session(task_id, session_id, "handed over".to_string());
    repository
        .insert_comment(mutation.conn(), pid, &comment)
        .await
        .expect("the comment inserts");

    let mut handoff = NewTaskHandoff::new(task_id, "work", HANDOFF_COMMIT, comment.id);
    handoff.source_session_id = Some(session_id);
    handoff.created_by_session_id = Some(session_id);
    repository
        .insert_handoff(mutation.conn(), pid, &handoff)
        .await
        .expect("the hand-off inserts");

    mutation.commit().await.expect("the mutation commits");
}

/// Link a session to a task the way a claim does, without taking the lease:
/// the launch claim is used and the lease cleared again, so the link and the
/// touch are the real ones.
async fn link_session(app: &TestApp, pid: Uuid, task_id: Uuid, session_id: Uuid, user_id: Uuid) {
    claim(app, pid, task_id, session_id, user_id).await;

    let mut mutation = TrackerMutation::begin(&app.pool, pid, TaskActor::System)
        .await
        .expect("the mutation opens");
    TaskRepository::new(&app.pool)
        .set_task_state_fields(
            mutation.conn(),
            pid,
            task_id,
            &StateFields {
                lease: Some(None),
                ..StateFields::default()
            },
        )
        .await
        .expect("the lease clears");
    mutation.commit().await.expect("the mutation commits");
}

/// Claim as the session launcher does: lease, `attempts`, event and link.
async fn claim(app: &TestApp, pid: Uuid, task_id: Uuid, session_id: Uuid, user_id: Uuid) {
    let mut mutation = TrackerMutation::begin(&app.pool, pid, TaskActor::User { user_id })
        .await
        .expect("the mutation opens");

    let task = TaskRepository::new(&app.pool)
        .find_task_for_update(mutation.conn(), pid, TaskRef::Id(task_id))
        .await
        .expect("the row reads")
        .expect("the task is this project's");

    claim_for_launch(&mut mutation, &task, session_id)
        .await
        .expect("the claim succeeds");
    mutation.commit().await.expect("the mutation commits");
}
