//! The board's columns, through the `tracker::states` verbs against a real
//! Postgres (`CLAUDE.md`, "Testing expectations").
//!
//! Three things are being asserted here.
//!
//! The state configuration is the documented default set, the position
//! arithmetic `SPEC.md`, "Task states" promises — a missing position appends,
//! an explicit one shifts, moves and deletions re-pack — and every refusal
//! `ARCHITECTURE.md`, "Task tracker" reserves: a project keeps at least one
//! queue state, exactly one human state and at least one terminal state, and
//! no state a task is still in may be removed. Every one of them is driven
//! through [`create_state`], [`update_state`] and [`delete_state`], which is
//! the surface a REST handler, an MCP tool and a test all share; the
//! repository's row helpers are the tracker's alone (`ARCHITECTURE.md`, "Task
//! tracker").
//!
//! The `profile_states` link is the role mechanism, and its two cross-table
//! rules — same project, `queue` kind — have no constraint behind them, so
//! they are asserted directly.
//!
//! Last, the notification. A board edit is the cheapest mutation that emits
//! more than one event, so it is what "one `pg_notify` per committed batch,
//! carrying its highest sequence, and none per rolled-back one" (ADR 0028) is
//! proved with here, at the [`TrackerMutation`] seam and through a verb. The
//! lock the mutation takes is asserted in `tests/tracker_mutation.rs`.
//!
//! Needs a container engine; see `tests/common/db.rs`.

mod common;

use std::time::Duration;

use axum::http::StatusCode;
use mars_orchestrator::events::TaskActor;
use mars_orchestrator::models::{DEFAULT_TASK_STATES, TaskStateKind};
use mars_orchestrator::prelude::*;
use mars_orchestrator::repositories::TaskRepository;
use mars_orchestrator::tracker::{
    NewStateInput, StateUpdate, TrackerMutation, create_state, delete_state, update_state,
};
use sqlx::postgres::PgListener;
use tokio::time::timeout;
use uuid::Uuid;

/// Not a credential: an obviously fake stand-in for the Argon2id PHC string
/// the seeded user would carry (`CLAUDE.md`, rule 3).
const FAKE_PASSWORD_HASH: &str = "$argon2id$fake$hash";

/// Not a real image: the stub the session tests replay a fixture transcript
/// with.
const TEST_IMAGE: &str = "localhost/mars-session-stub:test";

/// How long an expected notification is given once nothing should be in its
/// way.
const UNBLOCKED_WITHIN: Duration = Duration::from_secs(10);

/// The rows a project's tracker needs to exist at all.
///
/// Seeded with unchecked statements rather than through their repositories:
/// this file is about the state verbs, and the foreign keys are all it needs
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

/// A bare `tasks` row, for the one deletion refusal that needs a task to be in
/// the state.
///
/// Written directly rather than through `create_task`: this file is about the
/// columns, and "some task is in this state" is all the refusal reads.
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

fn new_state(name: &str, kind: TaskStateKind, position: Option<i32>) -> NewStateInput {
    NewStateInput {
        name: name.to_string(),
        kind,
        position,
        auto_merge: Default::default(),
    }
}

/// Run `body` against an open mutation on `project_id`, committing it when the
/// verb succeeded and rolling it back when it did not.
///
/// The pairing every transport makes (`tracker::commit_and_notify`), without
/// the email side no state edit owes.
async fn in_mutation<T, F>(pool: &PgPool, project_id: Uuid, body: F) -> Result<T>
where
    F: AsyncFnOnce(&mut TrackerMutation<'_>) -> Result<T>,
{
    let mut mutation = TrackerMutation::begin(pool, project_id, TaskActor::System).await?;
    match body(&mut mutation).await {
        Ok(outcome) => {
            mutation.commit().await?;
            Ok(outcome)
        }
        Err(error) => {
            mutation.no_change().await?;
            Err(error)
        }
    }
}

/// Add a column in its own committed mutation.
async fn create(pool: &PgPool, project_id: Uuid, input: NewStateInput) -> Result<Uuid> {
    in_mutation(pool, project_id, async |m| {
        create_state(m, input).await.map(|state| state.id)
    })
    .await
}

/// Rename and/or move a column in its own committed mutation.
async fn update(
    pool: &PgPool,
    project_id: Uuid,
    name: &str,
    update: StateUpdate,
) -> Result<(String, i32, bool)> {
    in_mutation(pool, project_id, async |m| {
        update_state(m, name, update)
            .await
            .map(|(state, changed)| (state.name, state.position, changed))
    })
    .await
}

/// Remove a column in its own committed mutation.
async fn delete(pool: &PgPool, project_id: Uuid, name: &str) -> Result<()> {
    in_mutation(pool, project_id, async |m| delete_state(m, name).await).await
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
async fn creating_a_state_appends_or_shifts_the_states_at_and_after_it() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let repository = TaskRepository::new(&pool);

    seed_default_states(&pool, project_id).await;

    // A missing position appends.
    create(
        &pool,
        project_id,
        new_state("archive", TaskStateKind::Terminal, None),
    )
    .await
    .unwrap();
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
    create(
        &pool,
        project_id,
        new_state("triage", TaskStateKind::Queue, Some(1)),
    )
    .await
    .unwrap();
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
    create(
        &pool,
        project_id,
        new_state("parked", TaskStateKind::Queue, Some(999)),
    )
    .await
    .unwrap();
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
    let error = create(
        &pool,
        project_id,
        new_state("nowhere", TaskStateKind::Queue, Some(-1)),
    )
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

    // And so is a name that is not one, before any row is written.
    let error = create(
        &pool,
        project_id,
        new_state("Not A Name", TaskStateKind::Queue, None),
    )
    .await
    .expect_err("the name does not parse");
    assert_eq!(error.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        positions(&repository, project_id).await,
        (0..10).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn a_duplicate_name_and_a_second_human_state_are_conflicts() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;

    seed_default_states(&pool, project_id).await;

    let error = create(
        &pool,
        project_id,
        new_state("ready", TaskStateKind::Queue, None),
    )
    .await
    .expect_err("the name is taken");
    assert_eq!(error.status(), StatusCode::CONFLICT);
    assert_eq!(error.to_string(), "state name already taken");

    let error = create(
        &pool,
        project_id,
        new_state("escalated", TaskStateKind::Human, None),
    )
    .await
    .expect_err("the project already has a human state");
    assert_eq!(error.status(), StatusCode::CONFLICT);
    assert_eq!(error.to_string(), "project already has a human state");

    // The same name in another project is fine: names are per project.
    let other_project = seed_project(&pool).await;
    seed_default_states(&pool, other_project).await;
    create(
        &pool,
        other_project,
        new_state("triage", TaskStateKind::Queue, None),
    )
    .await
    .expect("another project has its own board");
}

#[tokio::test]
async fn renaming_and_moving_repack_the_board_and_refuse_a_taken_name() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let repository = TaskRepository::new(&pool);

    seed_default_states(&pool, project_id).await;

    // A rename keeps the position and the kind.
    let (name, position, changed) = update(
        &pool,
        project_id,
        "review",
        StateUpdate {
            name: Some("in-review".into()),
            ..StateUpdate::default()
        },
    )
    .await
    .unwrap();
    assert_eq!((name.as_str(), position, changed), ("in-review", 2, true));
    assert_eq!(
        repository
            .find_state_by_name(project_id, "in-review")
            .await
            .unwrap()
            .expect("the renamed state is there")
            .kind,
        TaskStateKind::Queue,
    );

    // Towards the front, and then past the end, which lands it last. Both
    // re-pack every position to `0..n`.
    let (_, position, changed) = update(
        &pool,
        project_id,
        "merge",
        StateUpdate {
            position: Some(0),
            ..StateUpdate::default()
        },
    )
    .await
    .unwrap();
    assert_eq!((position, changed), (0, true));
    assert_eq!(
        names(&repository, project_id).await,
        [
            "merge",
            "backlog",
            "ready",
            "in-review",
            "needs_human",
            "done",
            "cancelled"
        ],
    );

    let (_, position, changed) = update(
        &pool,
        project_id,
        "merge",
        StateUpdate {
            position: Some(999),
            ..StateUpdate::default()
        },
    )
    .await
    .unwrap();
    assert_eq!((position, changed), (6, true));
    assert_eq!(
        names(&repository, project_id).await,
        [
            "backlog",
            "ready",
            "in-review",
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

    // Asking for what is already true changes nothing and says so, which is
    // what keeps the event stream quiet.
    let (_, _, changed) = update(
        &pool,
        project_id,
        "merge",
        StateUpdate {
            name: Some("merge".into()),
            position: Some(999),
            ..StateUpdate::default()
        },
    )
    .await
    .unwrap();
    assert!(!changed, "a move to where it already is is not a change");

    // A name another state already holds is a conflict.
    let error = update(
        &pool,
        project_id,
        "in-review",
        StateUpdate {
            name: Some("merge".into()),
            ..StateUpdate::default()
        },
    )
    .await
    .expect_err("the name is taken");
    assert_eq!(error.status(), StatusCode::CONFLICT);
    assert_eq!(error.to_string(), "state name already taken");

    // A negative position is refused before anything is written, even when a
    // legal rename came with it.
    let error = update(
        &pool,
        project_id,
        "in-review",
        StateUpdate {
            name: Some("triage".into()),
            position: Some(-1),
            ..StateUpdate::default()
        },
    )
    .await
    .expect_err("a negative position is rejected");
    assert_eq!(error.status(), StatusCode::BAD_REQUEST);
    assert_eq!(error.to_string(), "position must not be negative");
    assert!(
        repository
            .find_state_by_name(project_id, "triage")
            .await
            .unwrap()
            .is_none(),
        "the rename was applied anyway",
    );

    // A name this project does not have is not found — which is also how a
    // state of another project reads from here.
    let error = update(
        &pool,
        project_id,
        "nowhere",
        StateUpdate {
            name: Some("elsewhere".into()),
            ..StateUpdate::default()
        },
    )
    .await
    .expect_err("there is no such state");
    assert_eq!(error.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn deleting_a_state_repacks_and_refuses_the_four_documented_cases() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let repository = TaskRepository::new(&pool);

    seed_default_states(&pool, project_id).await;

    // An ordinary queue state goes, and the board closes up behind it.
    delete(&pool, project_id, "review").await.unwrap();
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
    let error = delete(&pool, project_id, "needs_human")
        .await
        .expect_err("the human state is kept");
    assert_eq!(error.status(), StatusCode::CONFLICT);
    assert_eq!(error.to_string(), "cannot delete the human state");

    // A state a task is in never goes either.
    let ready = state_id(&repository, project_id, "ready").await;
    seed_task(&pool, project_id, ready, 1).await;
    let error = delete(&pool, project_id, "ready")
        .await
        .expect_err("a task is in the state");
    assert_eq!(error.status(), StatusCode::CONFLICT);
    assert_eq!(error.to_string(), "state is in use by tasks");

    // A name this project does not have is out of scope.
    let error = delete(&pool, project_id, "archive")
        .await
        .expect_err("there is no such state");
    assert_eq!(error.status(), StatusCode::NOT_FOUND);

    // Down to the last queue state and the last terminal state, both of which
    // the project keeps.
    for name in ["backlog", "merge", "done"] {
        delete(&pool, project_id, name).await.unwrap();
    }
    assert_eq!(
        names(&repository, project_id).await,
        ["ready", "needs_human", "cancelled"],
    );

    let error = delete(&pool, project_id, "ready")
        .await
        .expect_err("the last queue state is kept");
    assert_eq!(error.status(), StatusCode::CONFLICT);
    assert_eq!(error.to_string(), "cannot delete the last queue state");

    let error = delete(&pool, project_id, "cancelled")
        .await
        .expect_err("the last terminal state is kept");
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
async fn a_batch_notifies_once_on_commit_and_never_on_rollback() {
    let (_postgres, pool) = common::db::test_pool_with(6).await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let repository = TaskRepository::new(&pool);

    seed_default_states(&pool, project_id).await;

    let mut listener = PgListener::connect_with(&pool).await.unwrap();
    listener.listen("task_events").await.unwrap();

    // Three verb calls in one mutation, so three `states_changed` events in
    // one batch.
    in_mutation(&pool, project_id, async |m| {
        for name in ["archive", "parked", "triage"] {
            create_state(m, new_state(name, TaskStateKind::Queue, None)).await?;
        }
        Ok(())
    })
    .await
    .unwrap();

    let notification = timeout(UNBLOCKED_WITHIN, listener.recv())
        .await
        .expect("a committed batch notifies")
        .unwrap();
    assert_eq!(notification.channel(), "task_events");
    // One notification per batch, carrying its highest sequence.
    assert_eq!(notification.payload(), format!("{project_id}:3"));

    // A rolled-back batch publishes neither its rows nor its notification
    // (ADR 0028). The verb succeeds; the mutation is ended with `no_change`
    // anyway, which is the no-op path every transport has.
    let mut mutation = TrackerMutation::begin(&pool, project_id, TaskActor::System)
        .await
        .unwrap();
    create_state(
        &mut mutation,
        new_state("rolled-back", TaskStateKind::Queue, None),
    )
    .await
    .unwrap();
    mutation.no_change().await.unwrap();

    assert_eq!(repository.max_task_event_seq(project_id).await.unwrap(), 3);
    assert!(
        repository
            .find_state_by_name(project_id, "rolled-back")
            .await
            .unwrap()
            .is_none()
    );

    // The next committed batch continues from 4, and its notification is the
    // only one waiting.
    delete(&pool, project_id, "archive").await.unwrap();
    let notification = timeout(UNBLOCKED_WITHIN, listener.recv())
        .await
        .expect("the next committed batch notifies")
        .unwrap();
    assert_eq!(notification.payload(), format!("{project_id}:4"));
}
