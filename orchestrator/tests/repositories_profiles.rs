//! Agent profiles against a real Postgres: served states, the default-profile
//! invariant and the two refusals a delete can hit (`CLAUDE.md`, "Testing
//! expectations").
//!
//! The profile CRUD round trip lives in `tests/repositories_projects.rs` with
//! the rest of the project aggregate. What is asserted here is everything that
//! spans two tables and therefore has no constraint behind it.
//!
//! `serves_states` is the `profile_states` link read back as state *names* in
//! board order, and written the way `ProfileInput` sends it — by name, queue
//! states only, with a message that lists the names the caller may use.
//!
//! The default-profile invariant is "exactly one per project". The partial
//! unique index can only refuse a second one; moving the flag, and refusing to
//! drop it from the last one, are the repository's, so both are asserted
//! against the stored rows rather than against the index.
//!
//! Needs a container engine; see `tests/common/db.rs`.

mod common;

use std::collections::HashMap;

use axum::http::StatusCode;
use mars_orchestrator::models::{AgentProfile, ProfileInput, ProfileUpdate};
use mars_orchestrator::prelude::*;
use mars_orchestrator::repositories::{ProjectRepository, TaskRepository};
use uuid::Uuid;

/// Not a real image: the stub the session tests replay a fixture transcript
/// with (`CLAUDE.md`, rule 3).
const TEST_IMAGE: &str = "mars-session-stub:test";

/// The configuration a resolved `ProfileInput` reads its image default from.
/// Obviously fake values throughout; nothing here is a credential (rule 3).
fn test_config() -> Config {
    let vars: HashMap<&str, &str> = [
        ("PUBLIC_URL", "https://mars.example.invalid"),
        ("JWT_SECRET", "not-a-real-signing-secret"),
        ("DATABASE_URL", "postgres://mars:fake@localhost:5432/mars"),
        ("DOCKER_HOST", "unix:///run/user/1000/podman/podman.sock"),
        ("DATA_DIR_HOST", "/srv/mars/data"),
        ("SECRETS_MASTER_KEYS", "1=not-a-real-key"),
        ("GIT_BOT_NAME", "Mars Bot"),
        ("GIT_BOT_EMAIL", "mars-bot@example.invalid"),
        ("SESSION_IMAGE_DEFAULT", TEST_IMAGE),
    ]
    .into_iter()
    .collect();

    Config::from_vars(|name| vars.get(name).map(|value| value.to_string()))
        .expect("a complete required set loads")
}

/// A committed project with the documented default state set.
///
/// The project row is written directly: this file is about profiles, and the
/// row is only here to be the scope and the lock.
async fn seeded_project(pool: &PgPool) -> Uuid {
    let project_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id, name, remote_url) VALUES ($1, $2, $3)")
        .bind(project_id)
        .bind(format!("project-{project_id}"))
        // `.invalid` can never resolve (`CLAUDE.md`, rule 3).
        .bind("https://example.invalid/org/repo.git")
        .execute(pool)
        .await
        .expect("the project seeds");

    let tasks = TaskRepository::new(pool);
    let mut tx = tasks
        .begin_mutation(project_id)
        .await
        .expect("the mutation opens");
    tasks
        .insert_default_states(&mut tx, project_id)
        .await
        .expect("the default states insert");
    tx.commit().await.expect("the transaction commits");

    project_id
}

/// The body a user would post, with everything else defaulted.
fn input(name: &str) -> ProfileInput {
    ProfileInput {
        name: name.to_string(),
        ..ProfileInput::default()
    }
}

/// Insert `input` as a profile of `project_id`, in one committed mutation,
/// writing its served states in the same transaction.
///
/// This is the shape every caller has: resolve the input, lock the project,
/// insert, then set the link rows (`docs/data-model.md`, "Tracker mutation
/// transactions").
async fn insert(pool: &PgPool, project_id: Uuid, input: ProfileInput) -> Result<AgentProfile> {
    let profile = input
        .resolve_new(project_id, &test_config())
        .expect("the test input is valid");

    let projects = ProjectRepository::new(pool);
    let tasks = TaskRepository::new(pool);
    let mut tx = tasks.begin_mutation(project_id).await?;
    let inserted = projects.insert_profile(&mut tx, &profile).await?;
    tasks
        .set_profile_states_by_name(&mut tx, project_id, inserted.id, &profile.serves_states)
        .await?;
    tx.commit().await?;

    projects
        .find_profile(project_id, inserted.id)
        .await
        .map(|profile| profile.expect("the profile was just inserted"))
}

/// Insert `input` expecting it to work.
async fn insert_ok(pool: &PgPool, project_id: Uuid, input: ProfileInput) -> AgentProfile {
    insert(pool, project_id, input)
        .await
        .expect("the profile inserts")
}

/// Apply `update` to `id` in one committed mutation.
async fn update(
    pool: &PgPool,
    project_id: Uuid,
    id: Uuid,
    update: &ProfileUpdate,
) -> Result<Option<AgentProfile>> {
    let projects = ProjectRepository::new(pool);
    let tasks = TaskRepository::new(pool);
    let mut tx = tasks.begin_mutation(project_id).await?;
    let updated = projects
        .update_profile(&mut tx, project_id, id, update)
        .await;
    match updated {
        Ok(updated) => {
            tx.commit().await?;
            Ok(updated)
        }
        Err(error) => {
            tx.rollback().await?;
            Err(error)
        }
    }
}

/// Replace the states `profile_id` serves, by name, in one committed mutation.
async fn set_served(
    pool: &PgPool,
    project_id: Uuid,
    profile_id: Uuid,
    names: &[&str],
) -> Result<()> {
    let names: Vec<String> = names.iter().map(|name| name.to_string()).collect();
    let tasks = TaskRepository::new(pool);
    let mut tx = tasks.begin_mutation(project_id).await?;
    match tasks
        .set_profile_states_by_name(&mut tx, project_id, profile_id, &names)
        .await
    {
        Ok(()) => {
            tx.commit().await?;
            Ok(())
        }
        Err(error) => {
            tx.rollback().await?;
            Err(error)
        }
    }
}

/// Seed a session row directly.
///
/// `SessionRepository` belongs to another task; this test only needs a row
/// that makes the profile undeletable. The token hash is an obviously fake
/// constant, never a credential (`CLAUDE.md`, rule 3).
async fn seed_session(pool: &PgPool, project_id: Uuid, profile_id: Uuid) {
    let id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO sessions (id, project_id, profile_id, kind, base_ref, branch, mcp_token_hash)
        VALUES ($1, $2, $3, 'conversational'::profile_kind, 'main', $4, $5)
        "#,
    )
    .bind(id)
    .bind(project_id)
    .bind(profile_id)
    .bind(format!("session/{id}"))
    .bind(format!("fake-token-hash-{id}"))
    .execute(pool)
    .await
    .expect("the session seeds");
}

fn assert_conflict(error: Error, message: &str) {
    assert_eq!(
        error.status(),
        StatusCode::CONFLICT,
        "not a conflict: {error}"
    );
    assert_eq!(error.to_string(), message);
}

fn assert_bad_request(error: Error, message: &str) {
    assert_eq!(
        error.status(),
        StatusCode::BAD_REQUEST,
        "not a bad request: {error}"
    );
    assert_eq!(error.to_string(), message);
}

/// The project's default queue states, in board order, as the refusal message
/// lists them (`docs/data-model.md`, `task_states`).
const QUEUE_STATES: &str = "backlog, ready, review, merge";

#[tokio::test]
async fn a_profile_serves_the_states_it_was_given_and_reads_them_back_in_board_order() {
    let (_postgres, pool) = common::db::test_pool().await;
    let project_id = seeded_project(&pool).await;
    let repository = ProjectRepository::new(&pool);

    // Nothing was said, so the input defaulted to `["ready"]`.
    let profile = insert_ok(&pool, project_id, input("default")).await;
    assert_eq!(profile.serves_states, ["ready"]);
    assert_eq!(profile.image, TEST_IMAGE);

    // Board order, not the order the caller listed them in.
    set_served(&pool, project_id, profile.id, &["review", "backlog"])
        .await
        .expect("both are queue states");
    let profile = repository
        .find_profile(project_id, profile.id)
        .await
        .unwrap()
        .expect("the profile exists");
    assert_eq!(profile.serves_states, ["backlog", "review"]);

    // Every read carries the link, including the list and the default lookup.
    let listed = repository.list_profiles(project_id).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].serves_states, ["backlog", "review"]);

    // A profile may serve nothing: `ready` then offers it no tasks.
    set_served(&pool, project_id, profile.id, &[])
        .await
        .expect("the empty set is valid");
    assert!(
        repository
            .find_profile(project_id, profile.id)
            .await
            .unwrap()
            .expect("the profile exists")
            .serves_states
            .is_empty()
    );

    // A second profile of the same project keeps its own links.
    let mut planner = input("planner");
    planner.serves_states = Some(vec!["backlog".to_string()]);
    let planner = insert_ok(&pool, project_id, planner).await;
    assert_eq!(planner.serves_states, ["backlog"]);
    let listed = repository.list_profiles(project_id).await.unwrap();
    assert_eq!(
        listed
            .iter()
            .map(|profile| (profile.name.as_str(), profile.serves_states.clone()))
            .collect::<Vec<_>>(),
        [
            ("default", Vec::new()),
            ("planner", vec!["backlog".to_string()]),
        ],
    );
}

#[tokio::test]
async fn a_served_state_that_is_not_a_queue_state_of_the_project_is_a_bad_request() {
    let (_postgres, pool) = common::db::test_pool().await;
    let project_id = seeded_project(&pool).await;
    let repository = ProjectRepository::new(&pool);

    let profile = insert_ok(&pool, project_id, input("default")).await;

    // The human state exists, but escalations are not a queue to claim from.
    let error = set_served(&pool, project_id, profile.id, &["needs_human"])
        .await
        .expect_err("the human state is not servable");
    assert_bad_request(
        error,
        &format!(
            "serves_states: \"needs_human\" is not a queue state of this project; \
             queue states are: {QUEUE_STATES}"
        ),
    );

    // An unknown name is the same mistake, and gets the same list.
    let error = set_served(&pool, project_id, profile.id, &["nope"])
        .await
        .expect_err("there is no such state");
    assert_bad_request(
        error,
        &format!(
            "serves_states: \"nope\" is not a queue state of this project; \
             queue states are: {QUEUE_STATES}"
        ),
    );

    // A queue state of another project is not this project's either.
    let other = seeded_project(&pool).await;
    let tasks = TaskRepository::new(&pool);
    let mut tx = tasks.begin_mutation(other).await.unwrap();
    let renamed = tasks
        .find_state_by_name(other, "backlog")
        .await
        .unwrap()
        .expect("the default set has a backlog state");
    tasks
        .rename_state(
            &mut tx,
            other,
            renamed.id,
            &mars_orchestrator::models::TaskStateName::parse("icebox").unwrap(),
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let error = set_served(&pool, project_id, profile.id, &["icebox"])
        .await
        .expect_err("the state belongs to another project");
    assert_bad_request(
        error,
        &format!(
            "serves_states: \"icebox\" is not a queue state of this project; \
             queue states are: {QUEUE_STATES}"
        ),
    );

    // The default `["ready"]` is not exempt: a project whose `ready` state has
    // been renamed refuses it, and says which names it would take.
    let tasks = TaskRepository::new(&pool);
    let ready = tasks
        .find_state_by_name(project_id, "ready")
        .await
        .unwrap()
        .expect("the default set has a ready state");
    let mut tx = tasks.begin_mutation(project_id).await.unwrap();
    tasks
        .rename_state(
            &mut tx,
            project_id,
            ready.id,
            &mars_orchestrator::models::TaskStateName::parse("planned").unwrap(),
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let error = insert(&pool, project_id, input("implementer"))
        .await
        .expect_err("the project has no state named ready any more");
    assert_bad_request(
        error,
        "serves_states: \"ready\" is not a queue state of this project; \
         queue states are: backlog, planned, review, merge",
    );

    // Nothing was written by any of the refusals.
    assert_eq!(
        repository
            .find_profile(project_id, profile.id)
            .await
            .unwrap()
            .expect("the profile exists")
            .serves_states,
        ["planned"],
    );
    assert_eq!(repository.list_profiles(project_id).await.unwrap().len(), 1);
}

#[tokio::test]
async fn the_default_profile_flag_moves_and_is_never_dropped() {
    let (_postgres, pool) = common::db::test_pool().await;
    let project_id = seeded_project(&pool).await;
    let repository = ProjectRepository::new(&pool);

    let mut first = input("default");
    first.is_default = Some(true);
    let first = insert_ok(&pool, project_id, first).await;
    assert!(first.is_default);

    // A second insert that claims the flag takes it, rather than being refused
    // by the partial unique index.
    let mut planner = input("planner");
    planner.is_default = Some(true);
    let planner = insert_ok(&pool, project_id, planner).await;
    assert!(planner.is_default);
    assert_eq!(
        repository
            .find_default_profile(project_id)
            .await
            .unwrap()
            .map(|profile| profile.id),
        Some(planner.id),
    );
    assert!(
        !repository
            .find_profile(project_id, first.id)
            .await
            .unwrap()
            .expect("the profile exists")
            .is_default
    );

    // And the update path moves it back.
    let mut promote = ProfileUpdate::from(&first);
    promote.is_default = Some(true);
    let first = update(&pool, project_id, first.id, &promote)
        .await
        .unwrap()
        .expect("the profile exists");
    assert!(first.is_default);
    assert_eq!(
        repository
            .list_profiles(project_id)
            .await
            .unwrap()
            .iter()
            .filter(|profile| profile.is_default)
            .count(),
        1,
    );

    // Promoting the profile that already has it changes nothing.
    let again = update(&pool, project_id, first.id, &promote)
        .await
        .unwrap()
        .expect("the profile exists");
    assert!(again.is_default);

    // An update that says nothing about the flag leaves it alone, on both the
    // default profile and the other one.
    let mut renamed = ProfileUpdate::from(&again);
    renamed.name = "seeded".to_string();
    renamed.is_default = None;
    let renamed = update(&pool, project_id, first.id, &renamed)
        .await
        .unwrap()
        .expect("the profile exists");
    assert_eq!(renamed.name, "seeded");
    assert!(renamed.is_default);

    let mut untouched = ProfileUpdate::from(&planner);
    untouched.is_default = None;
    let untouched = update(&pool, project_id, planner.id, &untouched)
        .await
        .unwrap()
        .expect("the profile exists");
    assert!(!untouched.is_default);

    // The project must keep a default, so the flag is moved, never cleared.
    let mut demote = ProfileUpdate::from(&renamed);
    demote.is_default = Some(false);
    let error = update(&pool, project_id, first.id, &demote)
        .await
        .expect_err("the project would be left without a default profile");
    assert_conflict(error, "project must keep a default profile");

    // Clearing a flag that is already clear is not that refusal.
    let mut stay = ProfileUpdate::from(&untouched);
    stay.is_default = Some(false);
    assert!(
        !update(&pool, project_id, planner.id, &stay)
            .await
            .unwrap()
            .expect("the profile exists")
            .is_default
    );

    // Another project has its own default slot.
    let other = seeded_project(&pool).await;
    let mut other_default = input("default");
    other_default.is_default = Some(true);
    assert!(insert_ok(&pool, other, other_default).await.is_default);
    assert_eq!(
        repository
            .find_default_profile(project_id)
            .await
            .unwrap()
            .map(|profile| profile.id),
        Some(first.id),
    );
}

#[tokio::test]
async fn the_default_profile_and_a_profile_with_sessions_cannot_be_deleted() {
    let (_postgres, pool) = common::db::test_pool().await;
    let project_id = seeded_project(&pool).await;
    let repository = ProjectRepository::new(&pool);
    let tasks = TaskRepository::new(&pool);

    let mut seeded = input("default");
    seeded.is_default = Some(true);
    let seeded = insert_ok(&pool, project_id, seeded).await;
    let planner = insert_ok(&pool, project_id, input("planner")).await;
    let spare = insert_ok(&pool, project_id, input("spare")).await;

    let mut tx = tasks.begin_mutation(project_id).await.unwrap();
    let error = repository
        .delete_profile(&mut tx, project_id, seeded.id)
        .await
        .expect_err("the default profile stays");
    tx.rollback().await.unwrap();
    assert_conflict(error, "the default profile cannot be deleted");

    // A profile that has ever run a session keeps the transcript alive, so the
    // `ON DELETE RESTRICT` case answers 409 rather than surfacing as a 500.
    seed_session(&pool, project_id, planner.id).await;
    let mut tx = tasks.begin_mutation(project_id).await.unwrap();
    assert_eq!(
        repository
            .profile_session_count(&mut tx, planner.id)
            .await
            .unwrap(),
        1,
    );
    let error = repository
        .delete_profile(&mut tx, project_id, planner.id)
        .await
        .expect_err("the profile still has a session");
    tx.rollback().await.unwrap();
    assert_conflict(error, "profile has sessions");

    // Neither refusal removed anything, and an ordinary profile still goes.
    let mut tx = tasks.begin_mutation(project_id).await.unwrap();
    assert_eq!(
        repository
            .profile_session_count(&mut tx, spare.id)
            .await
            .unwrap(),
        0,
    );
    assert!(
        repository
            .delete_profile(&mut tx, project_id, spare.id)
            .await
            .unwrap()
    );
    // Deleting it twice is not an error, and neither is a foreign project.
    assert!(
        !repository
            .delete_profile(&mut tx, project_id, spare.id)
            .await
            .unwrap()
    );
    assert!(
        !repository
            .delete_profile(&mut tx, Uuid::new_v4(), planner.id)
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();

    assert_eq!(
        repository
            .list_profiles(project_id)
            .await
            .unwrap()
            .iter()
            .map(|profile| profile.name.as_str())
            .collect::<Vec<_>>(),
        ["default", "planner"],
    );
    // The deleted profile's links went with it.
    assert!(
        tasks
            .list_profile_states(spare.id)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn a_profile_of_another_project_is_out_of_scope_for_its_served_states() {
    let (_postgres, pool) = common::db::test_pool().await;
    let project_id = seeded_project(&pool).await;
    let other = seeded_project(&pool).await;

    let profile = insert_ok(&pool, project_id, input("default")).await;

    let error = set_served(&pool, other, profile.id, &["ready"])
        .await
        .expect_err("the profile belongs to another project");
    assert_eq!(error.status(), StatusCode::NOT_FOUND);

    // And the profile kept what it had.
    assert_eq!(
        ProjectRepository::new(&pool)
            .find_profile(project_id, profile.id)
            .await
            .unwrap()
            .expect("the profile exists")
            .serves_states,
        ["ready"],
    );
}
