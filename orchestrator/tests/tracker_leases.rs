//! `tracker::leases` against a real Postgres (`CLAUDE.md`, "Testing
//! expectations").
//!
//! The lease is the worker, so what is asserted here is the contract the MCP
//! `claim` and `ready` tools, the session launcher and the release endpoint
//! are all allowed to assume (`ARCHITECTURE.md`, "Task tracker" → "The lease
//! is the worker", "Launching a session for a task" and "Attempts and
//! escalation"; `SPEC.md`, "MCP tool contracts"; `docs/data-model.md`,
//! `tasks`):
//!
//! - a claim in a served state takes the lease, raises `attempts`, writes one
//!   `claimed` event with actor `session` and links the session to the task;
//! - the profile's served states are a policy, not a kind check: a task
//!   outside them is refused with its own message, and the human state is
//!   never served — but a launch claims it happily;
//! - blocked, held and terminal tasks all fail the same statement and all
//!   answer `task is not claimable`;
//! - **two concurrent claims leave exactly one winner**, which is the whole
//!   point of the atomic statement and of the project lock around it;
//! - `ready` is a read: ordered by priority then number, excerpted,
//!   range-checked, and it writes neither an event nor a session link;
//! - a user's release clears the lease and touches nothing else.
//!
//! Driven through `TrackerMutation` directly, with sessions inserted through
//! `SessionRepository`: the rules are the tracker's, and the route and the
//! tools that will call them are other tasks'. The release *endpoint* is
//! asserted in `tests/tasks_api.rs`.
//!
//! Needs a container engine; see `tests/common/db.rs`.

mod common;

use std::time::Duration;

use mars_orchestrator::events::{TaskActor, TaskEvent, TaskEventKind};
use mars_orchestrator::models::{
    NewSession, NewTask, Priority, ProfileKind, Task, TaskDependencyKind,
};
use mars_orchestrator::prelude::*;
use mars_orchestrator::repositories::{SessionRepository, TaskRepository};
use mars_orchestrator::tracker::graph::recompute_blocked;
use mars_orchestrator::tracker::leases::{
    READY_DEFAULT_LIMIT, claim_for_launch, claim_for_profile, ready_summaries, release_by_user,
};
use mars_orchestrator::tracker::{TaskDto, TaskSummary, TrackerMutation};
use uuid::Uuid;

/// Not a credential: an obviously fake stand-in for the Argon2id PHC string
/// the seeded user would carry (`CLAUDE.md`, rule 3).
const FAKE_PASSWORD_HASH: &str = "$argon2id$fake$hash";

/// Not a real image: the stub the session tests replay a fixture transcript
/// with.
const TEST_IMAGE: &str = "localhost/mars-session-stub:test";

/// A claim that lost, whatever it lost to (`SPEC.md`, `claim`).
const NOT_CLAIMABLE: &str = "task is not claimable";
/// A claim from outside the profile's served states.
const NOT_SERVED: &str = "task is not in a state this profile serves";

/// How long a raced pair of claims is given before the test fails rather than
/// hangs.
const RACE_TIMEOUT: Duration = Duration::from_secs(10);

/// A project with the documented default states, a profile to hang sessions
/// off, and the user a release is attributed to.
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

/// A task of this project in the given state.
async fn task(pool: &PgPool, project_id: Uuid, title: &str, state: &str) -> Task {
    task_with(pool, project_id, title, state, "", 2).await
}

/// The same, with the description and priority a `ready` listing shows.
async fn task_with(
    pool: &PgPool,
    project_id: Uuid,
    title: &str,
    state: &str,
    description: &str,
    priority: i16,
) -> Task {
    let mut new = NewTask::new(project_id, title).expect("the title parses");
    new.state_id = Some(state_id(pool, project_id, state).await);
    new.description = description.to_string();
    new.priority = Priority::try_from(priority).expect("a priority in range");

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

/// The id of a project's state by name.
async fn state_id(pool: &PgPool, project_id: Uuid, name: &str) -> Uuid {
    TaskRepository::new(pool)
        .find_state_by_name(project_id, name)
        .await
        .expect("the state reads")
        .expect("the project has this state")
        .id
}

/// The ids of several of them, which is what a served-state policy is.
async fn state_ids(pool: &PgPool, project_id: Uuid, names: &[&str]) -> Vec<Uuid> {
    let mut ids = Vec::with_capacity(names.len());
    for name in names {
        ids.push(state_id(pool, project_id, name).await);
    }
    ids
}

/// Claim as an agent would: under the project lock, on the row as it is there,
/// committing only when the claim succeeded.
///
/// Each of the three helpers below opens and finishes its own mutation, which
/// is what every real caller does — and what makes the raced pair a race over
/// the project lock rather than two operations sharing one transaction.
async fn claim_profile(
    pool: &PgPool,
    project_id: Uuid,
    task_id: Uuid,
    session_id: Uuid,
    served: &[Uuid],
) -> Result<TaskDto> {
    let mut mutation = TrackerMutation::begin(pool, project_id, TaskActor::Session { session_id })
        .await
        .expect("the mutation opens");

    let claimed = match locked_task(&mut mutation, project_id, task_id).await {
        Ok(task) => claim_for_profile(&mut mutation, &task, session_id, served).await,
        Err(error) => Err(error),
    };

    finish(mutation, claimed).await
}

/// Claim as the session launcher would, ignoring the served states.
async fn claim_launch(
    pool: &PgPool,
    project_id: Uuid,
    task_id: Uuid,
    session_id: Uuid,
    user_id: Uuid,
) -> Result<TaskDto> {
    let mut mutation = TrackerMutation::begin(pool, project_id, TaskActor::User { user_id })
        .await
        .expect("the mutation opens");

    let claimed = match locked_task(&mut mutation, project_id, task_id).await {
        Ok(task) => claim_for_launch(&mut mutation, &task, session_id).await,
        Err(error) => Err(error),
    };

    finish(mutation, claimed).await
}

/// Release as the endpoint would.
async fn release(pool: &PgPool, project_id: Uuid, task_id: Uuid, user_id: Uuid) -> Result<TaskDto> {
    let mut mutation = TrackerMutation::begin(pool, project_id, TaskActor::User { user_id })
        .await
        .expect("the mutation opens");

    let released = match locked_task(&mut mutation, project_id, task_id).await {
        Ok(task) => release_by_user(&mut mutation, &task).await,
        Err(error) => Err(error),
    };

    finish(mutation, released).await
}

/// The task row under the mutation's lock, or `NotFound`.
async fn locked_task(m: &mut TrackerMutation<'_>, project_id: Uuid, task_id: Uuid) -> Result<Task> {
    let pool = m.pool();
    TaskRepository::new(pool)
        .find_task_for_update(m.conn(), project_id, task_id.into())
        .await?
        .ok_or(Error::NotFound)
}

/// Commit exactly when the operation succeeded, roll back otherwise.
async fn finish(mutation: TrackerMutation<'_>, outcome: Result<TaskDto>) -> Result<TaskDto> {
    match outcome {
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

/// Put a lease on a task without going through a claim, so that "already
/// held" is a precondition rather than a second assertion.
async fn hold(pool: &PgPool, project_id: Uuid, task_id: Uuid, session_id: Uuid) {
    common::tracker::hold(pool, project_id, task_id, session_id).await;
}

/// Add one dependency edge of this kind and store the flag it implies.
async fn depends_on(
    pool: &PgPool,
    project_id: Uuid,
    dependant: Uuid,
    prerequisite: Uuid,
    kind: TaskDependencyKind,
) {
    let mut mutation = TrackerMutation::begin(pool, project_id, TaskActor::System)
        .await
        .expect("the mutation opens");
    TaskRepository::new(pool)
        .insert_dependency(mutation.conn(), project_id, dependant, prerequisite, kind)
        .await
        .expect("the edge inserts");
    recompute_blocked(&mut mutation, &[dependant])
        .await
        .expect("the flag is recomputed");
    mutation.commit().await.expect("the mutation commits");
}

/// The task as it is committed.
async fn read(pool: &PgPool, project_id: Uuid, task_id: Uuid) -> TaskDto {
    TaskRepository::new(pool)
        .load_task_dto(project_id, task_id)
        .await
        .expect("the task reads")
        .expect("the task is in this project")
}

/// Every committed event of the project, oldest first.
async fn events(pool: &PgPool, project_id: Uuid) -> Vec<TaskEvent> {
    TaskRepository::new(pool)
        .list_task_events_after(project_id, 0, 100)
        .await
        .expect("the events read")
        .into_iter()
        .map(|row| TaskEvent::from_row(row).expect("the row is a documented event"))
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

/// How many sessions are linked to this task.
async fn link_count(pool: &PgPool, task_id: Uuid) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM task_sessions WHERE task_id = $1")
        .bind(task_id)
        .fetch_one(pool)
        .await
        .expect("the count runs")
}

/// The conflict message, or a failure naming what came back instead.
fn conflict(error: Error) -> String {
    match error {
        Error::Conflict(message) => message,
        other => panic!("expected a conflict, got {other:?}"),
    }
}

/// The numbers of a summary list, in the order it came back.
fn numbers(summaries: &[TaskSummary]) -> Vec<i32> {
    summaries.iter().map(|summary| summary.number).collect()
}

#[tokio::test]
async fn a_claim_in_a_served_state_takes_the_lease_and_counts_the_attempt() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let session_id = seed_session(&pool, project_id, fixture.profile_id).await;
    let served = state_ids(&pool, project_id, &["ready"]).await;

    let subject = task(&pool, project_id, "implement it", "ready").await;
    let claimed = claim_profile(&pool, project_id, subject.id, session_id, &served)
        .await
        .expect("the claim succeeds");

    assert_eq!(claimed.lease_holder_session_id, Some(session_id));
    assert!(claimed.lease_since.is_some());
    assert_eq!(claimed.attempts, 1);
    assert_eq!(claimed.state, "ready");

    let stored = read(&pool, project_id, subject.id).await;
    assert_eq!(stored, claimed);

    let written = events(&pool, project_id).await;
    assert_eq!(written.len(), 1, "a claim emits exactly one event");
    assert_eq!(written[0].kind, TaskEventKind::Claimed);
    assert_eq!(written[0].task_id, Some(subject.id));
    assert_eq!(written[0].actor, TaskActor::Session { session_id });
    // The payload carries the task as it is after the claim.
    assert_eq!(
        written[0]
            .task
            .as_ref()
            .and_then(|task| task.lease_holder_session_id),
        Some(session_id),
    );

    assert_eq!(link_count(&pool, subject.id).await, 1);
}

#[tokio::test]
async fn a_task_somebody_already_holds_is_not_claimable() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let holder = seed_session(&pool, project_id, fixture.profile_id).await;
    let other = seed_session(&pool, project_id, fixture.profile_id).await;
    let served = state_ids(&pool, project_id, &["ready"]).await;

    let subject = task(&pool, project_id, "implement it", "ready").await;
    claim_profile(&pool, project_id, subject.id, holder, &served)
        .await
        .expect("the first claim succeeds");

    let lost = claim_profile(&pool, project_id, subject.id, other, &served)
        .await
        .expect_err("the second claim loses");
    assert_eq!(conflict(lost), NOT_CLAIMABLE);

    // The loser changed nothing: the holder, the counter and the stream are
    // the winner's.
    let stored = read(&pool, project_id, subject.id).await;
    assert_eq!(stored.lease_holder_session_id, Some(holder));
    assert_eq!(stored.attempts, 1);
    assert_eq!(events(&pool, project_id).await.len(), 1);
    assert_eq!(link_count(&pool, subject.id).await, 1);
}

#[tokio::test]
async fn a_session_claiming_what_it_already_holds_is_told_it_is_not_claimable() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let session_id = seed_session(&pool, project_id, fixture.profile_id).await;
    let served = state_ids(&pool, project_id, &["ready"]).await;

    let subject = task(&pool, project_id, "implement it", "ready").await;
    claim_profile(&pool, project_id, subject.id, session_id, &served)
        .await
        .expect("the claim succeeds");

    let again = claim_profile(&pool, project_id, subject.id, session_id, &served)
        .await
        .expect_err("a second claim by the holder loses too");
    assert_eq!(conflict(again), NOT_CLAIMABLE);

    // In particular `attempts` did not move: the statement matched no row.
    assert_eq!(read(&pool, project_id, subject.id).await.attempts, 1);
}

#[tokio::test]
async fn a_task_outside_the_served_states_is_refused_with_its_own_message() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let session_id = seed_session(&pool, project_id, fixture.profile_id).await;
    let served = state_ids(&pool, project_id, &["ready"]).await;

    let backlogged = task(&pool, project_id, "not planned yet", "backlog").await;
    let refused = claim_profile(&pool, project_id, backlogged.id, session_id, &served)
        .await
        .expect_err("a state the profile does not serve is refused");
    assert_eq!(conflict(refused), NOT_SERVED);

    // Only queue states can be served, so a task waiting for a person is never
    // an agent's to claim either.
    let escalated = task(&pool, project_id, "needs a decision", "needs_human").await;
    let human = claim_profile(&pool, project_id, escalated.id, session_id, &served)
        .await
        .expect_err("the human state is never served");
    assert_eq!(conflict(human), NOT_SERVED);

    assert!(events(&pool, project_id).await.is_empty());
    assert_eq!(link_count(&pool, backlogged.id).await, 0);
}

#[tokio::test]
async fn a_blocked_task_is_not_claimable_even_in_a_served_state() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let session_id = seed_session(&pool, project_id, fixture.profile_id).await;
    let served = state_ids(&pool, project_id, &["ready"]).await;

    let prerequisite = task(&pool, project_id, "first this", "ready").await;
    let dependant = task(&pool, project_id, "then this", "ready").await;
    depends_on(
        &pool,
        project_id,
        dependant.id,
        prerequisite.id,
        TaskDependencyKind::Blocks,
    )
    .await;
    assert!(read(&pool, project_id, dependant.id).await.blocked);

    let refused = claim_profile(&pool, project_id, dependant.id, session_id, &served)
        .await
        .expect_err("a blocked task is not claimable");
    assert_eq!(conflict(refused), NOT_CLAIMABLE);
    assert_eq!(read(&pool, project_id, dependant.id).await.attempts, 0);
}

#[tokio::test]
async fn two_concurrent_claims_on_one_task_leave_exactly_one_winner() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let first = seed_session(&pool, project_id, fixture.profile_id).await;
    let second = seed_session(&pool, project_id, fixture.profile_id).await;
    let served = state_ids(&pool, project_id, &["ready"]).await;

    let subject = task(&pool, project_id, "contested", "ready").await;

    let (left, right) = tokio::time::timeout(RACE_TIMEOUT, async {
        tokio::join!(
            claim_profile(&pool, project_id, subject.id, first, &served),
            claim_profile(&pool, project_id, subject.id, second, &served),
        )
    })
    .await
    .expect("neither claim deadlocks");

    let winners = [&left, &right].iter().filter(|r| r.is_ok()).count();
    assert_eq!(winners, 1, "exactly one claim wins: {left:?} / {right:?}");

    let loser = [left, right]
        .into_iter()
        .find_map(|result| result.err())
        .expect("one claim lost");
    assert_eq!(conflict(loser), NOT_CLAIMABLE);

    // One winner, one attempt, one event, one link.
    let stored = read(&pool, project_id, subject.id).await;
    assert!(
        stored.lease_holder_session_id == Some(first)
            || stored.lease_holder_session_id == Some(second)
    );
    assert_eq!(stored.attempts, 1);
    assert_eq!(events(&pool, project_id).await.len(), 1);
    assert_eq!(link_count(&pool, subject.id).await, 1);
}

#[tokio::test]
async fn a_launch_claims_a_task_waiting_for_a_human() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let session_id = seed_session(&pool, project_id, fixture.profile_id).await;

    let subject = task(&pool, project_id, "needs a decision", "needs_human").await;
    let claimed = claim_launch(&pool, project_id, subject.id, session_id, fixture.user_id)
        .await
        .expect("a launch ignores the served states");

    assert_eq!(claimed.state, "needs_human");
    assert_eq!(claimed.lease_holder_session_id, Some(session_id));
    assert_eq!(claimed.attempts, 1);

    // The launching user is the actor, not the session it launched.
    let written = events(&pool, project_id).await;
    assert_eq!(written.len(), 1);
    assert_eq!(written[0].kind, TaskEventKind::Claimed);
    assert_eq!(
        written[0].actor,
        TaskActor::User {
            user_id: fixture.user_id
        }
    );
    // The session still worked on the task, so the link is the session's.
    assert_eq!(link_count(&pool, subject.id).await, 1);
}

#[tokio::test]
async fn a_launch_cannot_claim_a_finished_or_held_task() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let session_id = seed_session(&pool, project_id, fixture.profile_id).await;
    let other = seed_session(&pool, project_id, fixture.profile_id).await;

    let finished = task(&pool, project_id, "already done", "done").await;
    let terminal = claim_launch(&pool, project_id, finished.id, session_id, fixture.user_id)
        .await
        .expect_err("a terminal task is not claimable");
    assert_eq!(conflict(terminal), NOT_CLAIMABLE);

    let taken = task(&pool, project_id, "somebody else has it", "ready").await;
    hold(&pool, project_id, taken.id, other).await;
    // The arranging claim is all the stream holds so far.
    let after = since(&pool, project_id).await;
    let held = claim_launch(&pool, project_id, taken.id, session_id, fixture.user_id)
        .await
        .expect_err("a held task is not claimable");
    assert_eq!(conflict(held), NOT_CLAIMABLE);

    assert!(events_after(&pool, project_id, after).await.is_empty());
}

#[tokio::test]
async fn ready_lists_only_claimable_tasks_in_served_states_and_writes_nothing() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let session_id = seed_session(&pool, project_id, fixture.profile_id).await;
    let served = state_ids(&pool, project_id, &["ready"]).await;

    // Listed, and in this order: priority first, then number.
    let low = task_with(&pool, project_id, "low", "ready", "", 3).await;
    let critical = task_with(
        &pool,
        project_id,
        "critical",
        "ready",
        &format!("First line.\n{}", "x".repeat(250)),
        0,
    )
    .await;
    let medium = task_with(&pool, project_id, "medium", "ready", "Short.", 2).await;

    // Not listed: another state, held, blocked.
    let backlogged = task(&pool, project_id, "backlog", "backlog").await;
    let taken = task(&pool, project_id, "held", "ready").await;
    hold(&pool, project_id, taken.id, session_id).await;
    let dependant = task(&pool, project_id, "blocked", "ready").await;
    depends_on(
        &pool,
        project_id,
        dependant.id,
        backlogged.id,
        TaskDependencyKind::Blocks,
    )
    .await;

    // Two outgoing edges of two kinds on the medium task; both count.
    depends_on(
        &pool,
        project_id,
        medium.id,
        low.id,
        TaskDependencyKind::DiscoveredFrom,
    )
    .await;
    depends_on(
        &pool,
        project_id,
        medium.id,
        critical.id,
        TaskDependencyKind::Related,
    )
    .await;

    let before = events(&pool, project_id).await.len();

    let listed = ready_summaries(&pool, project_id, &served, READY_DEFAULT_LIMIT)
        .await
        .expect("the read succeeds");

    assert_eq!(
        numbers(&listed),
        vec![critical.number, medium.number, low.number],
    );

    let first = &listed[0];
    assert_eq!(first.id, critical.id);
    assert_eq!(first.state, "ready");
    assert_eq!(first.priority, 0);
    assert_eq!(first.attempts, 0);
    assert_eq!(first.depends_on_count, 0);
    // Flattened onto one line, 200 scalar values, no ellipsis.
    assert_eq!(first.description_excerpt.chars().count(), 200);
    assert!(first.description_excerpt.starts_with("First line. xxx"));
    assert!(!first.description_excerpt.contains('\n'));

    assert_eq!(listed[1].depends_on_count, 2, "edges of every kind count");
    assert_eq!(listed[2].description_excerpt, "");

    // A read is a read (ADR 0030).
    assert_eq!(events(&pool, project_id).await.len(), before);
    for id in [critical.id, medium.id, low.id] {
        assert_eq!(link_count(&pool, id).await, 0);
    }

    // A profile serving nothing gets an empty list.
    let nothing = ready_summaries(&pool, project_id, &[], READY_DEFAULT_LIMIT)
        .await
        .expect("the read succeeds");
    assert!(nothing.is_empty());
}

#[tokio::test]
async fn the_ready_limit_is_honoured_at_its_edges_and_refused_outside_them() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let served = state_ids(&pool, project_id, &["ready"]).await;

    for index in 0..3 {
        task(&pool, project_id, &format!("task {index}"), "ready").await;
    }

    let default = ready_summaries(&pool, project_id, &served, READY_DEFAULT_LIMIT)
        .await
        .expect("the default limit is accepted");
    assert_eq!(default.len(), 3);
    assert_eq!(READY_DEFAULT_LIMIT, 20);

    let one = ready_summaries(&pool, project_id, &served, 1)
        .await
        .expect("1 is accepted");
    assert_eq!(one.len(), 1);

    let hundred = ready_summaries(&pool, project_id, &served, 100)
        .await
        .expect("100 is accepted");
    assert_eq!(hundred.len(), 3);

    for refused in [0, 101, -1] {
        let error = ready_summaries(&pool, project_id, &served, refused)
            .await
            .expect_err("an out-of-range limit is refused, never clamped");
        match error {
            Error::BadRequest(message) => {
                assert_eq!(message, "limit must be an integer from 1 through 100");
            }
            other => panic!("expected a validation error, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn a_user_release_clears_the_lease_and_keeps_everything_else() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let session_id = seed_session(&pool, project_id, fixture.profile_id).await;
    let served = state_ids(&pool, project_id, &["ready"]).await;

    let subject = task(&pool, project_id, "stuck", "ready").await;
    claim_profile(&pool, project_id, subject.id, session_id, &served)
        .await
        .expect("the claim succeeds");

    let released = release(&pool, project_id, subject.id, fixture.user_id)
        .await
        .expect("the release succeeds");

    assert_eq!(released.lease_holder_session_id, None);
    assert_eq!(released.lease_since, None);
    assert_eq!(released.state, "ready");
    // Kept, so that a later agent release still escalates on the right count.
    assert_eq!(released.attempts, 1);
    assert_eq!(released.closed_at, None);

    let written = events(&pool, project_id).await;
    assert_eq!(written.len(), 2, "the claim and the release");
    assert_eq!(written[1].kind, TaskEventKind::Released);
    assert_eq!(written[1].reason.as_deref(), Some("user"));
    assert_eq!(
        written[1].actor,
        TaskActor::User {
            user_id: fixture.user_id
        }
    );
    assert_eq!(written[1].from, None);
    assert_eq!(written[1].to, None);

    // No comment, and no new session link: a user's change is not session work.
    let comments =
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM task_comments WHERE task_id = $1")
            .bind(subject.id)
            .fetch_one(&pool)
            .await
            .expect("the count runs");
    assert_eq!(comments, 0);
    assert_eq!(link_count(&pool, subject.id).await, 1);
}

#[tokio::test]
async fn a_user_releases_a_task_whose_session_has_already_ended() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let session_id = seed_session(&pool, project_id, fixture.profile_id).await;

    let subject = task(&pool, project_id, "orphaned", "ready").await;
    hold(&pool, project_id, subject.id, session_id).await;

    sqlx::query("UPDATE sessions SET state = 'done' WHERE id = $1")
        .bind(session_id)
        .execute(&pool)
        .await
        .expect("the session ends");

    let released = release(&pool, project_id, subject.id, fixture.user_id)
        .await
        .expect("a dead holder is still a holder to clear");
    assert_eq!(released.lease_holder_session_id, None);
}

#[tokio::test]
async fn releasing_a_task_nobody_holds_is_a_conflict() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;

    let subject = task(&pool, project_id, "free already", "ready").await;
    let refused = release(&pool, project_id, subject.id, fixture.user_id)
        .await
        .expect_err("there is no lease to clear");
    assert_eq!(conflict(refused), "task is not held");

    assert!(events(&pool, project_id).await.is_empty());
}
