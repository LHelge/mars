//! `TaskRepository`'s mutation primitive, state configuration and event append
//! against a real Postgres (`CLAUDE.md`, "Testing expectations").
//!
//! Four things are being asserted here.
//!
//! The state configuration is the documented default set, the position
//! arithmetic `SPEC.md`, "Task states" promises — a missing position appends,
//! an explicit one shifts, moves and deletions re-pack — and every refusal
//! `ARCHITECTURE.md`, "Task tracker" reserves: a project keeps at least one
//! queue state, exactly one human state and at least one terminal state, and
//! no state a task is still in may be removed.
//!
//! The `profile_states` link is the role mechanism, and its two cross-table
//! rules — same project, `queue` kind — have no constraint behind them, so
//! they are asserted directly.
//!
//! The event append is the sibling of `SessionRepository::append_events` with
//! the project row in place of the session row: successive sequences, the
//! scope check on `task_id` with its one documented exception for `deleted`,
//! and one notification per committed batch and none per rolled-back one
//! (ADR 0028).
//!
//! Last, the lock itself. `docs/data-model.md`, "Tracker mutation
//! transactions" makes opening a `TrackerMutation` the serialisation point of
//! the whole tracker, so it is asserted the only way a lock can be: a second mutation is
//! opened on the same project while the first still holds it and must wait,
//! while a third on another project must not. A `FOR UPDATE` quietly dropped
//! from the statement would pass every functional assertion and fail here.
//!
//! Needs a container engine; see `tests/common/db.rs`.

mod common;

use std::time::Duration;

use axum::http::StatusCode;
use mars_orchestrator::events::TaskActor;
use mars_orchestrator::models::{
    DEFAULT_TASK_STATES, NewTaskEvent, NewTaskState, TaskStateKind, TaskStateName, task_event_kind,
};
use mars_orchestrator::prelude::*;
use mars_orchestrator::repositories::TaskRepository;
use mars_orchestrator::repositories::tasks::test_support::TaskRepositoryTestExt;
use mars_orchestrator::tracker::TrackerMutation;
use serde_json::json;
use sqlx::postgres::PgListener;
use tokio::time::{sleep, timeout};
use uuid::Uuid;

/// Not a credential: an obviously fake stand-in for the Argon2id PHC string
/// the seeded user would carry (`CLAUDE.md`, rule 3).
const FAKE_PASSWORD_HASH: &str = "$argon2id$fake$hash";

/// Not a real image: the stub the session tests replay a fixture transcript
/// with.
const TEST_IMAGE: &str = "localhost/mars-session-stub:test";

/// How long a blocked transaction is given to prove it is blocked. Long enough
/// that a slow container has certainly started the second transaction, short
/// enough not to drag the suite out.
const BLOCKED_FOR: Duration = Duration::from_millis(400);

/// How long an unblocked transaction — or an expected notification — is given
/// once nothing should be in its way.
const UNBLOCKED_WITHIN: Duration = Duration::from_secs(10);

/// The rows a project's tracker needs to exist at all.
///
/// Seeded with unchecked statements rather than through their repositories:
/// this file is about `TaskRepository`, and the foreign keys are all it needs
/// from the others.
struct Fixture {
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

    let project_id = seed_project(pool).await;
    let profile_id = seed_profile(pool, project_id, "default").await;

    Fixture {
        project_id,
        profile_id,
    }
}

async fn seed_project(pool: &PgPool) -> Uuid {
    let project_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id, name, remote_url) VALUES ($1, $2, $3)")
        .bind(project_id)
        .bind(format!("project-{project_id}"))
        // `.invalid` can never resolve (`CLAUDE.md`, rule 3).
        .bind("https://example.invalid/org/repo.git")
        .execute(pool)
        .await
        .expect("the project seeds");

    project_id
}

async fn seed_profile(pool: &PgPool, project_id: Uuid, name: &str) -> Uuid {
    let profile_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO agent_profiles (id, project_id, name, image, partial_messages)
         VALUES ($1, $2, $3, $4, TRUE)",
    )
    .bind(profile_id)
    .bind(project_id)
    .bind(name)
    .bind(TEST_IMAGE)
    .execute(pool)
    .await
    .expect("the profile seeds");

    profile_id
}

/// A bare `tasks` row, for the checks that need one to exist.
///
/// Task rows are the next task's to write; this is the minimum set of columns
/// the table demands.
async fn seed_task(pool: &PgPool, project_id: Uuid, state_id: Uuid, number: i32) -> Uuid {
    let task_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO tasks (id, project_id, number, title, state_id) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(task_id)
    .bind(project_id)
    .bind(number)
    .bind(format!("task {number}"))
    .bind(state_id)
    .execute(pool)
    .await
    .expect("the task seeds");

    task_id
}

/// Create the default state set in its own committed mutation.
async fn seed_default_states(pool: &PgPool, project_id: Uuid) {
    let repository = TaskRepository::new(pool);
    let mut mutation = TrackerMutation::begin(pool, project_id, TaskActor::System)
        .await
        .expect("the mutation opens");
    repository
        .insert_default_states(mutation.conn(), project_id)
        .await
        .expect("the default states insert");
    mutation.commit().await.expect("the mutation commits");
}

/// The project's state names in board order.
async fn names(repository: &TaskRepository<'_>, project_id: Uuid) -> Vec<String> {
    repository
        .list_states(project_id)
        .await
        .unwrap()
        .into_iter()
        .map(|state| state.name)
        .collect()
}

/// The project's positions in board order, which must always be `0..n`.
async fn positions(repository: &TaskRepository<'_>, project_id: Uuid) -> Vec<i32> {
    repository
        .list_states(project_id)
        .await
        .unwrap()
        .into_iter()
        .map(|state| state.position)
        .collect()
}

async fn state_id(repository: &TaskRepository<'_>, project_id: Uuid, name: &str) -> Uuid {
    repository
        .find_state_by_name(project_id, name)
        .await
        .unwrap()
        .unwrap_or_else(|| panic!("the project has a state named {name}"))
        .id
}

fn new_state(project_id: Uuid, name: &str, kind: TaskStateKind) -> NewTaskState {
    NewTaskState::new(project_id, name, kind).expect("the test state name is valid")
}

/// Insert `state` in its own committed mutation.
async fn insert_state(pool: &PgPool, project_id: Uuid, state: &NewTaskState) -> Result<Uuid> {
    let repository = TaskRepository::new(pool);
    let mut mutation = TrackerMutation::begin(pool, project_id, TaskActor::System).await?;
    let inserted = repository
        .insert_state(mutation.conn(), project_id, state)
        .await?;
    mutation.commit().await?;

    Ok(inserted.id)
}

/// Append `events` in their own committed mutation.
async fn append(pool: &PgPool, project_id: Uuid, events: &[NewTaskEvent]) -> Result<Vec<i64>> {
    let repository = TaskRepository::new(pool);
    let mut mutation = TrackerMutation::begin(pool, project_id, TaskActor::System).await?;
    let sequences = repository
        .append_task_events(mutation.conn(), project_id, events)
        .await?;
    mutation.commit().await?;

    Ok(sequences)
}

fn states_changed_event() -> NewTaskEvent {
    NewTaskEvent::project_wide(task_event_kind::STATES_CHANGED, json!({ "states": [] }))
}

#[tokio::test]
async fn a_mutation_locks_a_project_that_exists_and_refuses_one_that_does_not() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let mutation = TrackerMutation::begin(&pool, fixture.project_id, TaskActor::System)
        .await
        .expect("the mutation opens");
    mutation.no_change().await.unwrap();

    let error = TrackerMutation::begin(&pool, Uuid::new_v4(), TaskActor::System)
        .await
        .err()
        .expect("there is no such project");
    assert_eq!(error.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn mutations_serialise_per_project_and_not_across_projects() {
    // Three transactions at once, plus the ones the helpers take.
    let (_postgres, pool) = common::db::test_pool_with(6).await;
    let fixture = seed(&pool).await;
    let other_project = seed_project(&pool).await;

    // The first mutation takes the lock and keeps it.
    let first = TrackerMutation::begin(&pool, fixture.project_id, TaskActor::System)
        .await
        .expect("the first mutation opens");

    // The second wants the same project and must wait at the lock.
    let second_pool = pool.clone();
    let project_id = fixture.project_id;
    let second = tokio::spawn(async move {
        let mutation = TrackerMutation::begin(&second_pool, project_id, TaskActor::System)
            .await
            .unwrap();
        mutation.commit().await.unwrap();
    });

    // The third wants a different project row and must not.
    let third_pool = pool.clone();
    let third = tokio::spawn(async move {
        let mutation = TrackerMutation::begin(&third_pool, other_project, TaskActor::System)
            .await
            .unwrap();
        mutation.commit().await.unwrap();
    });

    timeout(UNBLOCKED_WITHIN, third)
        .await
        .expect("another project's mutation is not blocked")
        .expect("the third transaction does not panic");

    sleep(BLOCKED_FOR).await;
    assert!(
        !second.is_finished(),
        "the second mutation did not wait for the project lock",
    );

    first.commit().await.unwrap();

    timeout(UNBLOCKED_WITHIN, second)
        .await
        .expect("the second mutation proceeds once the lock is free")
        .expect("the second transaction does not panic");
}

#[tokio::test]
async fn the_default_states_are_the_documented_set() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let repository = TaskRepository::new(&pool);

    seed_default_states(&pool, fixture.project_id).await;

    let states = repository.list_states(fixture.project_id).await.unwrap();
    let documented: Vec<(String, TaskStateKind, i32)> = DEFAULT_TASK_STATES
        .iter()
        .map(|(name, kind, position)| (name.to_string(), *kind, *position))
        .collect();
    let stored: Vec<(String, TaskStateKind, i32)> = states
        .iter()
        .map(|state| (state.name.clone(), state.kind, state.position))
        .collect();
    assert_eq!(stored, documented);

    // Every state belongs to the project, and every one of them is findable
    // both ways round.
    for state in &states {
        assert_eq!(state.project_id, fixture.project_id);
        assert_eq!(
            repository
                .find_state(fixture.project_id, state.id)
                .await
                .unwrap()
                .as_ref(),
            Some(state),
        );
        assert_eq!(
            repository
                .find_state_by_name(fixture.project_id, &state.name)
                .await
                .unwrap()
                .as_ref(),
            Some(state),
        );
    }

    // The default state of a new task is the lowest-positioned queue state.
    let mut mutation = TrackerMutation::begin(&pool, fixture.project_id, TaskActor::System)
        .await
        .unwrap();
    let default = repository
        .default_state(mutation.conn(), fixture.project_id)
        .await
        .unwrap();
    mutation.no_change().await.unwrap();
    assert_eq!(default.name, "backlog");

    // A state of another project is out of scope, by id and by name.
    let other_project = seed_project(&pool).await;
    assert!(
        repository
            .find_state(other_project, states[0].id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        repository
            .find_state_by_name(other_project, "backlog")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn inserting_a_state_appends_or_shifts_the_states_at_and_after_it() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let repository = TaskRepository::new(&pool);

    seed_default_states(&pool, project_id).await;

    // A missing position appends.
    let appended = new_state(project_id, "archive", TaskStateKind::Terminal);
    insert_state(&pool, project_id, &appended).await.unwrap();
    assert_eq!(
        names(&repository, project_id).await,
        [
            "backlog",
            "ready",
            "review",
            "merge",
            "needs_human",
            "done",
            "cancelled",
            "archive"
        ],
    );

    // An explicit position shifts the states at and after it.
    let mut triage = new_state(project_id, "triage", TaskStateKind::Queue);
    triage.position = Some(1);
    insert_state(&pool, project_id, &triage).await.unwrap();
    assert_eq!(
        names(&repository, project_id).await,
        [
            "backlog",
            "triage",
            "ready",
            "review",
            "merge",
            "needs_human",
            "done",
            "cancelled",
            "archive",
        ],
    );
    assert_eq!(
        positions(&repository, project_id).await,
        (0..9).collect::<Vec<_>>()
    );

    // A position beyond the end appends rather than leaving a gap.
    let mut parked = new_state(project_id, "parked", TaskStateKind::Queue);
    parked.position = Some(999);
    insert_state(&pool, project_id, &parked).await.unwrap();
    assert_eq!(
        names(&repository, project_id)
            .await
            .last()
            .map(String::as_str),
        Some("parked"),
    );
    assert_eq!(
        positions(&repository, project_id).await,
        (0..10).collect::<Vec<_>>()
    );

    // A negative position is the caller's mistake.
    let mut negative = new_state(project_id, "nowhere", TaskStateKind::Queue);
    negative.position = Some(-1);
    let error = insert_state(&pool, project_id, &negative)
        .await
        .expect_err("a negative position is rejected");
    assert_eq!(error.status(), StatusCode::BAD_REQUEST);
    assert!(
        repository
            .find_state_by_name(project_id, "nowhere")
            .await
            .unwrap()
            .is_none(),
        "the rejected state was inserted anyway",
    );
}

#[tokio::test]
async fn a_duplicate_name_and_a_second_human_state_are_conflicts() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;

    seed_default_states(&pool, project_id).await;

    let duplicate = new_state(project_id, "ready", TaskStateKind::Queue);
    let error = insert_state(&pool, project_id, &duplicate)
        .await
        .expect_err("the name is taken");
    assert_eq!(error.status(), StatusCode::CONFLICT);
    assert_eq!(error.to_string(), "state name already taken");

    let second_human = new_state(project_id, "escalated", TaskStateKind::Human);
    let error = insert_state(&pool, project_id, &second_human)
        .await
        .expect_err("the project already has a human state");
    assert_eq!(error.status(), StatusCode::CONFLICT);
    assert_eq!(error.to_string(), "project already has a human state");

    // The same name in another project is fine: names are per project.
    let other_project = seed_project(&pool).await;
    let elsewhere = new_state(other_project, "ready", TaskStateKind::Queue);
    insert_state(&pool, other_project, &elsewhere)
        .await
        .expect("another project may reuse the name");
}

#[tokio::test]
async fn renaming_keeps_the_position_and_refuses_a_taken_name() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let repository = TaskRepository::new(&pool);

    seed_default_states(&pool, project_id).await;
    let review = state_id(&repository, project_id, "review").await;

    let mut mutation = TrackerMutation::begin(&pool, project_id, TaskActor::System)
        .await
        .unwrap();
    let renamed = repository
        .rename_state(
            mutation.conn(),
            project_id,
            review,
            &TaskStateName::parse("in-review").unwrap(),
        )
        .await
        .unwrap();
    mutation.commit().await.unwrap();

    assert_eq!(renamed.id, review);
    assert_eq!(renamed.name, "in-review");
    assert_eq!(renamed.position, 2);
    assert_eq!(renamed.kind, TaskStateKind::Queue);
    assert_eq!(
        names(&repository, project_id).await,
        [
            "backlog",
            "ready",
            "in-review",
            "merge",
            "needs_human",
            "done",
            "cancelled"
        ],
    );
    assert_eq!(
        positions(&repository, project_id).await,
        (0..7).collect::<Vec<_>>()
    );

    // A name another state already holds is a conflict.
    let mut mutation = TrackerMutation::begin(&pool, project_id, TaskActor::System)
        .await
        .unwrap();
    let error = repository
        .rename_state(
            mutation.conn(),
            project_id,
            review,
            &TaskStateName::parse("merge").unwrap(),
        )
        .await
        .expect_err("the name is taken");
    mutation.no_change().await.unwrap();
    assert_eq!(error.status(), StatusCode::CONFLICT);
    assert_eq!(error.to_string(), "state name already taken");

    // An unknown state, and one of another project, are both out of scope.
    let other_project = seed_project(&pool).await;
    let mut mutation = TrackerMutation::begin(&pool, other_project, TaskActor::System)
        .await
        .unwrap();
    let error = repository
        .rename_state(
            mutation.conn(),
            other_project,
            review,
            &TaskStateName::parse("elsewhere").unwrap(),
        )
        .await
        .expect_err("the state belongs to another project");
    mutation.no_change().await.unwrap();
    assert_eq!(error.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn moving_a_state_repacks_the_board_order() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let repository = TaskRepository::new(&pool);

    seed_default_states(&pool, project_id).await;
    let merge = state_id(&repository, project_id, "merge").await;

    // Towards the front.
    let mut mutation = TrackerMutation::begin(&pool, project_id, TaskActor::System)
        .await
        .unwrap();
    let moved = repository
        .move_state(mutation.conn(), project_id, merge, 0)
        .await
        .unwrap();
    mutation.commit().await.unwrap();
    assert_eq!(moved.position, 0);
    assert_eq!(
        names(&repository, project_id).await,
        [
            "merge",
            "backlog",
            "ready",
            "review",
            "needs_human",
            "done",
            "cancelled"
        ],
    );
    assert_eq!(
        positions(&repository, project_id).await,
        (0..7).collect::<Vec<_>>()
    );

    // Towards the back, and past the end, which lands it last.
    let mut mutation = TrackerMutation::begin(&pool, project_id, TaskActor::System)
        .await
        .unwrap();
    let moved = repository
        .move_state(mutation.conn(), project_id, merge, 999)
        .await
        .unwrap();
    mutation.commit().await.unwrap();
    assert_eq!(moved.position, 6);
    assert_eq!(
        names(&repository, project_id).await,
        [
            "backlog",
            "ready",
            "review",
            "needs_human",
            "done",
            "cancelled",
            "merge"
        ],
    );
    assert_eq!(
        positions(&repository, project_id).await,
        (0..7).collect::<Vec<_>>()
    );

    // A negative position is the caller's mistake, an unknown state is not
    // found.
    let mut mutation = TrackerMutation::begin(&pool, project_id, TaskActor::System)
        .await
        .unwrap();
    let error = repository
        .move_state(mutation.conn(), project_id, merge, -1)
        .await
        .expect_err("a negative position is rejected");
    assert_eq!(error.status(), StatusCode::BAD_REQUEST);
    let error = repository
        .move_state(mutation.conn(), project_id, Uuid::new_v4(), 0)
        .await
        .expect_err("there is no such state");
    assert_eq!(error.status(), StatusCode::NOT_FOUND);
    mutation.no_change().await.unwrap();
}

#[tokio::test]
async fn deleting_a_state_repacks_and_refuses_the_four_documented_cases() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let repository = TaskRepository::new(&pool);

    seed_default_states(&pool, project_id).await;

    // An ordinary queue state goes, and the board closes up behind it.
    let review = state_id(&repository, project_id, "review").await;
    let mut mutation = TrackerMutation::begin(&pool, project_id, TaskActor::System)
        .await
        .unwrap();
    repository
        .delete_state(mutation.conn(), project_id, review)
        .await
        .unwrap();
    mutation.commit().await.unwrap();
    assert_eq!(
        names(&repository, project_id).await,
        [
            "backlog",
            "ready",
            "merge",
            "needs_human",
            "done",
            "cancelled"
        ],
    );
    assert_eq!(
        positions(&repository, project_id).await,
        (0..6).collect::<Vec<_>>()
    );

    // The human state never goes: escalations would have nowhere to land.
    let needs_human = state_id(&repository, project_id, "needs_human").await;
    let mut mutation = TrackerMutation::begin(&pool, project_id, TaskActor::System)
        .await
        .unwrap();
    let error = repository
        .delete_state(mutation.conn(), project_id, needs_human)
        .await
        .expect_err("the human state is kept");
    mutation.no_change().await.unwrap();
    assert_eq!(error.status(), StatusCode::CONFLICT);
    assert_eq!(error.to_string(), "cannot delete the human state");

    // A state a task is in never goes either.
    let ready = state_id(&repository, project_id, "ready").await;
    seed_task(&pool, project_id, ready, 1).await;
    let mut mutation = TrackerMutation::begin(&pool, project_id, TaskActor::System)
        .await
        .unwrap();
    let error = repository
        .delete_state(mutation.conn(), project_id, ready)
        .await
        .expect_err("a task is in the state");
    mutation.no_change().await.unwrap();
    assert_eq!(error.status(), StatusCode::CONFLICT);
    assert_eq!(error.to_string(), "state is in use by tasks");

    // An unknown state, and one of another project, are out of scope.
    let mut mutation = TrackerMutation::begin(&pool, project_id, TaskActor::System)
        .await
        .unwrap();
    let error = repository
        .delete_state(mutation.conn(), project_id, Uuid::new_v4())
        .await
        .expect_err("there is no such state");
    mutation.no_change().await.unwrap();
    assert_eq!(error.status(), StatusCode::NOT_FOUND);

    // Down to the last queue state and the last terminal state, both of which
    // the project keeps.
    for name in ["backlog", "merge", "done"] {
        let id = state_id(&repository, project_id, name).await;
        let mut mutation = TrackerMutation::begin(&pool, project_id, TaskActor::System)
            .await
            .unwrap();
        repository
            .delete_state(mutation.conn(), project_id, id)
            .await
            .unwrap();
        mutation.commit().await.unwrap();
    }
    assert_eq!(
        names(&repository, project_id).await,
        ["ready", "needs_human", "cancelled"],
    );

    let mut mutation = TrackerMutation::begin(&pool, project_id, TaskActor::System)
        .await
        .unwrap();
    let error = repository
        .delete_state(mutation.conn(), project_id, ready)
        .await
        .expect_err("the last queue state is kept");
    assert_eq!(error.status(), StatusCode::CONFLICT);
    assert_eq!(error.to_string(), "cannot delete the last queue state");

    let cancelled = state_id(&repository, project_id, "cancelled").await;
    let error = repository
        .delete_state(mutation.conn(), project_id, cancelled)
        .await
        .expect_err("the last terminal state is kept");
    mutation.no_change().await.unwrap();
    assert_eq!(error.status(), StatusCode::CONFLICT);
    assert_eq!(error.to_string(), "cannot delete the last terminal state");
}

#[tokio::test]
async fn a_profile_serves_queue_states_of_its_own_project_and_nothing_else() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let profile_id = fixture.profile_id;
    let repository = TaskRepository::new(&pool);

    seed_default_states(&pool, project_id).await;
    let ready = state_id(&repository, project_id, "ready").await;
    let review = state_id(&repository, project_id, "review").await;
    let needs_human = state_id(&repository, project_id, "needs_human").await;

    // The happy path, in board order whatever order the ids arrive in.
    let mut mutation = TrackerMutation::begin(&pool, project_id, TaskActor::System)
        .await
        .unwrap();
    repository
        .set_profile_states(mutation.conn(), project_id, profile_id, &[review, ready])
        .await
        .unwrap();
    mutation.commit().await.unwrap();

    let served = repository.list_profile_states(profile_id).await.unwrap();
    assert_eq!(
        served
            .iter()
            .map(|state| state.name.as_str())
            .collect::<Vec<_>>(),
        ["ready", "review"],
    );

    let by_profile = repository
        .list_profile_state_ids_for_project(project_id)
        .await
        .unwrap();
    assert_eq!(by_profile.get(&profile_id), Some(&vec![ready, review]));

    // The human state is not a queue.
    let mut mutation = TrackerMutation::begin(&pool, project_id, TaskActor::System)
        .await
        .unwrap();
    let error = repository
        .set_profile_states(
            mutation.conn(),
            project_id,
            profile_id,
            &[ready, needs_human],
        )
        .await
        .expect_err("the human state is not servable");
    mutation.no_change().await.unwrap();
    assert_eq!(error.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        error.to_string(),
        "served states must be queue states of this project"
    );

    // Neither is a queue state of another project.
    let other_project = seed_project(&pool).await;
    seed_default_states(&pool, other_project).await;
    let foreign_ready = state_id(&repository, other_project, "ready").await;
    let mut mutation = TrackerMutation::begin(&pool, project_id, TaskActor::System)
        .await
        .unwrap();
    let error = repository
        .set_profile_states(mutation.conn(), project_id, profile_id, &[foreign_ready])
        .await
        .expect_err("the state belongs to another project");
    mutation.no_change().await.unwrap();
    assert_eq!(error.status(), StatusCode::BAD_REQUEST);

    // A profile of another project is out of scope.
    let foreign_profile = seed_profile(&pool, other_project, "default").await;
    let mut mutation = TrackerMutation::begin(&pool, project_id, TaskActor::System)
        .await
        .unwrap();
    let error = repository
        .set_profile_states(mutation.conn(), project_id, foreign_profile, &[ready])
        .await
        .expect_err("the profile belongs to another project");
    mutation.no_change().await.unwrap();
    assert_eq!(error.status(), StatusCode::NOT_FOUND);

    // None of the refusals changed anything.
    assert_eq!(
        repository
            .list_profile_states(profile_id)
            .await
            .unwrap()
            .len(),
        2,
    );

    // The empty slice is valid: a profile may serve nothing.
    let mut mutation = TrackerMutation::begin(&pool, project_id, TaskActor::System)
        .await
        .unwrap();
    repository
        .set_profile_states(mutation.conn(), project_id, profile_id, &[])
        .await
        .unwrap();
    mutation.commit().await.unwrap();
    assert!(
        repository
            .list_profile_states(profile_id)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        !repository
            .list_profile_state_ids_for_project(project_id)
            .await
            .unwrap()
            .contains_key(&profile_id)
    );
}

#[tokio::test]
async fn appended_events_take_successive_sequences_per_project() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let repository = TaskRepository::new(&pool);

    // An empty batch writes nothing.
    assert!(append(&pool, project_id, &[]).await.unwrap().is_empty());
    assert_eq!(repository.max_task_event_seq(project_id).await.unwrap(), 0);

    let batch = vec![states_changed_event(), states_changed_event()];
    assert_eq!(append(&pool, project_id, &batch).await.unwrap(), [1, 2]);
    assert_eq!(append(&pool, project_id, &batch).await.unwrap(), [3, 4]);
    assert_eq!(repository.max_task_event_seq(project_id).await.unwrap(), 4);

    let replayed = repository
        .list_task_events_after(project_id, 2, 100)
        .await
        .unwrap();
    assert_eq!(
        replayed.iter().map(|row| row.seq).collect::<Vec<_>>(),
        [3, 4]
    );
    for row in &replayed {
        assert_eq!(row.project_id, project_id);
        assert!(row.task_id.is_none(), "a states_changed event has no task");
        assert_eq!(row.kind, task_event_kind::STATES_CHANGED);
    }

    // The limit is applied, and a cursor at the end replays nothing.
    assert_eq!(
        repository
            .list_task_events_after(project_id, 0, 1)
            .await
            .unwrap()
            .len(),
        1,
    );
    assert!(
        repository
            .list_task_events_after(project_id, 4, 100)
            .await
            .unwrap()
            .is_empty()
    );

    // Sequences are per project: another project starts at 1 again.
    let other_project = seed_project(&pool).await;
    assert_eq!(
        append(&pool, other_project, &[states_changed_event()])
            .await
            .unwrap(),
        [1],
    );
    assert_eq!(repository.max_task_event_seq(project_id).await.unwrap(), 4);
}

#[tokio::test]
async fn an_event_task_must_belong_to_the_project_unless_it_was_deleted() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let repository = TaskRepository::new(&pool);

    seed_default_states(&pool, project_id).await;
    let ready = state_id(&repository, project_id, "ready").await;
    let task_id = seed_task(&pool, project_id, ready, 1).await;

    // A task of this project is fine.
    assert_eq!(
        append(
            &pool,
            project_id,
            &[NewTaskEvent::about(
                task_id,
                task_event_kind::CREATED,
                json!({ "actor": { "kind": "system" } }),
            )],
        )
        .await
        .unwrap(),
        [1],
    );

    // A task of another project is not, and the batch writes nothing.
    let other_project = seed_project(&pool).await;
    seed_default_states(&pool, other_project).await;
    let foreign_ready = state_id(&repository, other_project, "ready").await;
    let foreign_task = seed_task(&pool, other_project, foreign_ready, 1).await;

    let error = append(
        &pool,
        project_id,
        &[
            states_changed_event(),
            NewTaskEvent::about(foreign_task, task_event_kind::UPDATED, json!({})),
        ],
    )
    .await
    .expect_err("the task belongs to another project");
    assert_eq!(error.status(), StatusCode::NOT_FOUND);
    assert_eq!(repository.max_task_event_seq(project_id).await.unwrap(), 1);

    // A `deleted` event keeps the original id of a task that is already gone,
    // which is exactly why it is not checked (ADR 0022).
    let gone = Uuid::new_v4();
    assert_eq!(
        append(
            &pool,
            project_id,
            &[NewTaskEvent::about(
                gone,
                task_event_kind::DELETED,
                json!({ "actor": { "kind": "system" } }),
            )],
        )
        .await
        .unwrap(),
        [2],
    );
    let stored = repository
        .list_task_events_after(project_id, 1, 100)
        .await
        .unwrap();
    assert_eq!(stored[0].task_id, Some(gone));
    assert_eq!(stored[0].kind, task_event_kind::DELETED);
}

#[tokio::test]
async fn a_batch_notifies_once_on_commit_and_never_on_rollback() {
    let (_postgres, pool) = common::db::test_pool_with(6).await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let repository = TaskRepository::new(&pool);

    let mut listener = PgListener::connect_with(&pool).await.unwrap();
    listener.listen("task_events").await.unwrap();

    let batch = vec![
        states_changed_event(),
        states_changed_event(),
        states_changed_event(),
    ];
    append(&pool, project_id, &batch).await.unwrap();

    let notification = timeout(UNBLOCKED_WITHIN, listener.recv())
        .await
        .expect("a committed batch notifies")
        .unwrap();
    assert_eq!(notification.channel(), "task_events");
    // One notification per batch, carrying its highest sequence.
    assert_eq!(notification.payload(), format!("{project_id}:3"));

    // A rolled-back batch publishes neither its rows nor its notification
    // (ADR 0028).
    let mut mutation = TrackerMutation::begin(&pool, project_id, TaskActor::System)
        .await
        .unwrap();
    repository
        .append_task_events(mutation.conn(), project_id, &batch)
        .await
        .unwrap();
    mutation.no_change().await.unwrap();
    assert_eq!(repository.max_task_event_seq(project_id).await.unwrap(), 3);

    // The next committed batch continues from 4, and its notification is the
    // only one waiting.
    append(&pool, project_id, &[states_changed_event()])
        .await
        .unwrap();
    let notification = timeout(UNBLOCKED_WITHIN, listener.recv())
        .await
        .expect("the next committed batch notifies")
        .unwrap();
    assert_eq!(notification.payload(), format!("{project_id}:4"));
}
