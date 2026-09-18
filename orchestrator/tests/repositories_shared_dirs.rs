//! The shared-directory half of `ProjectRepository` against a real Postgres
//! (`CLAUDE.md`, "Testing expectations").
//!
//! `project_shared_dirs` has no identity of its own: the rows are configuration
//! of a project, so they live in `ProjectRepository` alongside the project and
//! its profiles (`orchestrator/src/repositories/projects.rs`, module doc). What
//! is asserted here is the part of that contract the launcher depends on — the
//! round trip, the listing order, and the three ways the database answers back:
//! the primary key on `(project_id, name)`, the unique index on
//! `(project_id, container_path)` and the foreign key to `projects`. Each must
//! be the documented 409 or 404 (`SPEC.md`, "Shared directories"), never a 500,
//! because each is something a caller can provoke with a well-formed request.
//!
//! The mount order a launcher needs is a property of the model, not of the
//! table, and is asserted in `src/models/shared_dir.rs`; here it is only shown
//! that a listing can be handed to it.
//!
//! Needs a container engine; see `tests/common/db.rs`.

mod common;

use axum::http::StatusCode;
use mars_orchestrator::models::{
    BranchName, MaxAttempts, NewProject, NewSharedDir, Project, ProjectName, RemoteUrl, SharedDir,
};
use mars_orchestrator::prelude::*;
use mars_orchestrator::repositories::ProjectRepository;
use sqlx::PgPool;
use uuid::Uuid;

/// The administrator the `users` migration seeds (`docs/data-model.md`).
const SEEDED_ADMIN: Uuid = Uuid::from_u128(1);

/// Not a real remote: `.invalid` can never resolve (`CLAUDE.md`, rule 3).
const TEST_REMOTE: &str = "https://example.invalid/org/repo.git";

/// A committed project to hang shared directories off.
async fn seeded_project(pool: &PgPool, name: &str) -> Project {
    let new = NewProject {
        id: Uuid::new_v4(),
        name: ProjectName::parse(name).expect("the test name is valid"),
        remote_url: RemoteUrl::parse(TEST_REMOTE).expect("the test remote is valid"),
        default_branch: Some(BranchName::parse("main").expect("the test branch is valid")),
        created_by: Some(SEEDED_ADMIN),
        max_attempts: MaxAttempts::default(),
    };

    let repository = ProjectRepository::new(pool);
    let mut tx = pool.begin().await.expect("a transaction begins");
    let inserted = repository
        .insert(&mut tx, &new)
        .await
        .expect("the project inserts");
    tx.commit().await.expect("the transaction commits");

    inserted
}

/// Insert one shared directory in a committed transaction of its own.
async fn insert_dir(pool: &PgPool, project_id: Uuid, name: &str, path: &str) -> Result<SharedDir> {
    let repository = ProjectRepository::new(pool);
    let dir = NewSharedDir::new(name, path).expect("the test entry is valid");
    let mut tx = pool.begin().await.expect("a transaction begins");
    let inserted = repository
        .insert_shared_dir(&mut tx, project_id, &dir)
        .await;
    match inserted {
        Ok(row) => {
            tx.commit().await.expect("the transaction commits");
            Ok(row)
        }
        Err(err) => {
            // A failed statement poisons the transaction; roll it back rather
            // than leave the connection in it.
            tx.rollback().await.expect("the transaction rolls back");
            Err(err)
        }
    }
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
async fn a_shared_directory_survives_an_insert_find_list_delete_round_trip() {
    let (_postgres, pool) = common::db::test_pool().await;
    let repository = ProjectRepository::new(&pool);
    let project = seeded_project(&pool, "mars").await;

    let inserted = insert_dir(&pool, project.id, "target", "/session/work/target")
        .await
        .expect("the entry inserts");
    assert_eq!(inserted.project_id, project.id);
    assert_eq!(inserted.name, "target");
    assert_eq!(inserted.container_path, "/session/work/target");

    insert_dir(&pool, project.id, "npm-cache", "/session/home/.npm")
        .await
        .expect("the entry inserts");

    // Found by name, inside its project and nowhere else.
    let found = repository
        .find_shared_dir(project.id, "target")
        .await
        .unwrap()
        .expect("the entry was inserted");
    assert_eq!(found, inserted);
    assert!(
        repository
            .find_shared_dir(project.id, "absent")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        repository
            .find_shared_dir(Uuid::new_v4(), "target")
            .await
            .unwrap()
            .is_none()
    );

    // Listed by name, and scoped to the project.
    let mut listed = repository.list_shared_dirs(project.id).await.unwrap();
    assert_eq!(
        listed
            .iter()
            .map(|dir| dir.name.as_str())
            .collect::<Vec<_>>(),
        ["npm-cache", "target"]
    );
    assert!(
        repository
            .list_shared_dirs(Uuid::new_v4())
            .await
            .unwrap()
            .is_empty()
    );

    // What the launcher does with that listing: parents before children.
    SharedDir::sort_for_mount(&mut listed);
    assert_eq!(
        listed
            .iter()
            .map(|dir| dir.container_path.as_str())
            .collect::<Vec<_>>(),
        ["/session/home/.npm", "/session/work/target"]
    );

    let mut tx = pool.begin().await.unwrap();
    assert!(
        repository
            .delete_shared_dir(&mut tx, project.id, "target")
            .await
            .unwrap()
    );
    // Gone, so a second delete matches nothing.
    assert!(
        !repository
            .delete_shared_dir(&mut tx, project.id, "target")
            .await
            .unwrap()
    );
    // Another project's entry is out of scope, not deleted.
    assert!(
        !repository
            .delete_shared_dir(&mut tx, Uuid::new_v4(), "npm-cache")
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();

    assert!(
        repository
            .find_shared_dir(project.id, "target")
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        repository.list_shared_dirs(project.id).await.unwrap().len(),
        1
    );
}

#[tokio::test]
async fn a_used_name_and_a_used_path_are_two_different_conflicts() {
    let (_postgres, pool) = common::db::test_pool().await;
    let project = seeded_project(&pool, "mars").await;

    insert_dir(&pool, project.id, "target", "/session/work/target")
        .await
        .expect("the entry inserts");

    let error = insert_dir(&pool, project.id, "target", "/elsewhere")
        .await
        .expect_err("the name is used");
    assert_conflict(error, "shared directory name already used");

    let error = insert_dir(&pool, project.id, "other", "/session/work/target")
        .await
        .expect_err("the path is used");
    assert_conflict(error, "container path already used");

    // Both are per project: the same pair is free in another one.
    let other = seeded_project(&pool, "apollo").await;
    insert_dir(&pool, other.id, "target", "/session/work/target")
        .await
        .expect("a different project is a different scope");
}

#[tokio::test]
async fn a_shared_directory_for_an_unknown_project_is_not_found() {
    let (_postgres, pool) = common::db::test_pool().await;

    let error = insert_dir(&pool, Uuid::new_v4(), "target", "/session/work/target")
        .await
        .expect_err("the project does not exist");
    assert_eq!(
        error.status(),
        StatusCode::NOT_FOUND,
        "not a 404: {error} ({error:?})"
    );
}

#[tokio::test]
async fn deleting_a_project_takes_its_shared_directories_with_it() {
    let (_postgres, pool) = common::db::test_pool().await;
    let repository = ProjectRepository::new(&pool);
    let project = seeded_project(&pool, "mars").await;

    insert_dir(&pool, project.id, "target", "/session/work/target")
        .await
        .expect("the entry inserts");

    let mut tx = pool.begin().await.unwrap();
    assert!(repository.delete(&mut tx, project.id).await.unwrap());
    tx.commit().await.unwrap();

    assert!(
        repository
            .list_shared_dirs(project.id)
            .await
            .unwrap()
            .is_empty()
    );
}
