//! The fixed-name `GIT_CREDENTIAL` project secret: the lookup the git
//! credential provider makes, the write `POST /api/projects` makes and the
//! boolean the `Project` DTO carries (`docs/data-model.md`, `projects` and
//! `secret_uses`; `SPEC.md`, "Projects (`/api/projects`)"; ADR 0002).
//!
//! What these tests are about is the audit trail and the row shape, because
//! those are the parts a signature does not show:
//!
//! - a project with no credential answers `None` and writes no use row at all;
//! - every successful read writes exactly one `secret_uses` row with
//!   `purpose = 'git'` and the `(session_id, user_id)` pair its context names;
//! - a failed decrypt writes none, so the audit cannot claim a read that never
//!   happened;
//! - storing twice replaces the value in place, under a fresh data key, rather
//!   than leaving two rows or two ciphertexts of one value;
//! - the row is an ordinary orchestrator-only secret, so the secrets API lists
//!   it and the launch resolver never injects it.
//!
//! Every credential here is an obviously fake stand-in for a PAT (rule 3).
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use common::TestApp;
use mars_orchestrator::models::{
    NewAgentProfile, NewProject, NewSession, ProfileKind, ScopeRef, SecretError, SecretName,
    SecretUsePurpose, User,
};
use mars_orchestrator::prelude::*;
use mars_orchestrator::repositories::{
    ProjectRepository, SecretListFilter, SecretRepository, SessionRepository, UserFilter,
};
use mars_orchestrator::secrets::{
    GIT_CREDENTIAL_NAME, GitUseContext, has_project_git_credential, project_git_credential,
    set_project_git_credential,
};
use uuid::Uuid;
use zeroize::Zeroizing;

/// Not a real remote: the fixture the project tests use (rule 3).
const TEST_REMOTE: &str = "https://git.example.com/fake/repo.git";

/// Not a real PAT: an obviously fake stand-in (rule 3).
const FAKE_PAT: &str = "fake-github-pat-value";

/// The replacement, so a test can tell the two apart.
const OTHER_FAKE_PAT: &str = "fake-github-pat-replacement";

fn credential(raw: &str) -> Zeroizing<String> {
    Zeroizing::new(raw.to_string())
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

/// A committed user, for the `User` context and for `created_by`.
async fn seed_user(app: &TestApp, username: &str) -> User {
    app.insert_user(username, &format!("{username}@example.com"), false, false)
        .await
}

/// A committed session on `project_id`, for the `Session` context's foreign
/// key.
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

/// Every `secret_uses` row in the database, however it got there.
///
/// An unchecked count rather than a repository call: the point is that no row
/// exists anywhere, not that one secret has none.
async fn use_count(pool: &PgPool) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM secret_uses")
        .fetch_one(pool)
        .await
        .expect("the count runs")
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

/// The listing `GET /api/secrets?scope=project&scope_id=<pid>` resolves to.
fn project_scope(project_id: Uuid) -> SecretListFilter {
    SecretListFilter {
        scope: Some(ScopeRef::project(project_id)),
        user_ids: UserFilter::All,
    }
}

#[tokio::test]
async fn a_project_without_a_credential_answers_none_and_audits_nothing() {
    let app = TestApp::spawn().await;
    let project_id = seed_project(&app.pool, "no-credential").await;

    let found = project_git_credential(
        &app.pool,
        &app.state.keyring,
        project_id,
        GitUseContext::System,
    )
    .await
    .expect("the lookup succeeds");

    assert!(found.is_none(), "a public remote has no credential");
    assert!(
        !has_project_git_credential(&app.pool, project_id)
            .await
            .expect("the existence check runs")
    );
    assert_eq!(
        use_count(&app.pool).await,
        0,
        "a project with no credential is not a use"
    );
}

#[tokio::test]
async fn a_stored_credential_comes_back_and_records_one_git_use() {
    let app = TestApp::spawn().await;
    let project_id = seed_project(&app.pool, "with-credential").await;
    let ada = seed_user(&app, "ada").await;

    set_project_git_credential(
        &app.pool,
        &app.state.keyring,
        project_id,
        credential(FAKE_PAT),
        Some(ada.id),
    )
    .await
    .expect("the credential is stored");

    let found = project_git_credential(
        &app.pool,
        &app.state.keyring,
        project_id,
        GitUseContext::User(ada.id),
    )
    .await
    .expect("the lookup succeeds")
    .expect("the project has a credential");

    assert_eq!(found.as_str(), FAKE_PAT, "the raw value comes back");

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
async fn every_context_writes_the_id_pair_it_names() {
    let app = TestApp::spawn().await;
    let project_id = seed_project(&app.pool, "audited").await;
    let ada = seed_user(&app, "ada").await;
    let session_id = seed_session(&app.pool, project_id, ada.id).await;

    set_project_git_credential(
        &app.pool,
        &app.state.keyring,
        project_id,
        credential(FAKE_PAT),
        None,
    )
    .await
    .expect("the credential is stored");

    // An MCP tool, a REST request and the mirror-fetch job, in that order.
    for context in [
        GitUseContext::Session(session_id),
        GitUseContext::User(ada.id),
        GitUseContext::System,
    ] {
        let found = project_git_credential(&app.pool, &app.state.keyring, project_id, context)
            .await
            .expect("the lookup succeeds")
            .expect("the project has a credential");
        assert_eq!(found.as_str(), FAKE_PAT);
    }

    let uses = SecretRepository::new(&app.pool)
        .list_uses(credential_id(&app.pool, project_id).await, 10)
        .await
        .expect("the uses are listed");

    assert_eq!(uses.len(), 3, "one row per read");
    assert!(
        uses.iter()
            .all(|use_row| use_row.purpose == SecretUsePurpose::Git),
        "{uses:?}"
    );

    // Newest first, so the three come back in the reverse of the order above.
    let pairs: Vec<(Option<Uuid>, Option<Uuid>)> = uses
        .iter()
        .map(|use_row| (use_row.session_id, use_row.user_id))
        .collect();
    assert_eq!(
        pairs,
        vec![(None, None), (None, Some(ada.id)), (Some(session_id), None),]
    );
}

#[tokio::test]
async fn storing_twice_replaces_the_value_in_place() {
    let app = TestApp::spawn().await;
    let project_id = seed_project(&app.pool, "replaced").await;
    let repository = SecretRepository::new(&app.pool);

    set_project_git_credential(
        &app.pool,
        &app.state.keyring,
        project_id,
        credential(FAKE_PAT),
        None,
    )
    .await
    .expect("the credential is stored");

    let before = repository
        .find(credential_id(&app.pool, project_id).await)
        .await
        .expect("the row reads")
        .expect("the row exists");

    set_project_git_credential(
        &app.pool,
        &app.state.keyring,
        project_id,
        credential(OTHER_FAKE_PAT),
        None,
    )
    .await
    .expect("the credential is replaced");

    let after = repository
        .find(before.id)
        .await
        .expect("the row reads")
        .expect("the row exists");

    assert_eq!(after.id, before.id, "the replacement is the same row");
    assert_eq!(
        repository
            .list_meta_filtered(&project_scope(project_id))
            .await
            .expect("the scope lists")
            .len(),
        1,
        "and not a second row of the same name"
    );
    assert!(
        after.updated_at > before.updated_at,
        "{} is not after {}",
        after.updated_at,
        before.updated_at
    );
    assert_ne!(
        after.data_key_wrapped, before.data_key_wrapped,
        "a replacement draws a fresh data key"
    );
    assert_ne!(after.ciphertext, before.ciphertext);
    assert_eq!(after.created_at, before.created_at);

    let found = project_git_credential(
        &app.pool,
        &app.state.keyring,
        project_id,
        GitUseContext::System,
    )
    .await
    .expect("the lookup succeeds")
    .expect("the project has a credential");
    assert_eq!(found.as_str(), OTHER_FAKE_PAT);
}

#[tokio::test]
async fn the_row_is_an_ordinary_orchestrator_only_secret() {
    let app = TestApp::spawn().await;
    let project_id = seed_project(&app.pool, "listed").await;
    let ada = seed_user(&app, "ada").await;

    assert!(
        !has_project_git_credential(&app.pool, project_id)
            .await
            .expect("the existence check runs"),
        "nothing before the credential is stored"
    );

    set_project_git_credential(
        &app.pool,
        &app.state.keyring,
        project_id,
        credential(FAKE_PAT),
        Some(ada.id),
    )
    .await
    .expect("the credential is stored");

    assert!(
        has_project_git_credential(&app.pool, project_id)
            .await
            .expect("the existence check runs"),
        "and the DTO's boolean afterwards"
    );

    // What `GET /api/secrets?scope=project&scope_id=<pid>` answers: a row like
    // any other, which a user may rotate or delete, and which the resolver
    // never injects because of the flag.
    let listed = SecretRepository::new(&app.pool)
        .list_meta_filtered(&project_scope(project_id))
        .await
        .expect("the scope lists");

    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].name, GIT_CREDENTIAL_NAME);
    assert!(
        listed[0].orchestrator_only,
        "never injected into a container"
    );
    assert_eq!(listed[0].scope_id, Some(project_id));
    assert_eq!(listed[0].created_by, Some(ada.id));
    assert_eq!(listed[0].last_used_at, None, "storing is not a use");
}

#[tokio::test]
async fn an_empty_or_oversize_credential_is_a_bad_request() {
    let app = TestApp::spawn().await;
    let project_id = seed_project(&app.pool, "rejected").await;

    let error = set_project_git_credential(
        &app.pool,
        &app.state.keyring,
        project_id,
        credential(""),
        None,
    )
    .await
    .expect_err("an empty credential is rejected");
    assert!(
        matches!(error, Error::Secret(SecretError::InvalidValue)),
        "{error:?}"
    );

    let oversize = "x".repeat(65_536 + 1);
    let error = set_project_git_credential(
        &app.pool,
        &app.state.keyring,
        project_id,
        credential(&oversize),
        None,
    )
    .await
    .expect_err("an oversize credential is rejected");
    assert!(
        matches!(error, Error::Secret(SecretError::InvalidValue)),
        "{error:?}"
    );

    assert!(
        !has_project_git_credential(&app.pool, project_id)
            .await
            .expect("the existence check runs"),
        "a rejected value stores nothing"
    );
}

#[tokio::test]
async fn a_row_that_cannot_be_decrypted_is_internal_and_writes_no_use() {
    let app = TestApp::spawn().await;
    let project_id = seed_project(&app.pool, "corrupt").await;

    set_project_git_credential(
        &app.pool,
        &app.state.keyring,
        project_id,
        credential(FAKE_PAT),
        None,
    )
    .await
    .expect("the credential is stored");

    // A corrupt row, as a lost master key or a tampered dump would leave it.
    // Unchecked because no repository has — or should have — a statement that
    // breaks a ciphertext.
    let secret_id = credential_id(&app.pool, project_id).await;
    sqlx::query("UPDATE secrets SET ciphertext = $1 WHERE id = $2")
        .bind(b"not-a-ciphertext".as_slice())
        .bind(secret_id)
        .execute(&app.pool)
        .await
        .expect("the row is corrupted");

    let error = project_git_credential(
        &app.pool,
        &app.state.keyring,
        project_id,
        GitUseContext::System,
    )
    .await
    .expect_err("an unreadable row is a failure");

    assert!(matches!(error, Error::Internal(_)), "{error:?}");
    assert!(
        !error.to_string().contains(FAKE_PAT),
        "the message says nothing about the value"
    );
    assert_eq!(
        use_count(&app.pool).await,
        0,
        "a read that failed is not a use"
    );
}
