//! The domain operations behind `/api/secrets` (`SPEC.md`, "Secrets
//! (`/api/secrets`)"; `src/secrets/service.rs`).
//!
//! Driven directly against a real database and the harness's fixed test
//! keyring rather than through HTTP, because everything asserted here is
//! decided before a response is written: who may touch a row, what a scope
//! means, and what a rename does to the four encrypted columns. The route
//! module's own tests are the adapter's.
//!
//! The rules under test are the ones no signature shows:
//!
//! - the ownership rule is the same for listing, changing and deleting, and
//!   `global` and `project` secrets are open to every authenticated user;
//! - a rename really re-encrypts — the value opens under the new additional
//!   authenticated data and no longer under the old one — and keeps its data
//!   key, while a flag-only patch touches no ciphertext at all;
//! - a scope target that does not exist is a 400 rather than a foreign key
//!   error, because `scope_id` has none (`docs/data-model.md`, `secrets`);
//! - an unscoped listing shows the caller everything they may see and nothing
//!   they may not.
//!
//! Every value here is an obviously fake credential and no test prints one
//! (`CLAUDE.md`, rule 3).
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use axum::http::StatusCode;
use common::TestApp;
use mars_orchestrator::models::{
    EncryptedValue, MAX_SECRET_VALUE_BYTES, NewProject, Secret, SecretMeta, SecretName,
    SecretScope, SecretUsePurpose, User,
};
use mars_orchestrator::prelude::*;
use mars_orchestrator::repositories::{ProjectRepository, SecretRepository};
use mars_orchestrator::secrets::service::{
    Actor, CreateSecret, DEFAULT_USES_LIMIT, MAX_USES_LIMIT, PatchSecret, SecretsService,
};
use mars_orchestrator::secrets::{aad, open};
use uuid::Uuid;
use zeroize::Zeroizing;

/// Not a real remote: the fixture the project tests use (rule 3).
const TEST_REMOTE: &str = "https://git.example.com/fake/repo.git";

/// Not a real credential: an obviously fake value for the rows a test seals
/// (rule 3).
const FAKE_VALUE: &str = "fake-value-not-a-credential";

fn service(app: &TestApp) -> SecretsService<'_> {
    SecretsService::new(&app.pool, &app.state.keyring)
}

fn actor(user: &User) -> Actor {
    Actor::new(user.id, user.admin)
}

fn value(raw: &str) -> Zeroizing<String> {
    Zeroizing::new(raw.to_string())
}

/// A create request with the two fields most tests do not vary.
fn create_request(scope: SecretScope, scope_id: Option<Uuid>, name: &str) -> CreateSecret {
    CreateSecret {
        scope,
        scope_id,
        name: name.to_string(),
        value: value(FAKE_VALUE),
        orchestrator_only: false,
    }
}

/// A committed user for the `user` scope to point at.
async fn seed_user(app: &TestApp, username: &str, admin: bool) -> User {
    app.insert_user(username, &format!("{username}@example.com"), admin, false)
        .await
}

/// A committed project for the `project` scope to point at.
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

/// The stored row behind a metadata answer, encrypted columns included.
async fn row(app: &TestApp, id: Uuid) -> Secret {
    SecretRepository::new(&app.pool)
        .find(id)
        .await
        .expect("the row reads")
        .expect("the row is still there")
}

/// The four encrypted columns of a row, as the crypto layer takes them.
fn encrypted(row: &Secret) -> EncryptedValue {
    EncryptedValue {
        ciphertext: row.ciphertext.clone(),
        nonce: row.nonce.clone(),
        data_key_wrapped: row.data_key_wrapped.clone(),
        data_key_nonce: row.data_key_nonce.clone(),
        key_version: row.key_version,
    }
}

/// The plaintext of a stored row under its own identity.
fn opened(app: &TestApp, row: &Secret) -> String {
    let plaintext = open(
        &app.state.keyring,
        &aad(row.scope, row.scope_id, &row.name),
        &encrypted(row),
    )
    .expect("the stored row opens under its own AAD");

    String::from_utf8(plaintext.to_vec()).expect("the test value is UTF-8")
}

/// The names in a listing, which is what every visibility assertion compares.
fn names(listing: &[SecretMeta]) -> Vec<&str> {
    listing.iter().map(|meta| meta.name.as_str()).collect()
}

/// Assert an operation failed with this status, whichever error carried it.
///
/// A bad name widens through `SecretError` and a bad scope through the
/// service's own `BadRequest`; both are the 400 `SPEC.md`, "REST API"
/// promises, and the status is the contract.
#[track_caller]
fn assert_status<T>(result: Result<T>, expected: StatusCode) {
    match result {
        Ok(_) => panic!("expected {expected}, got success"),
        Err(error) => assert_eq!(error.status(), expected, "{error}"),
    }
}

/// A secret created by `owner` in their own scope, for the tests that only
/// need one to point at.
async fn user_secret(app: &TestApp, owner: &User, name: &str) -> SecretMeta {
    service(app)
        .create(&actor(owner), create_request(SecretScope::User, None, name))
        .await
        .expect("the secret is created")
}

// ---- create ----

#[tokio::test]
async fn a_global_secret_is_created_without_a_scope_id() {
    let app = TestApp::spawn().await;
    let ada = seed_user(&app, "ada", false).await;

    let meta = service(&app)
        .create(
            &actor(&ada),
            create_request(SecretScope::Global, None, "DEPLOY_TOKEN"),
        )
        .await
        .expect("a global secret is created");

    assert_eq!(meta.scope, SecretScope::Global);
    assert_eq!(meta.scope_id, None);
    assert_eq!(meta.name, "DEPLOY_TOKEN");
    assert!(!meta.orchestrator_only);
    assert_eq!(meta.created_by, Some(ada.id));
    assert_eq!(meta.last_used_at, None);
    assert_eq!(meta.key_version, app.state.keyring.current_version() as i32);

    // The value is sealed under this row's identity and nowhere in the answer.
    assert_eq!(opened(&app, &row(&app, meta.id).await), FAKE_VALUE);
}

#[tokio::test]
async fn a_project_secret_is_created_with_the_project_id() {
    let app = TestApp::spawn().await;
    let ada = seed_user(&app, "ada", false).await;
    let project = seed_project(&app.pool, "mars").await;

    let meta = service(&app)
        .create(
            &actor(&ada),
            CreateSecret {
                orchestrator_only: true,
                ..create_request(SecretScope::Project, Some(project), "DEPLOY_TOKEN")
            },
        )
        .await
        .expect("a project secret is created");

    assert_eq!(meta.scope, SecretScope::Project);
    assert_eq!(meta.scope_id, Some(project));
    assert!(meta.orchestrator_only);
    assert_eq!(opened(&app, &row(&app, meta.id).await), FAKE_VALUE);
}

#[tokio::test]
async fn a_user_secret_defaults_to_the_caller() {
    let app = TestApp::spawn().await;
    let ada = seed_user(&app, "ada", false).await;

    let meta = user_secret(&app, &ada, "DEPLOY_TOKEN").await;

    assert_eq!(meta.scope, SecretScope::User);
    assert_eq!(meta.scope_id, Some(ada.id));
}

#[tokio::test]
async fn a_global_secret_refuses_a_scope_id() {
    let app = TestApp::spawn().await;
    let ada = seed_user(&app, "ada", false).await;

    let refused = service(&app)
        .create(
            &actor(&ada),
            create_request(SecretScope::Global, Some(ada.id), "DEPLOY_TOKEN"),
        )
        .await;

    assert_status(refused, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_project_secret_needs_a_scope_id() {
    let app = TestApp::spawn().await;
    let ada = seed_user(&app, "ada", false).await;

    let refused = service(&app)
        .create(
            &actor(&ada),
            create_request(SecretScope::Project, None, "DEPLOY_TOKEN"),
        )
        .await;

    assert_status(refused, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_duplicate_name_in_one_scope_is_a_conflict() {
    let app = TestApp::spawn().await;
    let ada = seed_user(&app, "ada", false).await;

    user_secret(&app, &ada, "DEPLOY_TOKEN").await;
    let refused = service(&app)
        .create(
            &actor(&ada),
            create_request(SecretScope::User, None, "DEPLOY_TOKEN"),
        )
        .await;

    assert_status(refused, StatusCode::CONFLICT);

    // The same name in another scope is not a duplicate.
    service(&app)
        .create(
            &actor(&ada),
            create_request(SecretScope::Global, None, "DEPLOY_TOKEN"),
        )
        .await
        .expect("the global scope is a different row");
}

#[tokio::test]
async fn a_name_outside_the_pattern_is_rejected() {
    let app = TestApp::spawn().await;
    let ada = seed_user(&app, "ada", false).await;

    for name in ["", "deploy_token", "1TOKEN", "DEPLOY-TOKEN", "DEPLOY TOKEN"] {
        let refused = service(&app)
            .create(
                &actor(&ada),
                create_request(SecretScope::Global, None, name),
            )
            .await;

        assert_status(refused, StatusCode::BAD_REQUEST);
    }
}

#[tokio::test]
async fn an_empty_or_oversize_value_is_rejected() {
    let app = TestApp::spawn().await;
    let ada = seed_user(&app, "ada", false).await;

    for raw in [String::new(), "x".repeat(MAX_SECRET_VALUE_BYTES + 1)] {
        let refused = service(&app)
            .create(
                &actor(&ada),
                CreateSecret {
                    value: value(&raw),
                    ..create_request(SecretScope::Global, None, "DEPLOY_TOKEN")
                },
            )
            .await;

        assert_status(refused, StatusCode::BAD_REQUEST);
    }

    // Exactly at the limit is accepted, so the boundary is the documented one.
    service(&app)
        .create(
            &actor(&ada),
            CreateSecret {
                value: value(&"x".repeat(MAX_SECRET_VALUE_BYTES)),
                ..create_request(SecretScope::Global, None, "DEPLOY_TOKEN")
            },
        )
        .await
        .expect("a value at the limit is accepted");
}

#[tokio::test]
async fn an_unknown_project_or_user_is_a_bad_request() {
    let app = TestApp::spawn().await;
    let admin = seed_user(&app, "admin", true).await;

    let refused = service(&app)
        .create(
            &actor(&admin),
            create_request(SecretScope::Project, Some(Uuid::new_v4()), "DEPLOY_TOKEN"),
        )
        .await;
    assert_status(refused, StatusCode::BAD_REQUEST);

    // An administrator may create for another user, but not for one who is not
    // there: `scope_id` has no foreign key to say so.
    let refused = service(&app)
        .create(
            &actor(&admin),
            create_request(SecretScope::User, Some(Uuid::new_v4()), "DEPLOY_TOKEN"),
        )
        .await;
    assert_status(refused, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn creating_for_another_user_needs_an_administrator() {
    let app = TestApp::spawn().await;
    let ada = seed_user(&app, "ada", false).await;
    let bob = seed_user(&app, "bob", false).await;
    let admin = seed_user(&app, "admin", true).await;

    let refused = service(&app)
        .create(
            &actor(&ada),
            create_request(SecretScope::User, Some(bob.id), "DEPLOY_TOKEN"),
        )
        .await;
    assert_status(refused, StatusCode::FORBIDDEN);

    let meta = service(&app)
        .create(
            &actor(&admin),
            create_request(SecretScope::User, Some(bob.id), "DEPLOY_TOKEN"),
        )
        .await
        .expect("an administrator creates for another user");

    assert_eq!(meta.scope_id, Some(bob.id));
    assert_eq!(meta.created_by, Some(admin.id));
}

// ---- replace_value ----

#[tokio::test]
async fn replacing_a_value_re_seals_it_under_a_fresh_data_key() {
    let app = TestApp::spawn().await;
    let ada = seed_user(&app, "ada", false).await;

    let meta = user_secret(&app, &ada, "DEPLOY_TOKEN").await;
    let before = row(&app, meta.id).await;

    let replaced = service(&app)
        .replace_value(&actor(&ada), meta.id, value("fake-value-the-second"))
        .await
        .expect("the owner replaces the value");

    let after = row(&app, meta.id).await;
    assert_eq!(opened(&app, &after), "fake-value-the-second");
    assert_ne!(after.ciphertext, before.ciphertext);
    // A new value gets a new data key, not the old one re-used.
    assert_ne!(after.data_key_wrapped, before.data_key_wrapped);
    assert_eq!(after.name, before.name);
    assert!(replaced.updated_at > meta.updated_at);
}

#[tokio::test]
async fn replacing_a_value_validates_it() {
    let app = TestApp::spawn().await;
    let ada = seed_user(&app, "ada", false).await;
    let meta = user_secret(&app, &ada, "DEPLOY_TOKEN").await;

    let refused = service(&app)
        .replace_value(&actor(&ada), meta.id, value(""))
        .await;
    assert_status(refused, StatusCode::BAD_REQUEST);

    // And the stored value is untouched.
    assert_eq!(opened(&app, &row(&app, meta.id).await), FAKE_VALUE);
}

#[tokio::test]
async fn only_the_owner_or_an_administrator_replaces_a_user_secret() {
    let app = TestApp::spawn().await;
    let ada = seed_user(&app, "ada", false).await;
    let bob = seed_user(&app, "bob", false).await;
    let admin = seed_user(&app, "admin", true).await;
    let meta = user_secret(&app, &ada, "DEPLOY_TOKEN").await;

    let refused = service(&app)
        .replace_value(&actor(&bob), meta.id, value("fake-value-from-bob"))
        .await;
    assert_status(refused, StatusCode::FORBIDDEN);

    service(&app)
        .replace_value(&actor(&admin), meta.id, value("fake-value-from-the-admin"))
        .await
        .expect("an administrator replaces another user's value");

    assert_eq!(
        opened(&app, &row(&app, meta.id).await),
        "fake-value-from-the-admin"
    );
}

#[tokio::test]
async fn a_global_secret_is_replaced_by_any_user() {
    let app = TestApp::spawn().await;
    let ada = seed_user(&app, "ada", false).await;
    let bob = seed_user(&app, "bob", false).await;

    let meta = service(&app)
        .create(
            &actor(&ada),
            create_request(SecretScope::Global, None, "DEPLOY_TOKEN"),
        )
        .await
        .expect("a global secret is created");

    service(&app)
        .replace_value(&actor(&bob), meta.id, value("fake-value-from-bob"))
        .await
        .expect("global secrets are open to every user");
}

#[tokio::test]
async fn an_unknown_secret_is_not_found() {
    let app = TestApp::spawn().await;
    let ada = seed_user(&app, "ada", false).await;
    let absent = Uuid::new_v4();

    assert_status(
        service(&app)
            .replace_value(&actor(&ada), absent, value(FAKE_VALUE))
            .await,
        StatusCode::NOT_FOUND,
    );
    assert_status(
        service(&app)
            .patch(
                &actor(&ada),
                absent,
                PatchSecret {
                    name: Some("OTHER_TOKEN".into()),
                    orchestrator_only: None,
                },
            )
            .await,
        StatusCode::NOT_FOUND,
    );
    assert_status(
        service(&app).delete(&actor(&ada), absent).await,
        StatusCode::NOT_FOUND,
    );
    assert_status(
        service(&app).uses(&actor(&ada), absent, None).await,
        StatusCode::NOT_FOUND,
    );
}

// ---- patch ----

#[tokio::test]
async fn a_rename_re_encrypts_under_the_new_aad_and_keeps_the_data_key() {
    let app = TestApp::spawn().await;
    let ada = seed_user(&app, "ada", false).await;

    let meta = user_secret(&app, &ada, "DEPLOY_TOKEN").await;
    let before = row(&app, meta.id).await;
    let old_aad = aad(before.scope, before.scope_id, &before.name);

    let renamed = service(&app)
        .patch(
            &actor(&ada),
            meta.id,
            PatchSecret {
                name: Some("RELEASE_TOKEN".into()),
                orchestrator_only: None,
            },
        )
        .await
        .expect("the owner renames the secret");

    assert_eq!(renamed.name, "RELEASE_TOKEN");
    assert!(renamed.updated_at > meta.updated_at);

    let after = row(&app, meta.id).await;

    // The value opens under the new identity and no longer under the old one.
    assert_eq!(opened(&app, &after), FAKE_VALUE);
    assert!(
        open(&app.state.keyring, &old_aad, &encrypted(&after)).is_err(),
        "the old AAD must no longer open the row"
    );

    // Renaming re-encrypts; it does not re-key (`docs/data-model.md`).
    assert_eq!(after.data_key_wrapped, before.data_key_wrapped);
    assert_eq!(after.data_key_nonce, before.data_key_nonce);
    assert_eq!(after.key_version, before.key_version);
    assert_ne!(after.ciphertext, before.ciphertext);
    assert_ne!(after.nonce, before.nonce);
    assert!(after.updated_at > before.updated_at);
}

#[tokio::test]
async fn a_rename_onto_a_taken_name_is_a_conflict() {
    let app = TestApp::spawn().await;
    let ada = seed_user(&app, "ada", false).await;

    let first = user_secret(&app, &ada, "DEPLOY_TOKEN").await;
    user_secret(&app, &ada, "RELEASE_TOKEN").await;

    let refused = service(&app)
        .patch(
            &actor(&ada),
            first.id,
            PatchSecret {
                name: Some("RELEASE_TOKEN".into()),
                orchestrator_only: None,
            },
        )
        .await;

    assert_status(refused, StatusCode::CONFLICT);

    // The rolled-back transaction left the row exactly as it was.
    let unchanged = row(&app, first.id).await;
    assert_eq!(unchanged.name, "DEPLOY_TOKEN");
    assert_eq!(opened(&app, &unchanged), FAKE_VALUE);
}

#[tokio::test]
async fn a_rename_to_the_same_name_re_encrypts_nothing_but_still_applies_the_flag() {
    let app = TestApp::spawn().await;
    let ada = seed_user(&app, "ada", false).await;

    let meta = user_secret(&app, &ada, "DEPLOY_TOKEN").await;
    let before = row(&app, meta.id).await;

    let patched = service(&app)
        .patch(
            &actor(&ada),
            meta.id,
            PatchSecret {
                name: Some("DEPLOY_TOKEN".into()),
                orchestrator_only: Some(true),
            },
        )
        .await
        .expect("the same name with a new flag is applied");

    assert!(patched.orchestrator_only);

    let after = row(&app, meta.id).await;
    assert_eq!(after.ciphertext, before.ciphertext);
    assert_eq!(after.nonce, before.nonce);
    assert_eq!(after.name, "DEPLOY_TOKEN");
}

#[tokio::test]
async fn a_flag_only_patch_leaves_the_ciphertext_alone() {
    let app = TestApp::spawn().await;
    let ada = seed_user(&app, "ada", false).await;

    let meta = user_secret(&app, &ada, "DEPLOY_TOKEN").await;
    let before = row(&app, meta.id).await;

    let patched = service(&app)
        .patch(
            &actor(&ada),
            meta.id,
            PatchSecret {
                name: None,
                orchestrator_only: Some(true),
            },
        )
        .await
        .expect("the flag is set");

    assert!(patched.orchestrator_only);
    assert!(patched.updated_at > meta.updated_at);

    let after = row(&app, meta.id).await;
    assert_eq!(after.ciphertext, before.ciphertext);
    assert_eq!(after.nonce, before.nonce);
    assert_eq!(after.data_key_wrapped, before.data_key_wrapped);
}

#[tokio::test]
async fn a_patch_that_changes_nothing_answers_the_current_metadata() {
    let app = TestApp::spawn().await;
    let ada = seed_user(&app, "ada", false).await;

    let meta = user_secret(&app, &ada, "DEPLOY_TOKEN").await;

    let unchanged = service(&app)
        .patch(
            &actor(&ada),
            meta.id,
            PatchSecret {
                name: None,
                orchestrator_only: None,
            },
        )
        .await
        .expect("an empty patch is not an error");
    assert_eq!(unchanged, meta);

    // The same when both fields repeat what the row already says: no write, so
    // `updated_at` does not move.
    let unchanged = service(&app)
        .patch(
            &actor(&ada),
            meta.id,
            PatchSecret {
                name: Some("DEPLOY_TOKEN".into()),
                orchestrator_only: Some(false),
            },
        )
        .await
        .expect("a patch that repeats the row is not an error");
    assert_eq!(unchanged.updated_at, meta.updated_at);
}

#[tokio::test]
async fn a_patch_validates_the_new_name_even_when_nothing_else_changes() {
    let app = TestApp::spawn().await;
    let ada = seed_user(&app, "ada", false).await;
    let meta = user_secret(&app, &ada, "DEPLOY_TOKEN").await;

    let refused = service(&app)
        .patch(
            &actor(&ada),
            meta.id,
            PatchSecret {
                name: Some("not a name".into()),
                orchestrator_only: None,
            },
        )
        .await;

    assert_status(refused, StatusCode::BAD_REQUEST);
    assert_eq!(row(&app, meta.id).await.name, "DEPLOY_TOKEN");
}

#[tokio::test]
async fn only_the_owner_or_an_administrator_patches_a_user_secret() {
    let app = TestApp::spawn().await;
    let ada = seed_user(&app, "ada", false).await;
    let bob = seed_user(&app, "bob", false).await;
    let admin = seed_user(&app, "admin", true).await;
    let meta = user_secret(&app, &ada, "DEPLOY_TOKEN").await;

    let refused = service(&app)
        .patch(
            &actor(&bob),
            meta.id,
            PatchSecret {
                name: Some("BOBS_TOKEN".into()),
                orchestrator_only: None,
            },
        )
        .await;
    assert_status(refused, StatusCode::FORBIDDEN);

    let renamed = service(&app)
        .patch(
            &actor(&admin),
            meta.id,
            PatchSecret {
                name: Some("RELEASE_TOKEN".into()),
                orchestrator_only: None,
            },
        )
        .await
        .expect("an administrator renames another user's secret");
    assert_eq!(renamed.name, "RELEASE_TOKEN");
    assert_eq!(opened(&app, &row(&app, meta.id).await), FAKE_VALUE);
}

// ---- delete ----

#[tokio::test]
async fn the_owner_deletes_and_another_user_may_not() {
    let app = TestApp::spawn().await;
    let ada = seed_user(&app, "ada", false).await;
    let bob = seed_user(&app, "bob", false).await;
    let meta = user_secret(&app, &ada, "DEPLOY_TOKEN").await;

    assert_status(
        service(&app).delete(&actor(&bob), meta.id).await,
        StatusCode::FORBIDDEN,
    );

    service(&app)
        .delete(&actor(&ada), meta.id)
        .await
        .expect("the owner deletes their secret");

    assert!(
        SecretRepository::new(&app.pool)
            .find(meta.id)
            .await
            .expect("the read succeeds")
            .is_none()
    );

    // And a second delete is a 404 rather than a silent success.
    assert_status(
        service(&app).delete(&actor(&ada), meta.id).await,
        StatusCode::NOT_FOUND,
    );
}

#[tokio::test]
async fn an_administrator_deletes_another_users_secret() {
    let app = TestApp::spawn().await;
    let ada = seed_user(&app, "ada", false).await;
    let admin = seed_user(&app, "admin", true).await;
    let meta = user_secret(&app, &ada, "DEPLOY_TOKEN").await;

    service(&app)
        .delete(&actor(&admin), meta.id)
        .await
        .expect("an administrator deletes another user's secret");
}

// ---- list ----

/// One of every scope, owned by three different users.
async fn seed_listing(app: &TestApp) -> (User, User, User, Uuid) {
    let ada = seed_user(app, "ada", false).await;
    let bob = seed_user(app, "bob", false).await;
    let admin = seed_user(app, "admin", true).await;
    let project = seed_project(&app.pool, "mars").await;

    let service = service(app);
    service
        .create(
            &actor(&ada),
            create_request(SecretScope::Global, None, "GLOBAL_TOKEN"),
        )
        .await
        .expect("the global secret is created");
    service
        .create(
            &actor(&ada),
            create_request(SecretScope::Project, Some(project), "PROJECT_TOKEN"),
        )
        .await
        .expect("the project secret is created");
    service
        .create(
            &actor(&ada),
            create_request(SecretScope::User, None, "ADA_TOKEN"),
        )
        .await
        .expect("ada's secret is created");
    service
        .create(
            &actor(&bob),
            create_request(SecretScope::User, None, "BOB_TOKEN"),
        )
        .await
        .expect("bob's secret is created");

    (ada, bob, admin, project)
}

#[tokio::test]
async fn an_unscoped_listing_shows_a_user_theirs_and_an_administrator_everyones() {
    let app = TestApp::spawn().await;
    let (ada, _bob, admin, _project) = seed_listing(&app).await;

    let mine = service(&app)
        .list(&actor(&ada), None, None)
        .await
        .expect("the listing succeeds");
    assert_eq!(
        names(&mine),
        vec!["GLOBAL_TOKEN", "ADA_TOKEN", "PROJECT_TOKEN"]
    );

    let all = service(&app)
        .list(&actor(&admin), None, None)
        .await
        .expect("the listing succeeds");
    let mut listed = names(&all);
    listed.sort_unstable();
    assert_eq!(
        listed,
        vec!["ADA_TOKEN", "BOB_TOKEN", "GLOBAL_TOKEN", "PROJECT_TOKEN"]
    );
}

#[tokio::test]
async fn a_user_listing_defaults_to_the_caller_for_everybody() {
    let app = TestApp::spawn().await;
    let (ada, bob, admin, _project) = seed_listing(&app).await;

    let mine = service(&app)
        .list(&actor(&ada), Some(SecretScope::User), None)
        .await
        .expect("the listing succeeds");
    assert_eq!(names(&mine), vec!["ADA_TOKEN"]);

    // Including for an administrator: another user needs an explicit id.
    let theirs = service(&app)
        .list(&actor(&admin), Some(SecretScope::User), None)
        .await
        .expect("the listing succeeds");
    assert!(theirs.is_empty(), "{:?}", names(&theirs));

    let bobs = service(&app)
        .list(&actor(&admin), Some(SecretScope::User), Some(bob.id))
        .await
        .expect("an administrator selects another user");
    assert_eq!(names(&bobs), vec!["BOB_TOKEN"]);
}

#[tokio::test]
async fn another_users_listing_is_forbidden_without_admin() {
    let app = TestApp::spawn().await;
    let (ada, bob, _admin, _project) = seed_listing(&app).await;

    let refused = service(&app)
        .list(&actor(&ada), Some(SecretScope::User), Some(bob.id))
        .await;

    assert_status(refused, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn the_global_and_project_listings_check_their_scope_id() {
    let app = TestApp::spawn().await;
    let (ada, _bob, _admin, project) = seed_listing(&app).await;

    let global = service(&app)
        .list(&actor(&ada), Some(SecretScope::Global), None)
        .await
        .expect("the global listing succeeds");
    assert_eq!(names(&global), vec!["GLOBAL_TOKEN"]);

    assert_status(
        service(&app)
            .list(&actor(&ada), Some(SecretScope::Global), Some(project))
            .await,
        StatusCode::BAD_REQUEST,
    );

    let project_secrets = service(&app)
        .list(&actor(&ada), Some(SecretScope::Project), Some(project))
        .await
        .expect("the project listing succeeds");
    assert_eq!(names(&project_secrets), vec!["PROJECT_TOKEN"]);

    assert_status(
        service(&app)
            .list(&actor(&ada), Some(SecretScope::Project), None)
            .await,
        StatusCode::BAD_REQUEST,
    );
}

// ---- uses ----

/// Record `count` uses of `secret_id`, each a second older than the last.
///
/// One statement rather than `insert_use` in a loop: the point of these tests
/// is the limit, and six hundred round trips would say nothing extra. The
/// repositories have no bulk insert, so this is the documented unchecked
/// escape hatch.
async fn seed_uses(pool: &PgPool, secret_id: Uuid, count: i32) {
    sqlx::query(
        "INSERT INTO secret_uses (secret_id, purpose, at)
         SELECT $1, 'launch', NOW() - make_interval(secs => g)
         FROM generate_series(1, $2) AS g",
    )
    .bind(secret_id)
    .bind(count)
    .execute(pool)
    .await
    .expect("the uses insert");
}

#[tokio::test]
async fn the_uses_limit_defaults_to_fifty_and_is_capped_at_five_hundred() {
    let app = TestApp::spawn().await;
    let ada = seed_user(&app, "ada", false).await;
    let meta = user_secret(&app, &ada, "DEPLOY_TOKEN").await;

    seed_uses(&app.pool, meta.id, 600).await;
    let service = service(&app);

    let default = service
        .uses(&actor(&ada), meta.id, None)
        .await
        .expect("the default limit applies");
    assert_eq!(default.len(), DEFAULT_USES_LIMIT as usize);

    let asked = service
        .uses(&actor(&ada), meta.id, Some(10))
        .await
        .expect("an explicit limit applies");
    assert_eq!(asked.len(), 10);

    let capped = service
        .uses(&actor(&ada), meta.id, Some(10_000))
        .await
        .expect("an excessive limit is reduced");
    assert_eq!(capped.len(), MAX_USES_LIMIT as usize);

    assert_status(
        service.uses(&actor(&ada), meta.id, Some(0)).await,
        StatusCode::BAD_REQUEST,
    );

    // Newest first (`SPEC.md`, "Secrets").
    assert!(default[0].at > default[1].at);
    assert_eq!(default[0].purpose, SecretUsePurpose::Launch);
}

#[tokio::test]
async fn the_uses_of_a_user_secret_follow_the_same_ownership_rule() {
    let app = TestApp::spawn().await;
    let ada = seed_user(&app, "ada", false).await;
    let bob = seed_user(&app, "bob", false).await;
    let admin = seed_user(&app, "admin", true).await;
    let meta = user_secret(&app, &ada, "DEPLOY_TOKEN").await;

    let mut tx = app.pool.begin().await.expect("a transaction begins");
    SecretRepository::new(&app.pool)
        .insert_use(&mut tx, meta.id, None, Some(ada.id), SecretUsePurpose::Git)
        .await
        .expect("the use is recorded");
    tx.commit().await.expect("the transaction commits");

    assert_status(
        service(&app).uses(&actor(&bob), meta.id, None).await,
        StatusCode::FORBIDDEN,
    );

    for caller in [&ada, &admin] {
        let uses = service(&app)
            .uses(&actor(caller), meta.id, None)
            .await
            .expect("the owner and an administrator may read the audit");
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].purpose, SecretUsePurpose::Git);
        assert_eq!(uses[0].user_id, Some(ada.id));
    }
}

#[tokio::test]
async fn a_use_makes_last_used_at_appear_in_the_metadata() {
    let app = TestApp::spawn().await;
    let ada = seed_user(&app, "ada", false).await;
    let meta = user_secret(&app, &ada, "DEPLOY_TOKEN").await;
    assert_eq!(meta.last_used_at, None);

    seed_uses(&app.pool, meta.id, 1).await;

    let listed = service(&app)
        .list(&actor(&ada), Some(SecretScope::User), None)
        .await
        .expect("the listing succeeds");
    assert_eq!(listed.len(), 1);
    assert!(listed[0].last_used_at.is_some());
}

// ---- names ----

#[tokio::test]
async fn a_name_at_the_documented_maximum_round_trips() {
    let app = TestApp::spawn().await;
    let ada = seed_user(&app, "ada", false).await;

    let longest = format!("A{}", "B".repeat(127));
    assert!(SecretName::parse(&longest).is_ok());

    let meta = service(&app)
        .create(
            &actor(&ada),
            create_request(SecretScope::Global, None, &longest),
        )
        .await
        .expect("the longest legal name is accepted");

    assert_eq!(meta.name, longest);
    assert_eq!(opened(&app, &row(&app, meta.id).await), FAKE_VALUE);
}
