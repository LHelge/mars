//! The secrets-manager half of `SecretRepository`: the queries the manager,
//! the launch resolver, the git credential lookup and the rotation sweep ask
//! for (`SPEC.md`, "Secrets"; `ARCHITECTURE.md`, "Secrets").
//!
//! `tests/repositories_secrets.rs` owns the schema epic's round trip — insert,
//! find, rename, delete, orphans — and nothing is repeated here. What this
//! file asserts is the behaviour those queries have that a signature does not
//! show:
//!
//! - a metadata read never carries a ciphertext, and a value sealed by
//!   `secrets::crypto` comes back out of the row byte for byte;
//! - `find_for_update` really holds the row, so a rename cannot interleave
//!   with a value replacement;
//! - the listing filter decides visibility in the `WHERE` clause, including
//!   the empty `Only(vec![])` that may see no user-scoped secret at all;
//! - resolution matches exactly the three scopes of one launch, and no user
//!   scope at all when the session's creator is gone;
//! - the rotation guard refuses a write whose expected version has moved,
//!   which is the concurrent `PUT /secrets/{id}` it exists for.
//!
//! Values are either sealed by the real keyring or obviously fake bytes;
//! nothing here is or resembles key material (`CLAUDE.md`, rule 3).
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use common::TestApp;
use mars_orchestrator::models::{
    NewProject, NewSecret, ScopeRef, Secret, SecretName, SecretScope, SecretUsePurpose, User,
};
use mars_orchestrator::prelude::*;
use mars_orchestrator::repositories::{
    ProjectRepository, SecretListFilter, SecretRepository, UserFilter,
};
use mars_orchestrator::secrets::{SealedSecret, SecretIdentity, WrappedKey};
use uuid::Uuid;

/// Not a real remote: the fixture the project tests use (rule 3).
const TEST_REMOTE: &str = "https://git.example.com/fake/repo.git";

/// The SQLSTATE `FOR UPDATE NOWAIT` raises when the row is already locked.
const LOCK_NOT_AVAILABLE: &str = "55P03";

/// Not key material and not a real envelope: an obviously fake stand-in for
/// the encrypted columns, for the rows no test ever opens. `tag` tells one
/// from another (`CLAUDE.md`, rule 3).
fn fake_sealed(scope: ScopeRef, raw_name: &str, tag: &str, key_version: i32) -> SealedSecret {
    SealedSecret {
        identity: SecretIdentity::new(&scope, &name(raw_name)),
        ciphertext: format!("fake-ciphertext-{tag}").into_bytes(),
        nonce: format!("fake-nonce-{tag}").into_bytes(),
        wrapped: WrappedKey {
            wrapped: format!("fake-wrapped-data-key-{tag}").into_bytes(),
            nonce: format!("fake-wrap-nonce-{tag}").into_bytes(),
            version: key_version,
        },
    }
}

fn name(raw: &str) -> SecretName {
    SecretName::parse(raw).expect("the test secret name is valid")
}

fn new_secret(scope: ScopeRef, raw_name: &str, tag: &str) -> NewSecret {
    NewSecret::new(fake_sealed(scope, raw_name, tag, 1))
}

/// Insert `secret` in its own committed transaction.
async fn insert(pool: &PgPool, secret: &NewSecret) -> Secret {
    let repository = SecretRepository::new(pool);
    let mut tx = pool.begin().await.expect("a transaction begins");
    let inserted = repository
        .insert(&mut tx, secret)
        .await
        .expect("the secret inserts");
    tx.commit().await.expect("the transaction commits");

    inserted
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

/// A committed user for the `user` scope to point at.
async fn seed_user(app: &TestApp, username: &str) -> User {
    app.insert_user(username, &format!("{username}@example.com"), false, false)
        .await
}

#[tokio::test]
async fn a_sealed_value_round_trips_and_its_metadata_never_carries_it() {
    let app = TestApp::spawn().await;
    let repository = SecretRepository::new(&app.pool);

    let ada = seed_user(&app, "ada").await;
    let scope = ScopeRef::user(ada.id);
    let secret_name = name("ANTHROPIC_API_KEY");

    // The real envelope, under the harness's fixed test key.
    let sealed = SealedSecret::seal(
        &app.state.keyring,
        SecretIdentity::new(&scope, &secret_name),
        b"fake-api-key-value",
    )
    .expect("the value seals");

    let mut new = NewSecret::new(sealed);
    new.created_by = Some(ada.id);
    let inserted = insert(&app.pool, &new).await;

    // The stored bytes still open, which is what "the repository moves the
    // columns and nothing else" means.
    let stored = repository
        .find(inserted.id)
        .await
        .unwrap()
        .expect("the secret exists");
    assert_eq!(
        stored
            .sealed()
            .open(&app.state.keyring)
            .expect("the stored row opens")
            .as_slice(),
        b"fake-api-key-value"
    );

    let meta = repository
        .find_meta(inserted.id)
        .await
        .unwrap()
        .expect("the secret exists");
    assert_eq!(meta.id, inserted.id);
    assert_eq!(meta.scope, SecretScope::User);
    assert_eq!(meta.scope_id, Some(ada.id));
    assert_eq!(meta.name, "ANTHROPIC_API_KEY");
    assert!(!meta.orchestrator_only);
    assert_eq!(
        meta.key_version,
        app.state.keyring.current_version() as i32,
        "a new row is wrapped by the newest master key"
    );
    assert_eq!(meta.created_by, Some(ada.id));
    assert_eq!(meta.created_at, inserted.created_at);
    assert_eq!(meta.updated_at, inserted.updated_at);
    // Never used yet: `SPEC.md` promises the field, not a value.
    assert_eq!(meta.last_used_at, None);

    // Nothing in the metadata shape can carry a ciphertext, so the strongest
    // available assertion is that the API shape does not serialise one.
    let rendered = serde_json::to_string(&meta).expect("the metadata serialises");
    assert!(!rendered.contains("ciphertext"), "{rendered}");

    let mut tx = app.pool.begin().await.unwrap();
    let recorded = repository
        .insert_use(
            &mut tx,
            inserted.id,
            None,
            Some(ada.id),
            SecretUsePurpose::Launch,
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let meta = repository
        .find_meta(inserted.id)
        .await
        .unwrap()
        .expect("the secret exists");
    assert_eq!(meta.last_used_at, Some(recorded.at));

    // The `EXISTS` form answers the same question without reading the row.
    assert!(
        repository
            .exists_by_name(&scope, &secret_name)
            .await
            .unwrap()
    );
    assert!(
        !repository
            .exists_by_name(&scope, &name("TOKEN"))
            .await
            .unwrap()
    );
    assert!(
        !repository
            .exists_by_name(&ScopeRef::global(), &secret_name)
            .await
            .unwrap(),
        "the same name at another scope is a different secret"
    );

    assert!(
        repository
            .find_meta(Uuid::new_v4())
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn a_row_read_for_update_is_held_until_the_transaction_ends() {
    let app = TestApp::spawn().await;
    let repository = SecretRepository::new(&app.pool);

    let secret = insert(
        &app.pool,
        &new_secret(ScopeRef::global(), "GIT_CREDENTIAL", "one"),
    )
    .await;

    let mut tx = app.pool.begin().await.unwrap();
    let locked = repository
        .find_for_update(&mut tx, secret.id)
        .await
        .unwrap()
        .expect("the secret exists");
    assert_eq!(locked.id, secret.id);
    assert_eq!(locked.ciphertext, secret.ciphertext);

    // A second writer cannot take the row. `NOWAIT` rather than a timeout, so
    // the assertion is the lock itself and not how long the test waited.
    let contended = sqlx::query("SELECT id FROM secrets WHERE id = $1 FOR UPDATE NOWAIT")
        .bind(secret.id)
        .fetch_one(&app.pool)
        .await
        .expect_err("the row is already locked");
    assert_eq!(
        contended
            .as_database_error()
            .and_then(|error| error.code())
            .as_deref(),
        Some(LOCK_NOT_AVAILABLE),
        "{contended:?}"
    );

    // Rolling back releases it.
    tx.rollback().await.unwrap();
    sqlx::query("SELECT id FROM secrets WHERE id = $1 FOR UPDATE NOWAIT")
        .bind(secret.id)
        .fetch_one(&app.pool)
        .await
        .expect("the lock is gone");

    // A secret that does not exist locks nothing and is not an error.
    let mut tx = app.pool.begin().await.unwrap();
    assert!(
        repository
            .find_for_update(&mut tx, Uuid::new_v4())
            .await
            .unwrap()
            .is_none()
    );
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn a_filtered_listing_narrows_by_scope_and_by_who_may_see_it() {
    let app = TestApp::spawn().await;
    let repository = SecretRepository::new(&app.pool);

    let ada = seed_user(&app, "ada").await;
    let bob = seed_user(&app, "bob").await;
    let project_id = seed_project(&app.pool, "mars").await;

    insert(&app.pool, &new_secret(ScopeRef::global(), "ALPHA", "one")).await;
    insert(
        &app.pool,
        &new_secret(ScopeRef::project(project_id), "BETA", "two"),
    )
    .await;
    insert(
        &app.pool,
        &new_secret(ScopeRef::user(ada.id), "GAMMA", "three"),
    )
    .await;
    insert(
        &app.pool,
        &new_secret(ScopeRef::user(bob.id), "DELTA", "four"),
    )
    .await;

    async fn names(
        repository: &SecretRepository<'_>,
        scope: Option<SecretScope>,
        scope_id: Option<Uuid>,
        user_ids: UserFilter,
    ) -> Vec<String> {
        repository
            .list_meta_filtered(&SecretListFilter {
                scope,
                scope_id,
                user_ids,
            })
            .await
            .unwrap()
            .into_iter()
            .map(|meta| meta.name)
            .collect()
    }

    // Grouped by the enum's order — `global`, `user`, `project` — and by name
    // inside each group.
    assert_eq!(
        names(&repository, None, None, UserFilter::All).await,
        ["ALPHA", "DELTA", "GAMMA", "BETA"]
    );
    // An ordinary user sees the two shared scopes and only their own.
    assert_eq!(
        names(&repository, None, None, UserFilter::Only(vec![ada.id])).await,
        ["ALPHA", "GAMMA", "BETA"]
    );
    // Nobody's user-scoped secrets: a legitimate filter, not an error.
    assert_eq!(
        names(&repository, None, None, UserFilter::Only(vec![])).await,
        ["ALPHA", "BETA"]
    );
    assert_eq!(
        names(&repository, Some(SecretScope::User), None, UserFilter::All).await,
        ["DELTA", "GAMMA"]
    );
    assert_eq!(
        names(
            &repository,
            Some(SecretScope::User),
            None,
            UserFilter::Only(vec![bob.id]),
        )
        .await,
        ["DELTA"]
    );
    assert_eq!(
        names(&repository, None, Some(project_id), UserFilter::All).await,
        ["BETA"]
    );
    assert_eq!(
        names(
            &repository,
            Some(SecretScope::Global),
            None,
            UserFilter::All
        )
        .await,
        ["ALPHA"]
    );
    // The two halves compose rather than override: a scope this id is not in
    // is empty.
    assert!(
        names(
            &repository,
            Some(SecretScope::User),
            Some(project_id),
            UserFilter::All,
        )
        .await
        .is_empty()
    );
}

#[tokio::test]
async fn resolution_returns_the_three_scopes_of_this_launch_and_no_other() {
    let app = TestApp::spawn().await;
    let repository = SecretRepository::new(&app.pool);

    let ada = seed_user(&app, "ada").await;
    let bob = seed_user(&app, "bob").await;
    let project_id = seed_project(&app.pool, "mars").await;
    let other_project_id = seed_project(&app.pool, "phobos").await;

    let global = insert(&app.pool, &new_secret(ScopeRef::global(), "TOKEN", "one")).await;
    let project = insert(
        &app.pool,
        &new_secret(ScopeRef::project(project_id), "TOKEN", "two"),
    )
    .await;
    let user = insert(
        &app.pool,
        &new_secret(ScopeRef::user(ada.id), "TOKEN", "three"),
    )
    .await;
    // None of these belong to the launch.
    insert(
        &app.pool,
        &new_secret(ScopeRef::project(other_project_id), "TOKEN", "four"),
    )
    .await;
    insert(
        &app.pool,
        &new_secret(ScopeRef::user(bob.id), "TOKEN", "five"),
    )
    .await;
    let unrelated = insert(&app.pool, &new_secret(ScopeRef::global(), "OTHER", "six")).await;

    let names = ["TOKEN".to_string()];
    let mut resolved = repository
        .find_for_resolution(&names, project_id, Some(ada.id))
        .await
        .unwrap()
        .into_iter()
        .map(|secret| secret.id)
        .collect::<Vec<_>>();
    resolved.sort();
    let mut expected = vec![global.id, project.id, user.id];
    expected.sort();
    assert_eq!(resolved, expected);
    assert!(!resolved.contains(&unrelated.id));

    // A session whose creator has been deleted has no user scope at all.
    let mut resolved = repository
        .find_for_resolution(&names, project_id, None)
        .await
        .unwrap()
        .into_iter()
        .map(|secret| secret.id)
        .collect::<Vec<_>>();
    resolved.sort();
    let mut expected = vec![global.id, project.id];
    expected.sort();
    assert_eq!(resolved, expected);

    // Several names in one query, and a name with no row at any scope simply
    // has no candidate — the resolver turns that into a `launch_warning`.
    let names = [
        "TOKEN".to_string(),
        "OTHER".to_string(),
        "ABSENT".to_string(),
    ];
    let resolved = repository
        .find_for_resolution(&names, project_id, Some(ada.id))
        .await
        .unwrap();
    assert_eq!(resolved.len(), 4);
    assert!(resolved.iter().any(|secret| secret.id == unrelated.id));

    // An empty profile asks for nothing.
    assert!(
        repository
            .find_for_resolution(&[], project_id, Some(ada.id))
            .await
            .unwrap()
            .is_empty()
    );
    // A project with no secrets of its own still sees the global ones.
    let resolved = repository
        .find_for_resolution(&["TOKEN".to_string()], Uuid::new_v4(), None)
        .await
        .unwrap();
    assert_eq!(
        resolved.iter().map(|secret| secret.id).collect::<Vec<_>>(),
        [global.id]
    );
}

#[tokio::test]
async fn a_rotation_write_with_a_stale_expected_version_changes_nothing() {
    let app = TestApp::spawn().await;
    let repository = SecretRepository::new(&app.pool);
    let scope = ScopeRef::global();

    let mut older = new_secret(scope, "OLD_ONE", "one");
    older.sealed.wrapped.version = 1;
    let older = insert(&app.pool, &older).await;

    let mut second = new_secret(scope, "OLD_TWO", "two");
    second.sealed.wrapped.version = 1;
    let second = insert(&app.pool, &second).await;

    let mut newest = new_secret(scope, "NEWEST", "three");
    newest.sealed.wrapped.version = 2;
    let newest = insert(&app.pool, &newest).await;

    // The wrapping a rotation sweep would write: the same row, its data key
    // wrapped under a newer master key. Fake bytes, like every envelope in
    // this file (rule 3).
    let rewrapped = fake_sealed(scope, "OLD_ONE", "rewrapped", 3);
    let stale = fake_sealed(scope, "OLD_ONE", "stale", 3);

    assert_eq!(repository.count_below_version(1).await.unwrap(), 0);
    assert_eq!(repository.count_below_version(2).await.unwrap(), 2);
    assert_eq!(repository.count_below_version(3).await.unwrap(), 3);

    // The sweep read version 1, but a `PUT /secrets/{id}` replaced the value —
    // and with it the data key — in between. The guarded write matches nothing
    // rather than putting the old data key's wrapping over the new one.
    let mut tx = app.pool.begin().await.unwrap();
    assert!(
        !repository
            .rewrap(&mut tx, older.id, 99, &rewrapped)
            .await
            .unwrap(),
        "a stale expected version is skipped, not an error"
    );
    tx.commit().await.unwrap();

    let untouched = repository.find(older.id).await.unwrap().unwrap();
    assert_eq!(untouched.data_key_wrapped, older.data_key_wrapped);
    assert_eq!(untouched.data_key_nonce, older.data_key_nonce);
    assert_eq!(untouched.key_version, 1);

    // The same write with the version the row still has does land.
    let mut tx = app.pool.begin().await.unwrap();
    assert!(
        repository
            .rewrap(&mut tx, older.id, 1, &rewrapped)
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();

    let rotated = repository.find(older.id).await.unwrap().unwrap();
    assert_eq!(rotated.data_key_wrapped, rewrapped.wrapped.wrapped);
    assert_eq!(rotated.key_version, 3);
    assert_eq!(
        rotated.ciphertext, older.ciphertext,
        "the value is untouched"
    );

    // And a retry of the same batch row is now itself stale.
    let mut tx = app.pool.begin().await.unwrap();
    assert!(
        !repository
            .rewrap(&mut tx, older.id, 1, &stale)
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();
    assert_eq!(
        repository
            .find(older.id)
            .await
            .unwrap()
            .unwrap()
            .data_key_wrapped,
        rewrapped.wrapped.wrapped
    );

    assert_eq!(repository.count_below_version(3).await.unwrap(), 2);
    assert_eq!(
        repository
            .list_for_rotation(3, 100)
            .await
            .unwrap()
            .iter()
            .map(|secret| secret.id)
            .collect::<Vec<_>>(),
        [second.id, newest.id],
        "the two rows still below version 3, oldest key first"
    );
}

#[tokio::test]
async fn a_scope_exists_only_while_its_target_row_does() {
    let app = TestApp::spawn().await;
    let repository = SecretRepository::new(&app.pool);

    let ada = seed_user(&app, "ada").await;
    let project_id = seed_project(&app.pool, "mars").await;

    // No target, no query, always true.
    assert!(repository.scope_exists(&ScopeRef::global()).await.unwrap());
    assert!(
        repository
            .scope_exists(&ScopeRef::user(ada.id))
            .await
            .unwrap()
    );
    assert!(
        repository
            .scope_exists(&ScopeRef::project(project_id))
            .await
            .unwrap()
    );

    // Each scope looks in its own table, so a project id is not a user.
    assert!(
        !repository
            .scope_exists(&ScopeRef::user(project_id))
            .await
            .unwrap()
    );
    assert!(
        !repository
            .scope_exists(&ScopeRef::project(ada.id))
            .await
            .unwrap()
    );
    assert!(
        !repository
            .scope_exists(&ScopeRef::user(Uuid::new_v4()))
            .await
            .unwrap()
    );

    // `scope_id` has no foreign key, so the secret outlives its user; the
    // scope does not.
    let orphaned = insert(
        &app.pool,
        &new_secret(ScopeRef::user(ada.id), "TOKEN", "one"),
    )
    .await;
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(ada.id)
        .execute(&app.pool)
        .await
        .expect("the user deletes");

    assert!(
        !repository
            .scope_exists(&ScopeRef::user(ada.id))
            .await
            .unwrap()
    );
    assert!(
        repository.find(orphaned.id).await.unwrap().is_some(),
        "the row is the reaper's, not the foreign key's"
    );
}

#[tokio::test]
async fn a_useless_use_limit_is_clamped_to_one_row() {
    let app = TestApp::spawn().await;
    let repository = SecretRepository::new(&app.pool);

    let secret = insert(&app.pool, &new_secret(ScopeRef::global(), "TOKEN", "one")).await;

    let mut tx = app.pool.begin().await.unwrap();
    for _ in 0..2 {
        repository
            .insert_use(&mut tx, secret.id, None, None, SecretUsePurpose::Launch)
            .await
            .unwrap();
    }
    let newest = repository
        .insert_use(&mut tx, secret.id, None, None, SecretUsePurpose::Git)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(repository.list_uses(secret.id, 10).await.unwrap().len(), 3);

    // Zero and negative are the caller's bug: one row, the newest, rather than
    // an empty list that reads as "never used".
    for limit in [0, -5] {
        let uses = repository.list_uses(secret.id, limit).await.unwrap();
        assert_eq!(uses.len(), 1, "limit {limit}");
        assert_eq!(uses[0].id, newest.id, "limit {limit}");
    }
}
