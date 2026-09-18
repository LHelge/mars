//! `ProjectRepository` against a real Postgres (`CLAUDE.md`, "Testing
//! expectations").
//!
//! Three things are being asserted here. The CRUD half is a round trip per
//! table plus every conflict `SPEC.md` promises. The interesting half is the
//! project row lock: `docs/data-model.md`, "Tracker mutation transactions"
//! makes it the serialisation point of every tracker mutation, so it is
//! asserted the only way a lock can be — a second transaction is started while
//! the first still holds it, and the test shows that it does not get through
//! until the first commits, and that the two task numbers it hands out are 1
//! and 2 with no duplicate. A `FOR UPDATE` quietly dropped from the statement
//! would pass every functional assertion and fail here.
//!
//! The third is the two ways the database answers back: the `CHECK` that a
//! `ready` project has a default branch, and the `ON DELETE RESTRICT` that
//! keeps a profile with sessions alive. Both must be conflicts, not 500s.
//!
//! Needs a container engine; see `tests/common/db.rs`.

mod common;

use std::time::Duration;

use axum::http::StatusCode;
use mars_orchestrator::models::{
    AgentBackend, BranchName, EncryptedValue, MaxAttempts, NewAgentProfile, NewProject, NewSecret,
    NewSharedDir, ProfileKind, ProfileUpdate, Project, ProjectName, ProjectStatus, ProjectUpdate,
    RemoteUrl, ScopeRef, SecretName,
};
use mars_orchestrator::prelude::*;
use mars_orchestrator::repositories::{ProjectRepository, SecretRepository, SessionRepository};
use mars_orchestrator::secrets::GIT_CREDENTIAL_NAME;
use tokio::time::{sleep, timeout};
use uuid::Uuid;

/// The administrator the `users` migration seeds (`docs/data-model.md`).
const SEEDED_ADMIN: Uuid = Uuid::from_u128(1);

/// Not a real remote: `.invalid` can never resolve (`CLAUDE.md`, rule 3).
const TEST_REMOTE: &str = "https://example.invalid/org/repo.git";

/// Not a real image: the stub the session tests replay a fixture transcript
/// with.
const TEST_IMAGE: &str = "mars-session-stub:test";

/// How long a blocked transaction is given to prove it is blocked. Long enough
/// that a slow container has certainly started the second transaction, short
/// enough not to drag the suite out.
const BLOCKED_FOR: Duration = Duration::from_millis(400);

/// How long an unblocked transaction is given to finish once the lock is free.
const UNBLOCKED_WITHIN: Duration = Duration::from_secs(10);

fn new_project(name: &str) -> NewProject {
    NewProject {
        id: Uuid::new_v4(),
        name: ProjectName::parse(name).expect("the test name is valid"),
        remote_url: RemoteUrl::parse(TEST_REMOTE).expect("the test remote is valid"),
        default_branch: None,
        created_by: Some(SEEDED_ADMIN),
        max_attempts: MaxAttempts::default(),
    }
}

/// Insert `project` in its own committed transaction.
async fn insert(pool: &PgPool, project: &NewProject) -> Project {
    let repository = ProjectRepository::new(pool);
    let mut tx = pool.begin().await.expect("a transaction begins");
    let inserted = repository
        .insert(&mut tx, project)
        .await
        .expect("the project inserts");
    tx.commit().await.expect("the transaction commits");

    inserted
}

/// A committed project to hang shared directories, profiles and locks off.
async fn seeded_project(pool: &PgPool) -> Project {
    let mut new = new_project("mars");
    new.default_branch = Some(BranchName::parse("main").unwrap());
    insert(pool, &new).await
}

fn new_profile(project_id: Uuid, name: &str) -> NewAgentProfile {
    NewAgentProfile::new(project_id, name, TEST_IMAGE).expect("the test profile is valid")
}

/// Insert `profile` in its own committed transaction.
async fn insert_profile(pool: &PgPool, profile: &NewAgentProfile) -> Uuid {
    let repository = ProjectRepository::new(pool);
    let mut tx = pool.begin().await.unwrap();
    let inserted = repository
        .insert_profile(&mut tx, profile)
        .await
        .expect("the profile inserts");
    tx.commit().await.unwrap();

    inserted.id
}

/// Seed a session row directly.
///
/// `SessionRepository` belongs to another task; this test only needs a row
/// that makes `sessions_profile_id_fkey` bite, so it writes the minimum set of
/// columns itself. The token hash is an obviously fake constant, never a
/// credential (`CLAUDE.md`, rule 3).
async fn seed_session(pool: &PgPool, project_id: Uuid, profile_id: Uuid) -> Uuid {
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

    id
}

/// Not key material: an obviously fake stand-in for the four encrypted
/// columns of a secret this file only ever asks `EXISTS` about (`CLAUDE.md`,
/// rule 3).
fn fake_encrypted_value() -> EncryptedValue {
    EncryptedValue {
        ciphertext: b"fake-ciphertext".to_vec(),
        nonce: b"fake-nonce".to_vec(),
        data_key_wrapped: b"fake-wrapped-data-key".to_vec(),
        data_key_nonce: b"fake-wrap-nonce".to_vec(),
        key_version: 1,
    }
}

/// Move a seeded session to `state` directly.
///
/// `SessionRepository::set_state` would do it, but it also writes the event
/// and the notification that belong to a real transition; this file only needs
/// the column to hold a value so that the live count has something to skip.
async fn set_session_state(pool: &PgPool, id: Uuid, state: &str) {
    sqlx::query("UPDATE sessions SET state = $2::session_state WHERE id = $1")
        .bind(id)
        .bind(state)
        .execute(pool)
        .await
        .expect("the session state updates");
}

fn assert_conflict(error: Error, message: &str) {
    assert_eq!(
        error.status(),
        StatusCode::CONFLICT,
        "not a conflict: {error}"
    );
    assert_eq!(error.to_string(), message);
}

#[tokio::test]
async fn a_project_survives_an_insert_find_list_update_delete_round_trip() {
    let (_postgres, pool) = common::db::test_pool().await;
    let repository = ProjectRepository::new(&pool);

    let new = new_project("  mars  ");
    let inserted = insert(&pool, &new).await;

    assert_eq!(inserted.id, new.id);
    // The name was trimmed by the model, not by the repository.
    assert_eq!(inserted.name, "mars");
    assert_eq!(inserted.remote_url, TEST_REMOTE);
    assert_eq!(inserted.created_by, Some(SEEDED_ADMIN));
    assert_eq!(inserted.max_attempts, 3);
    // Column defaults the repository deliberately does not set.
    assert_eq!(inserted.status, ProjectStatus::Cloning);
    assert_eq!(inserted.status_message, None);
    assert_eq!(inserted.default_branch, None);
    assert_eq!(inserted.last_fetched_at, None);
    assert_eq!(inserted.next_task_number, 1);

    assert_eq!(
        repository.find(new.id).await.unwrap().as_ref(),
        Some(&inserted)
    );
    assert!(repository.find(Uuid::new_v4()).await.unwrap().is_none());

    // Listing is by name, not by insertion order.
    let other = insert(&pool, &new_project("apollo")).await;
    let listed = repository.list().await.unwrap();
    assert_eq!(
        listed.iter().map(|project| project.id).collect::<Vec<_>>(),
        [other.id, inserted.id]
    );

    let update = ProjectUpdate {
        name: Some(ProjectName::parse("mars-2").unwrap()),
        default_branch: Some(BranchName::parse("main").unwrap()),
        max_attempts: Some(MaxAttempts::parse(7).unwrap()),
    };
    let mut tx = pool.begin().await.unwrap();
    let updated = repository
        .update(&mut tx, new.id, &update)
        .await
        .unwrap()
        .expect("the project exists");
    tx.commit().await.unwrap();

    assert_eq!(updated.name, "mars-2");
    assert_eq!(updated.default_branch.as_deref(), Some("main"));
    assert_eq!(updated.max_attempts, 7);
    // Untouched fields keep their values, and `updated_at` moved.
    assert_eq!(updated.remote_url, TEST_REMOTE);
    assert_eq!(updated.created_at, inserted.created_at);
    assert!(updated.updated_at > inserted.updated_at);

    // An empty update refreshes `updated_at` and changes nothing else; in
    // particular `COALESCE` does not clear the branch it was not given.
    let mut tx = pool.begin().await.unwrap();
    let untouched = repository
        .update(&mut tx, new.id, &ProjectUpdate::default())
        .await
        .unwrap()
        .expect("the project exists");
    tx.commit().await.unwrap();

    assert_eq!(untouched.name, "mars-2");
    assert_eq!(untouched.default_branch.as_deref(), Some("main"));
    assert_eq!(untouched.max_attempts, 7);

    // An update of a project that does not exist is `None`, not an error.
    let mut tx = pool.begin().await.unwrap();
    assert!(
        repository
            .update(&mut tx, Uuid::new_v4(), &update)
            .await
            .unwrap()
            .is_none()
    );
    tx.commit().await.unwrap();

    let mut tx = pool.begin().await.unwrap();
    assert!(repository.delete(&mut tx, new.id).await.unwrap());
    assert!(!repository.delete(&mut tx, new.id).await.unwrap());
    tx.commit().await.unwrap();

    assert!(repository.find(new.id).await.unwrap().is_none());
    assert_eq!(repository.list().await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_duplicate_project_name_is_a_conflict() {
    let (_postgres, pool) = common::db::test_pool().await;
    let repository = ProjectRepository::new(&pool);

    let first = new_project("mars");
    insert(&pool, &first).await;

    let mut tx = pool.begin().await.unwrap();
    let error = repository
        .insert(&mut tx, &new_project("mars"))
        .await
        .expect_err("the name is taken");
    assert_conflict(error, "project name already taken");
    tx.rollback().await.unwrap();

    // And through the update path, which shares the mapping.
    let second = insert(&pool, &new_project("apollo")).await;
    let mut tx = pool.begin().await.unwrap();
    let error = repository
        .update(
            &mut tx,
            second.id,
            &ProjectUpdate {
                name: Some(ProjectName::parse("mars").unwrap()),
                ..ProjectUpdate::default()
            },
        )
        .await
        .expect_err("the name is taken");
    assert_conflict(error, "project name already taken");
}

#[tokio::test]
async fn the_clone_job_records_status_and_fetches() {
    let (_postgres, pool) = common::db::test_pool().await;
    let repository = ProjectRepository::new(&pool);

    let new = new_project("mars");
    let inserted = insert(&pool, &new).await;

    // A failed clone: the reason is stored with the status.
    let mut tx = pool.begin().await.unwrap();
    let failed = repository
        .set_status(
            &mut tx,
            new.id,
            ProjectStatus::Error,
            Some("remote HEAD is unborn"),
        )
        .await
        .unwrap()
        .expect("the project exists");
    tx.commit().await.unwrap();

    assert_eq!(failed.status, ProjectStatus::Error);
    assert_eq!(
        failed.status_message.as_deref(),
        Some("remote HEAD is unborn")
    );
    assert!(failed.updated_at > inserted.updated_at);

    // Becoming ready without a default branch is refused by the table CHECK,
    // and that is a conflict the caller can act on, not an internal error.
    let mut tx = pool.begin().await.unwrap();
    let error = repository
        .set_status(&mut tx, new.id, ProjectStatus::Ready, None)
        .await
        .expect_err("a ready project needs a default branch");
    assert_conflict(error, "default branch unknown");
    tx.rollback().await.unwrap();

    // With the branch discovered, the same call succeeds and clears the stale
    // failure message.
    let mut tx = pool.begin().await.unwrap();
    repository
        .update(
            &mut tx,
            new.id,
            &ProjectUpdate {
                default_branch: Some(BranchName::parse("main").unwrap()),
                ..ProjectUpdate::default()
            },
        )
        .await
        .unwrap();
    let ready = repository
        .set_status(&mut tx, new.id, ProjectStatus::Ready, None)
        .await
        .unwrap()
        .expect("the project exists");
    tx.commit().await.unwrap();

    assert_eq!(ready.status, ProjectStatus::Ready);
    assert_eq!(ready.status_message, None);

    let mut tx = pool.begin().await.unwrap();
    let fetched = repository
        .set_last_fetched_at(&mut tx, new.id)
        .await
        .unwrap()
        .expect("the project exists");
    tx.commit().await.unwrap();

    assert!(fetched.last_fetched_at.is_some());
    assert!(fetched.updated_at > ready.updated_at);

    // Both writes report a missing project as `None`.
    let mut tx = pool.begin().await.unwrap();
    assert!(
        repository
            .set_status(&mut tx, Uuid::new_v4(), ProjectStatus::Ready, None)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        repository
            .set_last_fetched_at(&mut tx, Uuid::new_v4())
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn locking_an_unknown_project_is_not_found() {
    let (_postgres, pool) = common::db::test_pool().await;
    let repository = ProjectRepository::new(&pool);

    let mut tx = pool.begin().await.unwrap();
    let error = repository
        .lock_project(&mut tx, Uuid::new_v4())
        .await
        .expect_err("there is no such project");
    assert_eq!(error.status(), StatusCode::NOT_FOUND);

    let error = repository
        .allocate_task_number(&mut tx, Uuid::new_v4())
        .await
        .expect_err("there is no such project");
    assert_eq!(error.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_project_lock_serialises_task_numbering() {
    // Two connections at once, plus the ones the helpers take.
    let (_postgres, pool) = common::db::test_pool_with(4).await;
    let repository = ProjectRepository::new(&pool);
    let project = seeded_project(&pool).await;

    // The first transaction takes the lock and its number, and keeps both.
    let mut first = pool.begin().await.unwrap();
    repository
        .lock_project(&mut first, project.id)
        .await
        .unwrap();
    let first_number = repository
        .allocate_task_number(&mut first, project.id)
        .await
        .unwrap();
    assert_eq!(first_number, 1);

    // The second does the same thing from another connection. It must wait at
    // the lock, before its own allocation.
    let second_pool = pool.clone();
    let project_id = project.id;
    let second = tokio::spawn(async move {
        let repository = ProjectRepository::new(&second_pool);
        let mut tx = second_pool.begin().await.unwrap();
        repository.lock_project(&mut tx, project_id).await.unwrap();
        let number = repository
            .allocate_task_number(&mut tx, project_id)
            .await
            .unwrap();
        tx.commit().await.unwrap();

        number
    });

    sleep(BLOCKED_FOR).await;
    assert!(
        !second.is_finished(),
        "the second transaction did not wait for the project lock"
    );

    first.commit().await.unwrap();

    let second_number = timeout(UNBLOCKED_WITHIN, second)
        .await
        .expect("the second transaction proceeds once the lock is free")
        .expect("the second transaction does not panic");

    // Successive numbers, no duplicate, and the counter is left pointing at
    // the next one.
    assert_eq!(second_number, 2);
    let after = ProjectRepository::new(&pool)
        .find(project.id)
        .await
        .unwrap()
        .expect("the project exists");
    assert_eq!(after.next_task_number, 3);
    // Allocating a number is not an edit of the project.
    assert_eq!(after.updated_at, project.updated_at);
}

#[tokio::test]
async fn a_project_reports_whether_it_has_a_git_credential() {
    let (_postgres, pool) = common::db::test_pool().await;
    let repository = ProjectRepository::new(&pool);

    let inserted = insert(&pool, &new_project("mars")).await;
    // The row is written by the secrets manager after the project exists, so
    // a fresh project has none.
    assert!(!inserted.has_credential);
    assert!(
        !repository
            .find(inserted.id)
            .await
            .unwrap()
            .unwrap()
            .has_credential
    );

    let secrets = SecretRepository::new(&pool);
    let mut tx = pool.begin().await.unwrap();
    let mut credential = NewSecret::new(
        ScopeRef::project(inserted.id),
        SecretName::parse(GIT_CREDENTIAL_NAME).unwrap(),
        fake_encrypted_value(),
    );
    credential.orchestrator_only = true;
    secrets.insert(&mut tx, &credential).await.unwrap();
    tx.commit().await.unwrap();

    // Every read computes it, not only `find`.
    assert!(
        repository
            .find(inserted.id)
            .await
            .unwrap()
            .unwrap()
            .has_credential
    );
    let listed = repository.list().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert!(listed[0].has_credential);

    let mut tx = pool.begin().await.unwrap();
    assert!(
        repository
            .update(&mut tx, inserted.id, &ProjectUpdate::default())
            .await
            .unwrap()
            .unwrap()
            .has_credential
    );
    assert!(
        repository
            .set_status(&mut tx, inserted.id, ProjectStatus::Error, Some("nope"))
            .await
            .unwrap()
            .unwrap()
            .has_credential
    );
    assert!(
        repository
            .mark_cloning_from_error(&mut tx, inserted.id)
            .await
            .unwrap()
            .unwrap()
            .has_credential
    );
    tx.commit().await.unwrap();

    // A secret of the same name in another scope, and another name in this
    // one, are both somebody else's.
    let other = insert(&pool, &new_project("apollo")).await;
    let mut tx = pool.begin().await.unwrap();
    secrets
        .insert(
            &mut tx,
            &NewSecret::new(
                ScopeRef::project(other.id),
                SecretName::parse("NPM_TOKEN").unwrap(),
                fake_encrypted_value(),
            ),
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert!(
        !repository
            .find(other.id)
            .await
            .unwrap()
            .unwrap()
            .has_credential
    );
}

#[tokio::test]
async fn the_clone_transitions_are_guarded_by_the_current_status() {
    let (_postgres, pool) = common::db::test_pool().await;
    let repository = ProjectRepository::new(&pool);

    let main = BranchName::parse("main").unwrap();
    let inserted = insert(&pool, &new_project("mars")).await;

    // A project that is still `cloning` has nothing to retry.
    let mut tx = pool.begin().await.unwrap();
    assert!(
        repository
            .mark_cloning_from_error(&mut tx, inserted.id)
            .await
            .unwrap()
            .is_none()
    );

    // The clone finishes once. The second call is the same job arriving after
    // a restart, and it must change nothing.
    assert_eq!(
        repository
            .mark_ready(&mut tx, inserted.id, &main)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        repository
            .mark_ready(&mut tx, inserted.id, &main)
            .await
            .unwrap(),
        0
    );
    // And a failure that arrives after the success is refused just as flatly.
    assert_eq!(
        repository
            .mark_error(&mut tx, inserted.id, "remote HEAD is unborn")
            .await
            .unwrap(),
        0
    );
    tx.commit().await.unwrap();

    let ready = repository.find(inserted.id).await.unwrap().unwrap();
    assert_eq!(ready.status, ProjectStatus::Ready);
    assert_eq!(ready.default_branch.as_deref(), Some("main"));
    assert_eq!(ready.status_message, None);
    // The clone that just finished is also a fetch.
    assert!(ready.last_fetched_at.is_some());
    assert!(ready.updated_at > inserted.updated_at);

    // An unknown project matches nothing either, and is not an error.
    let mut tx = pool.begin().await.unwrap();
    assert_eq!(
        repository
            .mark_ready(&mut tx, Uuid::new_v4(), &main)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        repository
            .mark_error(&mut tx, Uuid::new_v4(), "gone")
            .await
            .unwrap(),
        0
    );

    // A second project takes the failing path and is then retried.
    let failed = insert(&pool, &new_project("apollo")).await;
    assert_eq!(
        repository
            .mark_error(&mut tx, failed.id, "remote HEAD is unborn")
            .await
            .unwrap(),
        1
    );
    tx.commit().await.unwrap();

    let stored = repository.find(failed.id).await.unwrap().unwrap();
    assert_eq!(stored.status, ProjectStatus::Error);
    assert_eq!(
        stored.status_message.as_deref(),
        Some("remote HEAD is unborn")
    );

    let mut tx = pool.begin().await.unwrap();
    let retried = repository
        .mark_cloning_from_error(&mut tx, failed.id)
        .await
        .unwrap()
        .expect("the project was in error");
    // A second retry finds it `cloning` already.
    assert!(
        repository
            .mark_cloning_from_error(&mut tx, failed.id)
            .await
            .unwrap()
            .is_none()
    );
    // And the reopened clone can now finish, from `cloning`.
    assert_eq!(
        repository
            .mark_ready(&mut tx, failed.id, &main)
            .await
            .unwrap(),
        1
    );
    tx.commit().await.unwrap();

    assert_eq!(retried.status, ProjectStatus::Cloning);
    assert_eq!(retried.status_message, None);
    assert!(retried.updated_at > stored.updated_at);
}

#[tokio::test]
async fn only_running_and_creating_sessions_count_as_live() {
    let (_postgres, pool) = common::db::test_pool().await;
    let sessions = SessionRepository::new(&pool);
    let project = seeded_project(&pool).await;
    let other = insert(&pool, &new_project("apollo")).await;
    let profile = insert_profile(&pool, &new_profile(project.id, "default")).await;
    let other_profile = insert_profile(&pool, &new_profile(other.id, "default")).await;

    let mut tx = pool.begin().await.unwrap();
    assert_eq!(
        sessions
            .count_live_for_project(&mut tx, project.id)
            .await
            .unwrap(),
        0
    );
    tx.commit().await.unwrap();

    // One of each state, plus one on another project that must never be
    // counted here.
    let creating = seed_session(&pool, project.id, profile).await;
    let running = seed_session(&pool, project.id, profile).await;
    set_session_state(&pool, running, "running").await;
    for state in ["parked", "done", "failed"] {
        let id = seed_session(&pool, project.id, profile).await;
        set_session_state(&pool, id, state).await;
    }
    let elsewhere = seed_session(&pool, other.id, other_profile).await;

    let mut tx = pool.begin().await.unwrap();
    assert_eq!(
        sessions
            .count_live_for_project(&mut tx, project.id)
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        sessions
            .count_live_for_project(&mut tx, other.id)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sessions
            .count_live_for_project(&mut tx, Uuid::new_v4())
            .await
            .unwrap(),
        0
    );
    tx.commit().await.unwrap();

    set_session_state(&pool, creating, "done").await;
    set_session_state(&pool, running, "failed").await;
    set_session_state(&pool, elsewhere, "done").await;

    let mut tx = pool.begin().await.unwrap();
    assert_eq!(
        sessions
            .count_live_for_project(&mut tx, project.id)
            .await
            .unwrap(),
        0
    );
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn a_shared_directory_survives_an_insert_list_delete_round_trip() {
    let (_postgres, pool) = common::db::test_pool().await;
    let repository = ProjectRepository::new(&pool);
    let project = seeded_project(&pool).await;

    let target = NewSharedDir::new("target", "/session/work/target").unwrap();
    let cache = NewSharedDir::new("cache", "/cache").unwrap();

    let mut tx = pool.begin().await.unwrap();
    let inserted = repository
        .insert_shared_dir(&mut tx, project.id, &target)
        .await
        .unwrap();
    repository
        .insert_shared_dir(&mut tx, project.id, &cache)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(inserted.project_id, project.id);
    assert_eq!(inserted.name, "target");
    assert_eq!(inserted.container_path, "/session/work/target");

    // By name, and scoped to the project.
    let listed = repository.list_shared_dirs(project.id).await.unwrap();
    assert_eq!(
        listed
            .iter()
            .map(|dir| dir.name.as_str())
            .collect::<Vec<_>>(),
        ["cache", "target"]
    );
    assert!(
        repository
            .list_shared_dirs(Uuid::new_v4())
            .await
            .unwrap()
            .is_empty()
    );

    let mut tx = pool.begin().await.unwrap();
    assert!(
        repository
            .delete_shared_dir(&mut tx, project.id, "target")
            .await
            .unwrap()
    );
    assert!(
        !repository
            .delete_shared_dir(&mut tx, project.id, "target")
            .await
            .unwrap()
    );
    // Another project's directory is out of scope, not deleted.
    assert!(
        !repository
            .delete_shared_dir(&mut tx, Uuid::new_v4(), "cache")
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();

    assert_eq!(
        repository.list_shared_dirs(project.id).await.unwrap().len(),
        1
    );
}

#[tokio::test]
async fn a_duplicate_shared_directory_name_or_path_is_a_conflict() {
    let (_postgres, pool) = common::db::test_pool().await;
    let repository = ProjectRepository::new(&pool);
    let project = seeded_project(&pool).await;

    let mut tx = pool.begin().await.unwrap();
    repository
        .insert_shared_dir(
            &mut tx,
            project.id,
            &NewSharedDir::new("target", "/session/work/target").unwrap(),
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let mut tx = pool.begin().await.unwrap();
    let error = repository
        .insert_shared_dir(
            &mut tx,
            project.id,
            &NewSharedDir::new("target", "/elsewhere").unwrap(),
        )
        .await
        .expect_err("the name is used");
    assert_conflict(error, "shared directory name already used");
    tx.rollback().await.unwrap();

    let mut tx = pool.begin().await.unwrap();
    let error = repository
        .insert_shared_dir(
            &mut tx,
            project.id,
            &NewSharedDir::new("other", "/session/work/target").unwrap(),
        )
        .await
        .expect_err("the path is used");
    assert_conflict(error, "container path already used");
    tx.rollback().await.unwrap();

    // Another project may use both the same name and the same path.
    let other = insert(&pool, &new_project("apollo")).await;
    let mut tx = pool.begin().await.unwrap();
    repository
        .insert_shared_dir(
            &mut tx,
            other.id,
            &NewSharedDir::new("target", "/session/work/target").unwrap(),
        )
        .await
        .expect("a different project is a different scope");
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn a_profile_survives_an_insert_find_list_update_delete_round_trip() {
    let (_postgres, pool) = common::db::test_pool().await;
    let repository = ProjectRepository::new(&pool);
    let project = seeded_project(&pool).await;

    let mut new = new_profile(project.id, "default");
    new.is_default = true;
    new.mcp_tools = vec!["ready".to_string(), "claim".to_string()];
    new.secrets = vec!["NPM_TOKEN".to_string()];
    let inserted = {
        let mut tx = pool.begin().await.unwrap();
        let inserted = repository.insert_profile(&mut tx, &new).await.unwrap();
        tx.commit().await.unwrap();
        inserted
    };

    assert_eq!(inserted.id, new.id);
    assert_eq!(inserted.project_id, project.id);
    assert_eq!(inserted.name, "default");
    assert_eq!(inserted.kind, ProfileKind::Conversational);
    assert_eq!(inserted.backend, AgentBackend::Claude);
    assert_eq!(inserted.permission_mode, "bypass");
    assert_eq!(inserted.image, TEST_IMAGE);
    assert_eq!(inserted.mcp_tools, ["ready", "claim"]);
    assert_eq!(inserted.secrets, ["NPM_TOKEN"]);
    assert_eq!(inserted.idle_timeout_secs, 1800);
    assert!(inserted.is_default);
    // Resolved from the kind, because the caller set nothing.
    assert!(inserted.partial_messages);

    assert_eq!(
        repository
            .find_profile(project.id, new.id)
            .await
            .unwrap()
            .as_ref(),
        Some(&inserted)
    );
    assert_eq!(
        repository
            .find_default_profile(project.id)
            .await
            .unwrap()
            .as_ref(),
        Some(&inserted)
    );
    // The project is part of the scope, not a filter applied afterwards.
    assert!(
        repository
            .find_profile(Uuid::new_v4(), new.id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        repository
            .find_default_profile(Uuid::new_v4())
            .await
            .unwrap()
            .is_none()
    );

    // An ephemeral profile defaults the other way, and listing is by name.
    // `new` already resolved the value from the conversational default, so
    // changing the kind means asking for the default again.
    let mut ephemeral = new_profile(project.id, "implementer");
    ephemeral.kind = ProfileKind::Ephemeral;
    ephemeral.partial_messages = None;
    ephemeral.validate().unwrap();
    assert_eq!(ephemeral.partial_messages, Some(false));
    insert_profile(&pool, &ephemeral).await;

    let listed = repository.list_profiles(project.id).await.unwrap();
    assert_eq!(
        listed
            .iter()
            .map(|profile| profile.name.as_str())
            .collect::<Vec<_>>(),
        ["default", "implementer"]
    );
    assert!(!listed[1].partial_messages);

    let mut update = ProfileUpdate::from(&inserted);
    update.name = "planner".to_string();
    update.model = Some("a-model".to_string());
    update.system_prompt = Some("plan carefully".to_string());
    update.runtime = Some("runsc".to_string());
    update.idle_timeout_secs = 60;
    update.mcp_tools = Vec::new();
    update.validate().unwrap();

    let mut tx = pool.begin().await.unwrap();
    let updated = repository
        .update_profile(&mut tx, project.id, new.id, &update)
        .await
        .unwrap()
        .expect("the profile exists");
    tx.commit().await.unwrap();

    assert_eq!(updated.name, "planner");
    assert_eq!(updated.model.as_deref(), Some("a-model"));
    assert_eq!(updated.system_prompt.as_deref(), Some("plan carefully"));
    assert_eq!(updated.runtime.as_deref(), Some("runsc"));
    assert_eq!(updated.idle_timeout_secs, 60);
    assert!(updated.mcp_tools.is_empty());
    assert_eq!(updated.secrets, ["NPM_TOKEN"]);
    assert_eq!(updated.created_at, inserted.created_at);
    assert!(updated.updated_at > inserted.updated_at);

    // A full replacement really does clear a nullable column.
    let mut cleared = ProfileUpdate::from(&updated);
    cleared.model = None;
    cleared.runtime = None;
    let mut tx = pool.begin().await.unwrap();
    let cleared = repository
        .update_profile(&mut tx, project.id, new.id, &cleared)
        .await
        .unwrap()
        .expect("the profile exists");
    tx.commit().await.unwrap();
    assert_eq!(cleared.model, None);
    assert_eq!(cleared.runtime, None);

    // Out of scope, so nothing is updated or deleted.
    let mut tx = pool.begin().await.unwrap();
    assert!(
        repository
            .update_profile(&mut tx, Uuid::new_v4(), new.id, &update)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        !repository
            .delete_profile(&mut tx, Uuid::new_v4(), new.id)
            .await
            .unwrap()
    );
    assert!(
        repository
            .delete_profile(&mut tx, project.id, new.id)
            .await
            .unwrap()
    );
    assert!(
        !repository
            .delete_profile(&mut tx, project.id, new.id)
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();

    assert_eq!(repository.list_profiles(project.id).await.unwrap().len(), 1);
    assert!(
        repository
            .find_default_profile(project.id)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn a_duplicate_profile_name_or_second_default_is_a_conflict() {
    let (_postgres, pool) = common::db::test_pool().await;
    let repository = ProjectRepository::new(&pool);
    let project = seeded_project(&pool).await;

    let mut first = new_profile(project.id, "default");
    first.is_default = true;
    insert_profile(&pool, &first).await;

    let mut tx = pool.begin().await.unwrap();
    let error = repository
        .insert_profile(&mut tx, &new_profile(project.id, "default"))
        .await
        .expect_err("the name is taken");
    assert_conflict(error, "profile name already taken");
    tx.rollback().await.unwrap();

    let mut second_default = new_profile(project.id, "planner");
    second_default.is_default = true;
    let mut tx = pool.begin().await.unwrap();
    let error = repository
        .insert_profile(&mut tx, &second_default)
        .await
        .expect_err("a project has one default profile");
    assert_conflict(error, "project already has a default profile");
    tx.rollback().await.unwrap();

    // The same two conflicts through the update path.
    let planner = insert_profile(&pool, &new_profile(project.id, "planner")).await;
    let mut tx = pool.begin().await.unwrap();
    let mut rename = ProfileUpdate::from(
        &repository
            .find_profile(project.id, planner)
            .await
            .unwrap()
            .unwrap(),
    );
    rename.name = "default".to_string();
    let error = repository
        .update_profile(&mut tx, project.id, planner, &rename)
        .await
        .expect_err("the name is taken");
    assert_conflict(error, "profile name already taken");
    tx.rollback().await.unwrap();

    let mut tx = pool.begin().await.unwrap();
    let mut promote = ProfileUpdate::from(
        &repository
            .find_profile(project.id, planner)
            .await
            .unwrap()
            .unwrap(),
    );
    promote.is_default = true;
    let error = repository
        .update_profile(&mut tx, project.id, planner, &promote)
        .await
        .expect_err("a project has one default profile");
    assert_conflict(error, "project already has a default profile");
    tx.rollback().await.unwrap();

    // Another project has its own default slot and its own names.
    let other = insert(&pool, &new_project("apollo")).await;
    let mut other_default = new_profile(other.id, "default");
    other_default.is_default = true;
    insert_profile(&pool, &other_default).await;
}

#[tokio::test]
async fn deleting_a_profile_with_sessions_is_a_conflict() {
    let (_postgres, pool) = common::db::test_pool().await;
    let repository = ProjectRepository::new(&pool);
    let project = seeded_project(&pool).await;

    let profile = insert_profile(&pool, &new_profile(project.id, "default")).await;
    seed_session(&pool, project.id, profile).await;

    let mut tx = pool.begin().await.unwrap();
    let error = repository
        .delete_profile(&mut tx, project.id, profile)
        .await
        .expect_err("the profile still has a session");
    assert_conflict(error, "profile has sessions");
    tx.rollback().await.unwrap();

    assert!(
        repository
            .find_profile(project.id, profile)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn deleting_a_project_cascades_its_configuration() {
    let (_postgres, pool) = common::db::test_pool().await;
    let repository = ProjectRepository::new(&pool);
    let project = seeded_project(&pool).await;

    insert_profile(&pool, &new_profile(project.id, "default")).await;
    let mut tx = pool.begin().await.unwrap();
    repository
        .insert_shared_dir(
            &mut tx,
            project.id,
            &NewSharedDir::new("target", "/session/work/target").unwrap(),
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let mut tx = pool.begin().await.unwrap();
    assert!(repository.delete(&mut tx, project.id).await.unwrap());
    tx.commit().await.unwrap();

    assert!(
        repository
            .list_profiles(project.id)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        repository
            .list_shared_dirs(project.id)
            .await
            .unwrap()
            .is_empty()
    );
}
