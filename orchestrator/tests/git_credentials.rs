//! The real [`PatCredentialProvider`] against the test keyring: the header it
//! builds, the audit row every read leaves and the temporary config the header
//! travels in (ADR 0002; `ARCHITECTURE.md`, "Git model", Credentials;
//! `docs/data-model.md`, `secret_uses`).
//!
//! The unit tests beside the module already cover the encoding, the file mode
//! and the guard. What needs a database is the part a signature does not show:
//!
//! - a project with no `GIT_CREDENTIAL` answers `None`, so the caller runs the
//!   command unauthenticated and the remote decides;
//! - a stored credential comes back as `Authorization: Basic
//!   base64("x-access-token:" + PAT)` and leaves exactly one `secret_uses` row
//!   with `purpose = 'git'` and the id pair its actor names;
//! - a stored value the config syntax cannot carry is refused rather than
//!   written;
//! - the identity is the configured `GIT_BOT_NAME`/`GIT_BOT_EMAIL` pair;
//! - a command that fails with the credential config attached records an argv
//!   with no `Authorization` in it.
//!
//! Every credential here is an obviously fake stand-in for a PAT (rule 3).
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use common::TestApp;
use mars_orchestrator::git::{
    CommitIdentity, CredentialConfig, GitActor, GitCommand, GitCredentialProvider, GitError,
    PatCredentialProvider,
};
use mars_orchestrator::models::{
    NewAgentProfile, NewProject, NewSession, ProfileKind, ScopeRef, SecretName, SecretUsePurpose,
    User,
};
use mars_orchestrator::prelude::*;
use mars_orchestrator::repositories::{ProjectRepository, SecretRepository, SessionRepository};
use mars_orchestrator::secrets::{GIT_CREDENTIAL_NAME, set_project_git_credential};
use uuid::Uuid;
use zeroize::Zeroizing;

/// Not a real remote: the fixture the project tests use (rule 3).
const TEST_REMOTE: &str = "https://git.example.com/fake/repo.git";

/// Not a real PAT: an obviously fake stand-in (rule 3).
const FAKE_PAT: &str = "ghp_FAKE_TEST_TOKEN_0000000000";

/// The basic-auth user a PAT authenticates as.
const PAT_USERNAME: &str = "x-access-token";

/// What the provider is asked for. The PAT implementation ignores it.
const ANY_TTL: Duration = Duration::from_secs(300);

/// The bot identity these tests configure, obviously fake (rule 3).
const BOT_NAME: &str = "Mars Test Bot";

/// `.invalid` is reserved and never resolves.
const BOT_EMAIL: &str = "bot@example.invalid";

/// The provider under test, on this app's pool and test keyring.
fn provider(app: &TestApp) -> PatCredentialProvider {
    PatCredentialProvider::new(
        app.pool.clone(),
        app.state.keyring.clone(),
        CommitIdentity {
            name: BOT_NAME.to_string(),
            email: BOT_EMAIL.to_string(),
        },
    )
}

/// A committed project for the credential to belong to.
async fn seed_project(pool: &PgPool, project_name: &str) -> Uuid {
    let project = NewProject::new(project_name, TEST_REMOTE).expect("the test project is valid");
    let mut tx = pool.begin().await.expect("a transaction begins");
    let inserted = ProjectRepository::new(pool)
        .insert(&mut tx, &project)
        .await
        .expect("the project inserts");
    tx.commit().await.expect("the transaction commits");

    inserted.id
}

/// A committed user, for the `User` actor and for `created_by`.
async fn seed_user(app: &TestApp, username: &str) -> User {
    app.insert_user(username, &format!("{username}@example.com"), false, false)
        .await
}

/// A committed session on `project_id`, for the `Session` actor's foreign key.
async fn seed_session(pool: &PgPool, project_id: Uuid, created_by: Uuid) -> Uuid {
    let profile = NewAgentProfile::new(project_id, "default", "localhost/mars-session:test")
        .expect("the test profile is valid");

    let mut tx = pool.begin().await.expect("a transaction begins");
    let profile = ProjectRepository::new(pool)
        .insert_profile(&mut tx, &profile)
        .await
        .expect("the profile inserts");
    tx.commit().await.expect("the transaction commits");

    let mut session = NewSession::new(
        project_id,
        profile.id,
        ProfileKind::Conversational,
        "main",
        format!("fake-mcp-token-hash-{}", Uuid::new_v4()),
    );
    session.created_by = Some(created_by);

    let mut tx = pool.begin().await.expect("a transaction begins");
    let inserted = SessionRepository::new(pool)
        .insert(&mut tx, &session)
        .await
        .expect("the session inserts");
    tx.commit().await.expect("the transaction commits");

    inserted.id
}

/// Store `value` as the project's `GIT_CREDENTIAL`.
async fn store_credential(app: &TestApp, project_id: Uuid, value: &str) {
    set_project_git_credential(
        &app.pool,
        &app.state.keyring,
        project_id,
        Zeroizing::new(value.to_string()),
        None,
    )
    .await
    .expect("the credential is stored");
}

/// The project's `GIT_CREDENTIAL` row id.
async fn credential_id(pool: &PgPool, project_id: Uuid) -> Uuid {
    let name = SecretName::parse(GIT_CREDENTIAL_NAME).expect("the fixed name is valid");
    SecretRepository::new(pool)
        .find_by_name(&ScopeRef::project(project_id), &name)
        .await
        .expect("the lookup runs")
        .expect("the credential row exists")
        .id
}

/// Every `secret_uses` row in the database, however it got there.
async fn use_count(pool: &PgPool) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM secret_uses")
        .fetch_one(pool)
        .await
        .expect("the count runs")
}

#[tokio::test]
async fn a_project_without_a_credential_answers_none_and_audits_nothing() {
    let app = TestApp::spawn().await;
    let project_id = seed_project(&app.pool, "public-remote").await;

    let found = provider(&app)
        .credential_for(project_id, &GitActor::System, ANY_TTL)
        .await
        .expect("the lookup succeeds");

    assert!(
        found.is_none(),
        "a public remote runs with no config file at all"
    );
    assert_eq!(use_count(&app.pool).await, 0);
}

#[tokio::test]
async fn a_stored_pat_becomes_the_documented_basic_header() {
    let app = TestApp::spawn().await;
    let project_id = seed_project(&app.pool, "with-credential").await;
    store_credential(&app, project_id, FAKE_PAT).await;

    let credential = provider(&app)
        .credential_for(project_id, &GitActor::System, ANY_TTL)
        .await
        .expect("the lookup succeeds")
        .expect("the project has a credential");

    let expected = STANDARD.encode(format!("{PAT_USERNAME}:{FAKE_PAT}").as_bytes());
    assert_eq!(
        credential.header_value(),
        format!("Authorization: Basic {expected}")
    );
    assert!(
        credential.expires_at.is_none(),
        "a PAT has no expiry the orchestrator can see"
    );
}

#[tokio::test]
async fn a_read_for_a_user_records_one_git_use_naming_that_user() {
    let app = TestApp::spawn().await;
    let project_id = seed_project(&app.pool, "audited-user").await;
    let ada = seed_user(&app, "ada").await;
    store_credential(&app, project_id, FAKE_PAT).await;

    provider(&app)
        .credential_for(project_id, &GitActor::User(ada.id), ANY_TTL)
        .await
        .expect("the lookup succeeds")
        .expect("the project has a credential");

    let uses = SecretRepository::new(&app.pool)
        .list_uses(credential_id(&app.pool, project_id).await, 10)
        .await
        .expect("the uses are listed");

    assert_eq!(uses.len(), 1, "one read is one use row");
    assert_eq!(uses[0].purpose, SecretUsePurpose::Git);
    assert_eq!(uses[0].user_id, Some(ada.id));
    assert_eq!(uses[0].session_id, None);
}

#[tokio::test]
async fn every_actor_writes_the_id_pair_it_names() {
    let app = TestApp::spawn().await;
    let project_id = seed_project(&app.pool, "audited-all").await;
    let ada = seed_user(&app, "ada").await;
    let session_id = seed_session(&app.pool, project_id, ada.id).await;
    store_credential(&app, project_id, FAKE_PAT).await;

    let provider = provider(&app);

    // An MCP tool, a REST request and the mirror-fetch job, in that order.
    for actor in [
        GitActor::Session(session_id),
        GitActor::User(ada.id),
        GitActor::System,
    ] {
        provider
            .credential_for(project_id, &actor, ANY_TTL)
            .await
            .expect("the lookup succeeds")
            .expect("the project has a credential");
    }

    let uses = SecretRepository::new(&app.pool)
        .list_uses(credential_id(&app.pool, project_id).await, 10)
        .await
        .expect("the uses are listed");

    assert_eq!(uses.len(), 3, "one row per read");
    assert!(
        uses.iter()
            .all(|use_| use_.purpose == SecretUsePurpose::Git),
        "every provider read is a git use"
    );

    let session_use = uses
        .iter()
        .find(|use_| use_.session_id == Some(session_id))
        .expect("the session read is recorded");
    assert_eq!(session_use.user_id, None);

    let user_use = uses
        .iter()
        .find(|use_| use_.user_id == Some(ada.id))
        .expect("the user read is recorded");
    assert_eq!(user_use.session_id, None);

    assert_eq!(
        uses.iter()
            .filter(|use_| use_.session_id.is_none() && use_.user_id.is_none())
            .count(),
        1,
        "the system read sets neither id"
    );
}

#[tokio::test]
async fn a_stored_value_the_config_syntax_cannot_carry_is_refused() {
    let app = TestApp::spawn().await;
    let project_id = seed_project(&app.pool, "broken-credential").await;
    // A value a well-meaning operator pastes with the trailing newline.
    store_credential(&app, project_id, &format!("{FAKE_PAT}\n")).await;

    let error = provider(&app)
        .credential_for(project_id, &GitActor::System, ANY_TTL)
        .await
        .expect_err("a value with a newline in it is refused");

    assert!(
        matches!(error, Error::Git(GitError::CredentialUnavailable)),
        "expected CredentialUnavailable, got {error:?}"
    );

    // The read still happened, so the audit still records it: the refusal is
    // downstream of the decrypt.
    assert_eq!(use_count(&app.pool).await, 1);
}

#[tokio::test]
async fn the_commit_identity_is_the_configured_bot_pair() {
    let app = TestApp::spawn().await;
    let project_id = seed_project(&app.pool, "identity").await;

    let identity = provider(&app)
        .commit_identity(project_id)
        .await
        .expect("the identity is available");

    assert_eq!(identity.name, BOT_NAME);
    assert_eq!(identity.email, BOT_EMAIL);
}

#[tokio::test]
async fn a_command_failing_with_the_config_attached_records_no_authorization() {
    let app = TestApp::spawn().await;
    let project_id = seed_project(&app.pool, "argv").await;
    store_credential(&app, project_id, FAKE_PAT).await;

    let credential = provider(&app)
        .credential_for(project_id, &GitActor::System, ANY_TTL)
        .await
        .expect("the lookup succeeds")
        .expect("the project has a credential");

    let tmp_dir = app.data_dir.path().join("tmp");
    let config = CredentialConfig::write(&credential, &tmp_dir).expect("the config is written");
    let path = config.path().to_path_buf();
    assert!(path.starts_with(&tmp_dir), "{path:?}");

    // A remote that does not exist, so git fails and the failure records the
    // argv the header must never be part of.
    let error = GitCommand::new()
        .args(["fetch", "/nonexistent/mars-test/absent.git"])
        .cwd(app.data_dir.path())
        .config_global(config.path())
        .run_ok()
        .await
        .expect_err("fetching a nonexistent remote fails");

    let GitError::Command { ref args, .. } = error else {
        panic!("expected a command failure, got {error:?}");
    };

    for rendered in [format!("{args:?}"), error.to_string(), format!("{error:?}")] {
        assert!(!rendered.contains("Authorization"), "{rendered}");
        assert!(!rendered.contains(FAKE_PAT), "{rendered}");
    }

    config.close().expect("the config is deleted");
    assert!(!path.exists(), "the credential outlived the command");
}
