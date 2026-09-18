//! Creating a project against a real Postgres: the row, the seven default
//! task states, the `default` profile serving `ready`, the `GIT_CREDENTIAL`
//! secret, and the rollback that leaves none of them behind (`SPEC.md`,
//! "Projects"; `docs/data-model.md`, `task_states`, `profile_states`,
//! `agent_profiles`, `secrets`).
//!
//! `projects::create_project` is called directly rather than through a route:
//! the endpoint is the next task's, and what is asserted here is the
//! transaction — which rows one call leaves behind, and that a failure leaves
//! none of them.
//!
//! Every credential here is an obviously fake stand-in for a PAT (rule 3).
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use common::TestApp;
use mars_orchestrator::models::{
    AgentBackend, ProfileKind, ProjectStatus, ScopeRef, Secret, SecretName, SecretScope,
    TaskStateKind,
};
use mars_orchestrator::prelude::*;
use mars_orchestrator::projects::{NewProjectRequest, create_project};
use mars_orchestrator::repositories::{ProjectRepository, SecretRepository, TaskRepository};
use mars_orchestrator::secrets::{GIT_CREDENTIAL_NAME, GitUseContext, project_git_credential};
use uuid::Uuid;

/// Not a real remote: `.invalid` can never resolve (rule 3).
const TEST_REMOTE: &str = "https://example.invalid/org/repo.git";

/// Not a real PAT: an obviously fake stand-in (rule 3).
const FAKE_CREDENTIAL: &str = "fake-git-credential-for-tests";

/// The image `TestApp`'s configuration names in `SESSION_IMAGE_DEFAULT`; the
/// seeded profile must land on this one and not on a literal of its own.
const TEST_IMAGE: &str = "mars-session-stub:test";

/// The request a user would send for a public repository.
fn request(name: &str) -> NewProjectRequest {
    NewProjectRequest {
        name: name.to_string(),
        remote_url: TEST_REMOTE.to_string(),
        default_branch: None,
        credential: None,
    }
}

/// The four unscoped row counts the rollback assertions are made of.
///
/// Deliberately unscoped: what a rolled-back creation must leave behind is no
/// orphan *anywhere*, not a count of one project's rows. Whole statements
/// rather than a table name interpolated into one, because `sqlx` accepts only
/// a literal.
const COUNT_PROJECTS: &str = "SELECT COUNT(*) FROM projects";
const COUNT_TASK_STATES: &str = "SELECT COUNT(*) FROM task_states";
const COUNT_PROFILES: &str = "SELECT COUNT(*) FROM agent_profiles";
const COUNT_SECRETS: &str = "SELECT COUNT(*) FROM secrets";

/// Run one of the counts above.
async fn count(pool: &PgPool, sql: &'static str) -> i64 {
    sqlx::query_scalar::<_, i64>(sql)
        .fetch_one(pool)
        .await
        .expect("the count runs")
}

/// The project's `GIT_CREDENTIAL` row, or `None`.
async fn credential_row(pool: &PgPool, project_id: Uuid) -> Option<Secret> {
    let name = SecretName::parse(GIT_CREDENTIAL_NAME).expect("the fixed name is valid");
    SecretRepository::new(pool)
        .find_by_name(&ScopeRef::project(project_id), &name)
        .await
        .expect("the lookup runs")
}

#[tokio::test]
async fn a_new_project_starts_cloning_with_no_branch_and_no_credential() {
    let app = TestApp::spawn().await;
    let user = app
        .insert_user("creator", "creator@example.com", false, false)
        .await;

    let project = create_project(&app.state, request("mars"), user.id)
        .await
        .expect("the project is created");

    assert_eq!(project.name, "mars");
    assert_eq!(project.remote_url, TEST_REMOTE);
    assert_eq!(project.status, ProjectStatus::Cloning);
    assert_eq!(project.default_branch, None);
    assert_eq!(project.status_message, None);
    assert_eq!(project.created_by, Some(user.id));
    assert_eq!(project.next_task_number, 1);
    assert!(!project.has_credential);

    // Committed, not just returned.
    let stored = ProjectRepository::new(&app.pool)
        .find(project.id)
        .await
        .expect("the lookup runs")
        .expect("the project is committed");
    assert_eq!(stored, project);
}

#[tokio::test]
async fn the_default_task_states_are_seeded_in_board_order() {
    let app = TestApp::spawn().await;
    let user = app
        .insert_user("creator", "creator@example.com", false, false)
        .await;

    let project = create_project(&app.state, request("mars"), user.id)
        .await
        .expect("the project is created");

    let states = TaskRepository::new(&app.pool)
        .list_states(project.id)
        .await
        .expect("the states are read");

    let seeded: Vec<(&str, TaskStateKind, i32)> = states
        .iter()
        .map(|state| (state.name.as_str(), state.kind, state.position))
        .collect();

    assert_eq!(
        seeded,
        vec![
            ("backlog", TaskStateKind::Queue, 0),
            ("ready", TaskStateKind::Queue, 1),
            ("review", TaskStateKind::Queue, 2),
            ("merge", TaskStateKind::Queue, 3),
            ("needs_human", TaskStateKind::Human, 4),
            ("done", TaskStateKind::Terminal, 5),
            ("cancelled", TaskStateKind::Terminal, 6),
        ]
    );
    assert!(states.iter().all(|state| state.project_id == project.id));
}

#[tokio::test]
async fn the_seeded_profile_is_the_default_one_and_serves_ready() {
    let app = TestApp::spawn().await;
    let user = app
        .insert_user("creator", "creator@example.com", false, false)
        .await;

    let project = create_project(&app.state, request("mars"), user.id)
        .await
        .expect("the project is created");

    let projects = ProjectRepository::new(&app.pool);
    let profiles = projects
        .list_profiles(project.id)
        .await
        .expect("the profiles are read");
    assert_eq!(profiles.len(), 1, "exactly one profile is seeded");

    let profile = &profiles[0];
    assert_eq!(profile.name, "default");
    assert_eq!(profile.kind, ProfileKind::Conversational);
    assert_eq!(profile.backend, AgentBackend::Claude);
    assert_eq!(profile.permission_mode, "bypass");
    // The configured image, not a literal of this test's own: the value comes
    // from `SESSION_IMAGE_DEFAULT` (`README.md`, "Configuration").
    assert_eq!(profile.image, app.state.config.session_image_default);
    assert_eq!(profile.image, TEST_IMAGE);
    assert_eq!(profile.model, None);
    assert_eq!(profile.system_prompt, None);
    assert_eq!(profile.runtime, None);
    assert!(profile.mcp_tools.is_empty());
    assert!(profile.secrets.is_empty());
    assert!(profile.partial_messages);
    assert_eq!(profile.idle_timeout_secs, 1800);
    assert!(profile.is_default);
    assert_eq!(profile.serves_states, vec!["ready".to_string()]);

    // The same row through the default-profile lookup every launch makes.
    let default = projects
        .find_default_profile(project.id)
        .await
        .expect("the lookup runs")
        .expect("the project has a default profile");
    assert_eq!(default.id, profile.id);
}

#[tokio::test]
async fn a_supplied_credential_becomes_the_orchestrator_only_git_credential_secret() {
    let app = TestApp::spawn().await;
    let user = app
        .insert_user("creator", "creator@example.com", false, false)
        .await;

    let project = create_project(
        &app.state,
        NewProjectRequest {
            credential: Some(FAKE_CREDENTIAL.to_string()),
            ..request("mars")
        },
        user.id,
    )
    .await
    .expect("the project is created");

    // Computed by the insert's own `EXISTS`, in the same transaction as the
    // secret: the returned row already knows the credential is there.
    assert!(project.has_credential);

    let row = credential_row(&app.pool, project.id)
        .await
        .expect("the credential row exists");
    assert_eq!(row.scope, SecretScope::Project);
    assert_eq!(row.scope_id, Some(project.id));
    assert_eq!(row.name, GIT_CREDENTIAL_NAME);
    assert!(row.orchestrator_only);
    assert_eq!(row.created_by, Some(user.id));

    // And it decrypts to what was supplied.
    let value = project_git_credential(
        &app.pool,
        &app.state.keyring,
        project.id,
        GitUseContext::System,
    )
    .await
    .expect("the credential is read")
    .expect("the project has one");
    assert_eq!(value.as_str(), FAKE_CREDENTIAL);
}

#[tokio::test]
async fn a_project_without_a_credential_stores_no_secret() {
    let app = TestApp::spawn().await;
    let user = app
        .insert_user("creator", "creator@example.com", false, false)
        .await;

    let project = create_project(&app.state, request("mars"), user.id)
        .await
        .expect("the project is created");

    assert!(!project.has_credential);
    assert!(credential_row(&app.pool, project.id).await.is_none());
    assert_eq!(count(&app.pool, COUNT_SECRETS).await, 0);
}

#[tokio::test]
async fn a_supplied_default_branch_is_stored_as_given() {
    let app = TestApp::spawn().await;
    let user = app
        .insert_user("creator", "creator@example.com", false, false)
        .await;

    let project = create_project(
        &app.state,
        NewProjectRequest {
            default_branch: Some("release/1.2".to_string()),
            ..request("mars")
        },
        user.id,
    )
    .await
    .expect("the project is created");

    // Stored as-is and still `cloning`: the clone job validates it against the
    // fetched heads (`SPEC.md`, "Projects").
    assert_eq!(project.default_branch.as_deref(), Some("release/1.2"));
    assert_eq!(project.status, ProjectStatus::Cloning);
}

#[tokio::test]
async fn a_malformed_field_is_rejected_before_anything_is_written() {
    let app = TestApp::spawn().await;
    let user = app
        .insert_user("creator", "creator@example.com", false, false)
        .await;

    let cases = [
        NewProjectRequest {
            name: "   ".to_string(),
            ..request("unused")
        },
        NewProjectRequest {
            remote_url: "http://example.invalid/org/repo.git".to_string(),
            ..request("mars")
        },
        NewProjectRequest {
            default_branch: Some("refs/heads/main".to_string()),
            ..request("mars")
        },
        NewProjectRequest {
            credential: Some("   ".to_string()),
            ..request("mars")
        },
    ];

    for case in cases {
        let error = create_project(&app.state, case, user.id)
            .await
            .expect_err("the request is rejected");

        assert_eq!(
            error.status(),
            axum::http::StatusCode::BAD_REQUEST,
            "{error}"
        );
    }

    // Validation runs before the transaction, so not one row was written.
    assert_eq!(count(&app.pool, COUNT_PROJECTS).await, 0);
    assert_eq!(count(&app.pool, COUNT_TASK_STATES).await, 0);
    assert_eq!(count(&app.pool, COUNT_PROFILES).await, 0);
    assert_eq!(count(&app.pool, COUNT_SECRETS).await, 0);
}

#[tokio::test]
async fn a_blank_credential_says_which_field_is_wrong() {
    let app = TestApp::spawn().await;
    let user = app
        .insert_user("creator", "creator@example.com", false, false)
        .await;

    let error = create_project(
        &app.state,
        NewProjectRequest {
            credential: Some("  ".to_string()),
            ..request("mars")
        },
        user.id,
    )
    .await
    .expect_err("a blank credential is rejected");

    assert_eq!(error.to_string(), "credential must not be empty");
}

#[tokio::test]
async fn a_duplicate_name_rolls_back_the_states_the_profile_and_the_secret() {
    let app = TestApp::spawn().await;
    let user = app
        .insert_user("creator", "creator@example.com", false, false)
        .await;

    create_project(&app.state, request("mars"), user.id)
        .await
        .expect("the first project is created");

    let error = create_project(
        &app.state,
        NewProjectRequest {
            credential: Some(FAKE_CREDENTIAL.to_string()),
            ..request("mars")
        },
        user.id,
    )
    .await
    .expect_err("the second project is refused");

    assert_eq!(error.status(), axum::http::StatusCode::CONFLICT);
    assert_eq!(error.to_string(), "project name already taken");

    // Only the first project's rows survive: no orphan states, no orphan
    // profile, and — although the credential is written before the project row
    // — no orphan secret.
    assert_eq!(count(&app.pool, COUNT_PROJECTS).await, 1);
    assert_eq!(count(&app.pool, COUNT_TASK_STATES).await, 7);
    assert_eq!(count(&app.pool, COUNT_PROFILES).await, 1);
    assert_eq!(count(&app.pool, COUNT_SECRETS).await, 0);
}

#[tokio::test]
async fn two_projects_are_created_independently() {
    let app = TestApp::spawn().await;
    let user = app
        .insert_user("creator", "creator@example.com", false, false)
        .await;

    let first = create_project(&app.state, request("mars"), user.id)
        .await
        .expect("the first project is created");
    let second = create_project(
        &app.state,
        NewProjectRequest {
            credential: Some(FAKE_CREDENTIAL.to_string()),
            ..request("phobos")
        },
        user.id,
    )
    .await
    .expect("the second project is created");

    assert_ne!(first.id, second.id);
    assert!(!first.has_credential);
    assert!(second.has_credential);

    assert_eq!(count(&app.pool, COUNT_TASK_STATES).await, 14);
    assert_eq!(count(&app.pool, COUNT_PROFILES).await, 2);
    assert_eq!(count(&app.pool, COUNT_SECRETS).await, 1);

    // Each project's default profile serves its own `ready` state.
    let tasks = TaskRepository::new(&app.pool);
    for project in [&first, &second] {
        let profile = ProjectRepository::new(&app.pool)
            .find_default_profile(project.id)
            .await
            .expect("the lookup runs")
            .expect("the project has a default profile");
        let served = tasks
            .list_profile_states(profile.id)
            .await
            .expect("the served states are read");

        assert_eq!(served.len(), 1);
        assert_eq!(served[0].name, "ready");
        assert_eq!(served[0].project_id, project.id);
    }
}
