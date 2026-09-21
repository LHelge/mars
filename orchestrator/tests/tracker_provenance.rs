//! Discovery provenance against a real Postgres (`CLAUDE.md`, "Testing
//! expectations").
//!
//! What a session's `create_task` records about where the new task came from
//! (`ARCHITECTURE.md`, "Task tracker" → "Discovery provenance"; `SPEC.md`,
//! "MCP tool contracts" → `create_task`; `docs/data-model.md`,
//! `task_dependencies`; ADR 0023):
//!
//! - holding exactly one task infers the origin, and the new task carries a
//!   `discovered_from` edge to it with one `dependency_added` event;
//! - holding several refuses the creation unless the origin is named, and the
//!   refusal leaves no task row behind — provenance is validated under the
//!   project lock, before the insert;
//! - a named origin must be a task of this project the caller currently
//!   holds: an unknown one is 404, one it does not hold is 400;
//! - holding nothing records nothing, which is how a scheduled agent files
//!   work;
//! - the parent link is provenance already, so an origin that is the new
//!   task's parent adds no second edge;
//! - and a `blocks` edge to the same origin coexists with the provenance one,
//!   because they say different things about the same pair.
//!
//! Driven through `TrackerMutation` and `tracker::create_task` directly: the
//! rules are the tracker's, and the MCP tool that will call them is another
//! task's.
//!
//! Needs a container engine; see `tests/common/db.rs`.

mod common;

use mars_orchestrator::events::{TaskActor, TaskEvent, TaskEventKind};
use mars_orchestrator::models::{
    NewSession, NewTask, ProfileKind, Task, TaskDependencyKind, TaskRef,
};
use mars_orchestrator::prelude::*;
use mars_orchestrator::repositories::{SessionRepository, TaskRepository};
use mars_orchestrator::tracker::provenance::{AMBIGUOUS_ORIGIN, ORIGIN_NOT_HELD};
use mars_orchestrator::tracker::{
    CreateTaskInput, CreatedBy, TaskDto, TrackerMutation, create_task,
};
use uuid::Uuid;

/// Not a credential: an obviously fake stand-in for the Argon2id PHC string
/// the seeded user would carry (`CLAUDE.md`, rule 3).
const FAKE_PASSWORD_HASH: &str = "$argon2id$fake$hash";

/// Not a real image: the stub the session tests replay a fixture transcript
/// with.
const TEST_IMAGE: &str = "localhost/mars-session-stub:test";

/// A project with the documented default states, a profile to hang sessions
/// off, and the user a REST creation is attributed to.
struct Fixture {
    user_id: Uuid,
    project_id: Uuid,
    profile_id: Uuid,
}

async fn seed(pool: &PgPool) -> Fixture {
    let user_id = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, username, email, password_hash) VALUES ($1, $2, $3, $4)")
        .bind(user_id)
        .bind(format!("user-{}", &user_id.simple().to_string()[..8]))
        .bind(format!("{user_id}@example.test"))
        .bind(FAKE_PASSWORD_HASH)
        .execute(pool)
        .await
        .expect("the user seeds");

    let project_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id, name, remote_url) VALUES ($1, $2, $3)")
        .bind(project_id)
        .bind(format!("project-{project_id}"))
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
        user_id,
        project_id,
        profile_id,
    }
}

/// A session, so a lease has something to point at.
async fn seed_session(pool: &PgPool, project_id: Uuid, profile_id: Uuid) -> Uuid {
    let session = NewSession::new(
        project_id,
        profile_id,
        ProfileKind::Conversational,
        "main",
        // Not a credential: a fake stand-in for the hashed MCP token (rule 3).
        format!("fake-mcp-token-hash-{}", Uuid::new_v4()),
    );

    let mut tx = pool.begin().await.expect("a transaction begins");
    let inserted = SessionRepository::new(pool)
        .insert(&mut tx, &session)
        .await
        .expect("the session inserts");
    tx.commit().await.expect("the transaction commits");

    inserted.id
}

/// A task of this project, inserted and committed.
async fn task(pool: &PgPool, project_id: Uuid, title: &str) -> Task {
    let new = NewTask::new(project_id, title).expect("the title parses");

    let mut mutation = TrackerMutation::begin(pool, project_id, TaskActor::System)
        .await
        .expect("the mutation opens");
    let inserted = TaskRepository::new(pool)
        .insert_task(mutation.conn(), project_id, &new)
        .await
        .expect("the task inserts");
    mutation.commit().await.expect("the mutation commits");

    inserted
}

/// Put a lease on a task, so that "the session holds it" is a precondition
/// rather than a second assertion.
async fn hold(pool: &PgPool, project_id: Uuid, task_id: Uuid, session_id: Uuid) {
    common::tracker::hold(pool, project_id, task_id, session_id).await;
}

/// The input an MCP `create_task` maps onto, with only the fields a provenance
/// test varies.
fn input(
    title: &str,
    session_id: Uuid,
    discovered_from: Option<TaskRef>,
    parent: Option<Uuid>,
    depends_on: Vec<TaskRef>,
) -> CreateTaskInput {
    CreateTaskInput {
        title: title.to_string(),
        description: None,
        state: None,
        priority: None,
        labels: Vec::new(),
        parent,
        depends_on,
        discovered_from,
        created_by: CreatedBy::Session(session_id),
    }
}

/// Create as an agent would: its own mutation, committed only when the
/// creation succeeded, so a refusal leaves nothing behind.
async fn create_as_session(
    pool: &PgPool,
    project_id: Uuid,
    session_id: Uuid,
    input: CreateTaskInput,
) -> Result<TaskDto> {
    let mut mutation = TrackerMutation::begin(pool, project_id, TaskActor::Session { session_id })
        .await
        .expect("the mutation opens");

    match create_task(&mut mutation, input).await {
        Ok(dto) => {
            mutation.commit().await.expect("the mutation commits");
            Ok(dto)
        }
        Err(error) => {
            mutation.no_change().await.expect("the mutation rolls back");
            Err(error)
        }
    }
}

/// This task's outgoing edges, in the order the repository lists them.
async fn edges(pool: &PgPool, project_id: Uuid, task_id: Uuid) -> Vec<(Uuid, TaskDependencyKind)> {
    TaskRepository::new(pool)
        .list_dependencies(project_id, task_id)
        .await
        .expect("the edges read")
        .into_iter()
        .map(|edge| (edge.depends_on_task_id, edge.kind))
        .collect()
}

/// Where the stream stands right now.
///
/// The cursor an assertion about "what this call emitted" starts from, so that
/// the claims an arrangement really makes — a lease is a claim's, and only a
/// claim raises `attempts` — are behind it rather than in it.
async fn since(pool: &PgPool, project_id: Uuid) -> i64 {
    TaskRepository::new(pool)
        .max_task_event_seq(project_id)
        .await
        .expect("the cursor reads")
}

/// The events written after `after`, oldest first.
async fn events_after(pool: &PgPool, project_id: Uuid, after: i64) -> Vec<TaskEvent> {
    TaskRepository::new(pool)
        .list_task_events_after(project_id, after, 100)
        .await
        .expect("the events read")
        .into_iter()
        .map(|row| TaskEvent::from_row(row).expect("the row is a documented event"))
        .collect()
}

/// How many tasks this project has, which is how "nothing was created" is
/// asserted.
async fn task_count(pool: &PgPool, project_id: Uuid) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM tasks WHERE project_id = $1")
        .bind(project_id)
        .fetch_one(pool)
        .await
        .expect("the count runs")
}

/// The stored row, for the columns the DTO deliberately leaves behind.
async fn read(pool: &PgPool, project_id: Uuid, task_id: Uuid) -> Task {
    TaskRepository::new(pool)
        .find_task(project_id, TaskRef::Id(task_id))
        .await
        .expect("the task reads")
        .expect("the task is there")
}

/// The sessions linked to this task.
async fn linked_sessions(pool: &PgPool, task_id: Uuid) -> Vec<Uuid> {
    TaskRepository::new(pool)
        .list_task_sessions(task_id)
        .await
        .expect("the links read")
        .into_iter()
        .map(|link| link.session_id)
        .collect()
}

/// The bad-request message, or a failure naming what came back instead.
fn bad_request(error: Error) -> String {
    match error {
        Error::BadRequest(message) => message,
        other => panic!("expected a bad request, got {other:?}"),
    }
}

#[tokio::test]
async fn holding_one_task_infers_the_origin_and_records_the_edge() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let session_id = seed_session(&pool, project_id, fixture.profile_id).await;

    let origin = task(&pool, project_id, "the task being worked").await;
    hold(&pool, project_id, origin.id, session_id).await;
    // The arranging claim is behind the cursor; what follows is the
    // creation's own.
    let after = since(&pool, project_id).await;

    let created = create_as_session(
        &pool,
        project_id,
        session_id,
        input(
            "a bug found along the way",
            session_id,
            None,
            None,
            Vec::new(),
        ),
    )
    .await
    .expect("the creation succeeds");

    assert_eq!(
        edges(&pool, project_id, created.id).await,
        vec![(origin.id, TaskDependencyKind::DiscoveredFrom)],
    );

    // The creation is the session's: the row says so and the link records it.
    let stored = read(&pool, project_id, created.id).await;
    assert_eq!(stored.created_by_session_id, Some(session_id));
    assert_eq!(stored.created_by_user_id, None);
    assert_eq!(linked_sessions(&pool, created.id).await, vec![session_id]);

    // The origin is not modified: no event about it, and it keeps its lease.
    let written = events_after(&pool, project_id, after).await;
    let kinds: Vec<_> = written.iter().map(|event| event.kind).collect();
    assert_eq!(
        kinds,
        vec![TaskEventKind::Created, TaskEventKind::DependencyAdded],
    );
    assert!(
        written
            .iter()
            .all(|event| event.task_id == Some(created.id)),
        "provenance says nothing about the origin task",
    );
    assert_eq!(
        written[0].actor,
        TaskActor::Session { session_id },
        "the events carry the creating session",
    );
    assert_eq!(
        read(&pool, project_id, origin.id)
            .await
            .lease_holder_session_id,
        Some(session_id),
    );

    // The `created` payload already carries the edge.
    assert_eq!(
        written[0]
            .task
            .as_ref()
            .map(|task| task.depends_on.len())
            .unwrap_or_default(),
        1,
    );
}

#[tokio::test]
async fn holding_several_tasks_without_an_origin_creates_nothing() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let session_id = seed_session(&pool, project_id, fixture.profile_id).await;

    let first = task(&pool, project_id, "one").await;
    let second = task(&pool, project_id, "two").await;
    hold(&pool, project_id, first.id, session_id).await;
    hold(&pool, project_id, second.id, session_id).await;
    // The arranging claims are behind the cursor; what follows is the
    // creation's own.
    let after = since(&pool, project_id).await;

    let before = task_count(&pool, project_id).await;
    let error = create_as_session(
        &pool,
        project_id,
        session_id,
        input("ambiguous", session_id, None, None, Vec::new()),
    )
    .await
    .expect_err("the creation is refused");

    assert_eq!(bad_request(error), AMBIGUOUS_ORIGIN);
    assert_eq!(
        task_count(&pool, project_id).await,
        before,
        "a refused creation inserts no task",
    );
    assert!(events_after(&pool, project_id, after).await.is_empty());
}

#[tokio::test]
async fn holding_several_tasks_with_an_explicit_origin_records_that_one() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let session_id = seed_session(&pool, project_id, fixture.profile_id).await;

    let first = task(&pool, project_id, "one").await;
    let second = task(&pool, project_id, "two").await;
    hold(&pool, project_id, first.id, session_id).await;
    hold(&pool, project_id, second.id, session_id).await;

    // Named by its per-project number, as an agent would.
    let created = create_as_session(
        &pool,
        project_id,
        session_id,
        input(
            "discovered while on the second",
            session_id,
            Some(TaskRef::Number(second.number)),
            None,
            Vec::new(),
        ),
    )
    .await
    .expect("the creation succeeds");

    assert_eq!(
        edges(&pool, project_id, created.id).await,
        vec![(second.id, TaskDependencyKind::DiscoveredFrom)],
    );
}

#[tokio::test]
async fn an_origin_the_session_does_not_hold_creates_nothing() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let session_id = seed_session(&pool, project_id, fixture.profile_id).await;

    let held = task(&pool, project_id, "the one it holds").await;
    let other = task(&pool, project_id, "somebody else's work").await;
    hold(&pool, project_id, held.id, session_id).await;

    let before = task_count(&pool, project_id).await;
    let error = create_as_session(
        &pool,
        project_id,
        session_id,
        input(
            "unfounded provenance",
            session_id,
            Some(TaskRef::Id(other.id)),
            None,
            Vec::new(),
        ),
    )
    .await
    .expect_err("the creation is refused");

    assert_eq!(bad_request(error), ORIGIN_NOT_HELD);
    assert_eq!(task_count(&pool, project_id).await, before);
}

#[tokio::test]
async fn an_unknown_origin_is_not_found() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let session_id = seed_session(&pool, project_id, fixture.profile_id).await;

    let held = task(&pool, project_id, "the one it holds").await;
    hold(&pool, project_id, held.id, session_id).await;

    let before = task_count(&pool, project_id).await;
    let error = create_as_session(
        &pool,
        project_id,
        session_id,
        input(
            "provenance from nowhere",
            session_id,
            Some(TaskRef::Number(9_999)),
            None,
            Vec::new(),
        ),
    )
    .await
    .expect_err("the creation is refused");

    assert!(
        matches!(error, Error::NotFound),
        "an origin naming nothing in this project is 404, got {error:?}",
    );
    assert_eq!(task_count(&pool, project_id).await, before);
}

#[tokio::test]
async fn an_origin_the_session_holds_may_also_be_named_explicitly() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let session_id = seed_session(&pool, project_id, fixture.profile_id).await;

    let origin = task(&pool, project_id, "the only one it holds").await;
    hold(&pool, project_id, origin.id, session_id).await;

    let created = create_as_session(
        &pool,
        project_id,
        session_id,
        input(
            "explicit and inferred agree",
            session_id,
            Some(TaskRef::Id(origin.id)),
            None,
            Vec::new(),
        ),
    )
    .await
    .expect("the creation succeeds");

    assert_eq!(
        edges(&pool, project_id, created.id).await,
        vec![(origin.id, TaskDependencyKind::DiscoveredFrom)],
    );
}

#[tokio::test]
async fn holding_nothing_records_no_provenance() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let session_id = seed_session(&pool, project_id, fixture.profile_id).await;

    let created = create_as_session(
        &pool,
        project_id,
        session_id,
        input("filed out of nothing", session_id, None, None, Vec::new()),
    )
    .await
    .expect("the creation succeeds");

    assert!(edges(&pool, project_id, created.id).await.is_empty());

    let kinds: Vec<_> = events_after(&pool, project_id, 0)
        .await
        .iter()
        .map(|event| event.kind)
        .collect();
    assert_eq!(kinds, vec![TaskEventKind::Created]);
}

#[tokio::test]
async fn an_origin_that_is_the_parent_adds_no_second_edge() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let session_id = seed_session(&pool, project_id, fixture.profile_id).await;

    let epic = task(&pool, project_id, "the plan being worked").await;
    hold(&pool, project_id, epic.id, session_id).await;
    // The arranging claims are behind the cursor; what follows is the
    // creation's own.
    let after = since(&pool, project_id).await;

    let created = create_as_session(
        &pool,
        project_id,
        session_id,
        input(
            "a step of the plan",
            session_id,
            Some(TaskRef::Id(epic.id)),
            Some(epic.id),
            Vec::new(),
        ),
    )
    .await
    .expect("the creation succeeds");

    assert_eq!(created.parent_id, Some(epic.id));
    assert!(
        edges(&pool, project_id, created.id).await.is_empty(),
        "the parent link is the provenance",
    );

    // `created`, and the flag the new open child flips on its parent — which
    // is the parent rule, not provenance (`ARCHITECTURE.md`, "Task tracker" →
    // "Parents"). No `dependency_added`, because no edge was recorded.
    let written = events_after(&pool, project_id, after).await;
    let kinds: Vec<_> = written.iter().map(|event| event.kind).collect();
    assert_eq!(
        kinds,
        vec![TaskEventKind::Created, TaskEventKind::Blocked],
        "no edge, no `dependency_added`",
    );
    assert_eq!(written[1].task_id, Some(epic.id));
}

#[tokio::test]
async fn the_inferred_origin_also_named_in_depends_on_carries_both_kinds() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let session_id = seed_session(&pool, project_id, fixture.profile_id).await;

    let origin = task(&pool, project_id, "the task being worked").await;
    hold(&pool, project_id, origin.id, session_id).await;
    // The arranging claim is behind the cursor; what follows is the
    // creation's own.
    let after = since(&pool, project_id).await;

    let created = create_as_session(
        &pool,
        project_id,
        session_id,
        input(
            "must wait for what it came from",
            session_id,
            None,
            None,
            vec![TaskRef::Id(origin.id)],
        ),
    )
    .await
    .expect("the creation succeeds");

    let mut stored = edges(&pool, project_id, created.id).await;
    stored.sort_by_key(|(_, kind)| format!("{kind:?}"));
    assert_eq!(
        stored,
        vec![
            (origin.id, TaskDependencyKind::Blocks),
            (origin.id, TaskDependencyKind::DiscoveredFrom),
        ],
    );

    // `created`, the `blocks` edge, the provenance edge, and the flag the open
    // prerequisite flips — in that order.
    let kinds: Vec<_> = events_after(&pool, project_id, after)
        .await
        .iter()
        .map(|event| event.kind)
        .collect();
    assert_eq!(
        kinds,
        vec![
            TaskEventKind::Created,
            TaskEventKind::DependencyAdded,
            TaskEventKind::DependencyAdded,
            TaskEventKind::Blocked,
        ],
    );
}

#[tokio::test]
async fn a_users_creation_has_no_provenance() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let session_id = seed_session(&pool, project_id, fixture.profile_id).await;

    // The session holds one task, so an inference would have something to
    // find; a user's creation is not a session's discovery and takes none.
    let held = task(&pool, project_id, "held by a session").await;
    hold(&pool, project_id, held.id, session_id).await;

    let user_id = fixture.user_id;
    let mut mutation = TrackerMutation::begin(&pool, project_id, TaskActor::User { user_id })
        .await
        .expect("the mutation opens");
    let created = create_task(
        &mut mutation,
        CreateTaskInput {
            created_by: CreatedBy::User(user_id),
            // Ignored for a user: REST never supplies it.
            discovered_from: Some(TaskRef::Id(held.id)),
            ..input("filed by a person", session_id, None, None, Vec::new())
        },
    )
    .await
    .expect("the creation succeeds");
    mutation.commit().await.expect("the mutation commits");

    assert!(edges(&pool, project_id, created.id).await.is_empty());

    let stored = read(&pool, project_id, created.id).await;
    assert_eq!(stored.created_by_user_id, Some(user_id));
    assert_eq!(stored.created_by_session_id, None);
    assert!(linked_sessions(&pool, created.id).await.is_empty());
}
