//! `SecretRepository` against a real Postgres (`CLAUDE.md`, "Testing
//! expectations").
//!
//! The CRUD half is a round trip and the conflict paths; the interesting half
//! is the three things `docs/data-model.md`, "Secrets" and `ARCHITECTURE.md`,
//! "Secrets" promise that a functional assertion alone would not catch:
//!
//! - the unique index is `NULLS NOT DISTINCT`, so two `global` secrets of the
//!   same name collide even though both have a NULL `scope_id`;
//! - rotation touches the wrapping and never the ciphertext;
//! - `scope_id` has no foreign key, so a deleted user leaves orphans behind
//!   rather than cascading, and the reaper has to be able to find them.
//!
//! Every byte value here is obviously fake (`CLAUDE.md`, rule 3); nothing in
//! this file is, or resembles, key material.
//!
//! Needs a container engine; see `tests/common/db.rs`.

mod common;

use axum::http::StatusCode;
use mars_orchestrator::models::{
    NewSecret, ScopeRef, Secret, SecretName, SecretScope, SecretUsePurpose,
};
use mars_orchestrator::prelude::*;
use mars_orchestrator::repositories::SecretRepository;
use mars_orchestrator::secrets::{SealedSecret, SecretIdentity, WrappedKey};
use uuid::Uuid;

/// The administrator the `users` migration seeds (`docs/data-model.md`).
const SEEDED_ADMIN: Uuid = Uuid::from_u128(1);

/// Not key material and not a real envelope: an obviously fake stand-in for
/// the encrypted columns. `tag` distinguishes one fake value from another so a
/// test can tell which one it is looking at (`CLAUDE.md`, rule 3). Nothing in
/// this file opens a row, so nothing in it needs a keyring.
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

/// Seed a user row directly.
///
/// `ProjectRepository` and the rest of `UserRepository`'s callers are being
/// written in parallel, and these tests only need a row for `scope_id` to
/// point at, so the statements are unchecked inserts rather than a dependency
/// on another aggregate's repository. The credentials are obviously fake
/// (rule 3).
async fn seed_user(pool: &PgPool, username: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, username, email, password_hash) VALUES ($1, $2, $3, $4)")
        .bind(id)
        .bind(username)
        .bind(format!("{username}@example.com"))
        .bind("$argon2id$fake$hash")
        .execute(pool)
        .await
        .expect("the seeded user inserts");

    id
}

/// Seed a project row directly; see [`seed_user`] for why this is unchecked.
async fn seed_project(pool: &PgPool, name: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id, name, remote_url) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(name)
        .bind("https://git.example.com/fake/repo.git")
        .execute(pool)
        .await
        .expect("the seeded project inserts");

    id
}

#[tokio::test]
async fn a_secret_survives_an_insert_find_list_update_delete_round_trip() {
    let (_postgres, pool) = common::db::test_pool().await;
    let repository = SecretRepository::new(&pool);

    let project_id = seed_project(&pool, "mars").await;
    let scope = ScopeRef::project(project_id);

    let mut new = new_secret(scope, "GIT_CREDENTIAL", "one");
    new.orchestrator_only = true;
    new.created_by = Some(SEEDED_ADMIN);
    let inserted = insert(&pool, &new).await;

    assert_eq!(inserted.id, new.id);
    assert_eq!(inserted.scope, SecretScope::Project);
    assert_eq!(inserted.scope_id, Some(project_id));
    assert_eq!(inserted.name, "GIT_CREDENTIAL");
    assert_eq!(inserted.ciphertext, b"fake-ciphertext-one");
    assert_eq!(inserted.nonce, b"fake-nonce-one");
    assert_eq!(inserted.data_key_wrapped, b"fake-wrapped-data-key-one");
    assert_eq!(inserted.data_key_nonce, b"fake-wrap-nonce-one");
    assert_eq!(inserted.key_version, 1);
    assert!(inserted.orchestrator_only);
    assert_eq!(inserted.created_by, Some(SEEDED_ADMIN));
    assert_eq!(inserted.scope_ref(), scope);

    assert_eq!(
        repository.find(new.id).await.unwrap().as_ref(),
        Some(&inserted)
    );
    assert_eq!(
        repository
            .find_by_name(&scope, &name("GIT_CREDENTIAL"))
            .await
            .unwrap()
            .as_ref(),
        Some(&inserted)
    );
    assert!(repository.find(Uuid::new_v4()).await.unwrap().is_none());
    // The same name in a different scope is a different secret.
    assert!(
        repository
            .find_by_name(&ScopeRef::global(), &name("GIT_CREDENTIAL"))
            .await
            .unwrap()
            .is_none()
    );

    // Replacing the value moves all five encrypted fields and `updated_at`.
    let replacement = fake_sealed(scope, "GIT_CREDENTIAL", "two", 2);
    let mut tx = pool.begin().await.unwrap();
    let updated = repository
        .update_value(&mut tx, new.id, &replacement)
        .await
        .unwrap()
        .expect("the secret exists");
    tx.commit().await.unwrap();

    assert_eq!(updated.ciphertext, b"fake-ciphertext-two");
    assert_eq!(updated.nonce, b"fake-nonce-two");
    assert_eq!(updated.data_key_wrapped, b"fake-wrapped-data-key-two");
    assert_eq!(updated.data_key_nonce, b"fake-wrap-nonce-two");
    assert_eq!(updated.key_version, 2);
    assert_eq!(updated.name, "GIT_CREDENTIAL");
    assert_eq!(updated.created_at, inserted.created_at);
    assert!(updated.updated_at >= inserted.updated_at);

    // Renaming writes the new name and the re-encrypted value together, and
    // the name comes from the envelope's own identity.
    let renamed_value = fake_sealed(scope, "FORGE_TOKEN", "three", 2);
    let mut tx = pool.begin().await.unwrap();
    let renamed = repository
        .rename(&mut tx, new.id, &renamed_value)
        .await
        .unwrap()
        .expect("the secret exists");
    tx.commit().await.unwrap();

    assert_eq!(renamed.name, "FORGE_TOKEN");
    assert_eq!(renamed.ciphertext, b"fake-ciphertext-three");
    assert_eq!(renamed.data_key_wrapped, b"fake-wrapped-data-key-three");
    assert!(
        repository
            .find_by_name(&scope, &name("GIT_CREDENTIAL"))
            .await
            .unwrap()
            .is_none()
    );

    // The flag moves on its own, without re-encrypting anything.
    let mut tx = pool.begin().await.unwrap();
    let flagged = repository
        .set_orchestrator_only(&mut tx, new.id, false)
        .await
        .unwrap()
        .expect("the secret exists");
    tx.commit().await.unwrap();

    assert!(!flagged.orchestrator_only);
    assert_eq!(flagged.ciphertext, renamed.ciphertext);

    let mut tx = pool.begin().await.unwrap();
    assert!(repository.delete(&mut tx, new.id).await.unwrap());
    // A second delete matches nothing rather than failing.
    assert!(!repository.delete(&mut tx, new.id).await.unwrap());
    tx.commit().await.unwrap();

    assert!(repository.find(new.id).await.unwrap().is_none());
}

#[tokio::test]
async fn a_missing_secret_is_none_rather_than_an_error_on_every_write() {
    let (_postgres, pool) = common::db::test_pool().await;
    let repository = SecretRepository::new(&pool);
    let missing = Uuid::new_v4();

    let mut tx = pool.begin().await.unwrap();
    assert!(
        repository
            .update_value(
                &mut tx,
                missing,
                &fake_sealed(ScopeRef::global(), "TOKEN", "one", 1),
            )
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        repository
            .rename(
                &mut tx,
                missing,
                &fake_sealed(ScopeRef::global(), "TOKEN", "one", 1),
            )
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        repository
            .set_orchestrator_only(&mut tx, missing, true)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        !repository
            .rewrap(
                &mut tx,
                missing,
                1,
                &fake_sealed(ScopeRef::global(), "TOKEN", "one", 2),
            )
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn a_duplicate_name_in_the_same_scope_is_a_conflict() {
    let (_postgres, pool) = common::db::test_pool().await;
    let repository = SecretRepository::new(&pool);

    let user_id = seed_user(&pool, "ada").await;
    let scope = ScopeRef::user(user_id);
    insert(&pool, &new_secret(scope, "TOKEN", "one")).await;

    let mut tx = pool.begin().await.unwrap();
    let error = repository
        .insert(&mut tx, &new_secret(scope, "TOKEN", "two"))
        .await
        .expect_err("a duplicate name in one scope is rejected");

    assert_eq!(error.status(), StatusCode::CONFLICT);
    assert_eq!(error.to_string(), "secret already exists");
    assert!(matches!(error, Error::Conflict(_)), "{error:?}");
}

#[tokio::test]
async fn two_global_secrets_of_the_same_name_collide_although_scope_id_is_null() {
    // `UNIQUE NULLS NOT DISTINCT`: with the default nulls-distinct behaviour
    // the NULL `scope_id` would let duplicates in (`docs/data-model.md`,
    // `secrets`).
    let (_postgres, pool) = common::db::test_pool().await;
    let repository = SecretRepository::new(&pool);
    let scope = ScopeRef::global();

    let first = insert(&pool, &new_secret(scope, "TOKEN", "one")).await;
    assert_eq!(first.scope_id, None);

    let mut tx = pool.begin().await.unwrap();
    let error = repository
        .insert(&mut tx, &new_secret(scope, "TOKEN", "two"))
        .await
        .expect_err("a second global TOKEN is rejected");
    assert_eq!(error.status(), StatusCode::CONFLICT);
    drop(tx);

    // The same name at another scope is fine, and a global lookup still finds
    // the NULL-`scope_id` row.
    let user_id = seed_user(&pool, "ada").await;
    insert(
        &pool,
        &new_secret(ScopeRef::user(user_id), "TOKEN", "three"),
    )
    .await;
    assert_eq!(
        repository
            .find_by_name(&scope, &name("TOKEN"))
            .await
            .unwrap()
            .map(|secret| secret.id),
        Some(first.id)
    );
}

#[tokio::test]
async fn renaming_onto_a_taken_name_is_a_conflict() {
    let (_postgres, pool) = common::db::test_pool().await;
    let repository = SecretRepository::new(&pool);
    let scope = ScopeRef::global();

    let first = insert(&pool, &new_secret(scope, "TOKEN", "one")).await;
    insert(&pool, &new_secret(scope, "FORGE_TOKEN", "two")).await;

    let mut tx = pool.begin().await.unwrap();
    let error = repository
        .rename(
            &mut tx,
            first.id,
            &fake_sealed(scope, "FORGE_TOKEN", "one", 1),
        )
        .await
        .expect_err("renaming onto a taken name is rejected");

    assert_eq!(error.status(), StatusCode::CONFLICT);
    assert_eq!(error.to_string(), "secret already exists");
}

#[tokio::test]
async fn renaming_to_the_same_name_writes_exactly_what_it_is_given() {
    // The repository does not decide whether a rename is a no-op: it writes
    // the name and the value it was handed.
    let (_postgres, pool) = common::db::test_pool().await;
    let repository = SecretRepository::new(&pool);

    let secret = insert(&pool, &new_secret(ScopeRef::global(), "TOKEN", "one")).await;

    let mut tx = pool.begin().await.unwrap();
    let renamed = repository
        .rename(
            &mut tx,
            secret.id,
            &fake_sealed(ScopeRef::global(), "TOKEN", "two", 1),
        )
        .await
        .unwrap()
        .expect("the secret exists");
    tx.commit().await.unwrap();

    assert_eq!(renamed.name, "TOKEN");
    assert_eq!(renamed.ciphertext, b"fake-ciphertext-two");
}

#[tokio::test]
async fn list_meta_is_by_name_and_carries_the_last_use() {
    let (_postgres, pool) = common::db::test_pool().await;
    let repository = SecretRepository::new(&pool);

    let project_id = seed_project(&pool, "mars").await;
    let scope = ScopeRef::project(project_id);

    // An empty scope is an empty vector, not a 404.
    assert!(repository.list_meta(&scope).await.unwrap().is_empty());
    assert!(
        repository
            .list_meta(&ScopeRef::global())
            .await
            .unwrap()
            .is_empty()
    );

    let zulu = insert(&pool, &new_secret(scope, "ZULU", "one")).await;
    let mut alpha_new = new_secret(scope, "ALPHA", "two");
    alpha_new.orchestrator_only = true;
    alpha_new.created_by = Some(SEEDED_ADMIN);
    let alpha = insert(&pool, &alpha_new).await;
    // A secret in another scope must not appear in this listing.
    insert(&pool, &new_secret(ScopeRef::global(), "ALPHA", "three")).await;

    let listed = repository.list_meta(&scope).await.unwrap();
    assert_eq!(
        listed
            .iter()
            .map(|meta| meta.name.as_str())
            .collect::<Vec<_>>(),
        ["ALPHA", "ZULU"]
    );
    // Never used yet.
    assert!(listed.iter().all(|meta| meta.last_used_at.is_none()));

    let first = &listed[0];
    assert_eq!(first.id, alpha.id);
    assert_eq!(first.scope, SecretScope::Project);
    assert_eq!(first.scope_id, Some(project_id));
    assert!(first.orchestrator_only);
    assert_eq!(first.key_version, 1);
    assert_eq!(first.created_by, Some(SEEDED_ADMIN));
    assert_eq!(first.created_at, alpha.created_at);
    assert_eq!(first.updated_at, alpha.updated_at);

    // Two uses of ZULU: `last_used_at` is the newest of them.
    let mut tx = pool.begin().await.unwrap();
    let older = repository
        .insert_use(&mut tx, zulu.id, None, None, SecretUsePurpose::Launch)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let mut tx = pool.begin().await.unwrap();
    let newer = repository
        .insert_use(
            &mut tx,
            zulu.id,
            None,
            Some(SEEDED_ADMIN),
            SecretUsePurpose::Git,
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert!(newer.at >= older.at);

    let listed = repository.list_meta(&scope).await.unwrap();
    assert_eq!(listed[0].last_used_at, None, "ALPHA was never used");
    assert_eq!(listed[1].last_used_at, Some(newer.at));
}

#[tokio::test]
async fn uses_are_newest_first_and_accept_every_documented_shape() {
    let (_postgres, pool) = common::db::test_pool().await;
    let repository = SecretRepository::new(&pool);

    let secret = insert(&pool, &new_secret(ScopeRef::global(), "TOKEN", "one")).await;
    let other = insert(&pool, &new_secret(ScopeRef::global(), "FORGE_TOKEN", "two")).await;
    let user_id = seed_user(&pool, "ada").await;

    assert!(
        repository
            .list_uses(secret.id, 10)
            .await
            .unwrap()
            .is_empty()
    );

    let mut tx = pool.begin().await.unwrap();
    // The mirror-fetch job: neither a session nor a user.
    let anonymous = repository
        .insert_use(&mut tx, secret.id, None, None, SecretUsePurpose::Git)
        .await
        .unwrap();
    // The REST git path: a user and no session.
    let by_user = repository
        .insert_use(
            &mut tx,
            secret.id,
            None,
            Some(user_id),
            SecretUsePurpose::Launch,
        )
        .await
        .unwrap();
    repository
        .insert_use(&mut tx, other.id, None, None, SecretUsePurpose::Launch)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(anonymous.secret_id, secret.id);
    assert_eq!(anonymous.session_id, None);
    assert_eq!(anonymous.user_id, None);
    assert_eq!(anonymous.purpose, SecretUsePurpose::Git);
    assert_eq!(by_user.user_id, Some(user_id));
    assert_eq!(by_user.purpose, SecretUsePurpose::Launch);

    // Both rows share `NOW()` inside one transaction, so `id DESC` decides:
    // newest first.
    let uses = repository.list_uses(secret.id, 10).await.unwrap();
    assert_eq!(
        uses.iter().map(|use_| use_.id).collect::<Vec<_>>(),
        [by_user.id, anonymous.id],
    );
    // Scoped to one secret.
    assert_eq!(repository.list_uses(other.id, 10).await.unwrap().len(), 1);

    let uses = repository.list_uses(secret.id, 1).await.unwrap();
    assert_eq!(uses.len(), 1);
    assert_eq!(uses[0].id, by_user.id);

    // Deleting the secret cascades its audit rows.
    let mut tx = pool.begin().await.unwrap();
    assert!(repository.delete(&mut tx, secret.id).await.unwrap());
    tx.commit().await.unwrap();
    assert!(
        repository
            .list_uses(secret.id, 10)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn rotation_selects_only_older_rows_and_rewraps_without_touching_the_ciphertext() {
    let (_postgres, pool) = common::db::test_pool().await;
    let repository = SecretRepository::new(&pool);
    let scope = ScopeRef::global();

    assert!(repository.distinct_key_versions().await.unwrap().is_empty());

    let mut old_one = new_secret(scope, "OLD_ONE", "one");
    old_one.sealed.wrapped.version = 1;
    let old_one = insert(&pool, &old_one).await;

    let mut old_two = new_secret(scope, "OLD_TWO", "two");
    old_two.sealed.wrapped.version = 2;
    let old_two = insert(&pool, &old_two).await;

    let mut newest = new_secret(scope, "NEWEST", "three");
    newest.sealed.wrapped.version = 3;
    let newest = insert(&pool, &newest).await;

    // The wrapping a rotation sweep would write: fake bytes under version 3.
    let rewrapped = fake_sealed(scope, "OLD_ONE", "rewrapped", 3);

    assert_eq!(repository.distinct_key_versions().await.unwrap(), [1, 2, 3]);

    // Only rows below the newest version, oldest key first.
    let batch = repository.list_for_rotation(3, 100).await.unwrap();
    assert_eq!(
        batch.iter().map(|secret| secret.id).collect::<Vec<_>>(),
        [old_one.id, old_two.id]
    );
    // The limit bounds the batch.
    let batch = repository.list_for_rotation(3, 1).await.unwrap();
    assert_eq!(batch.len(), 1);
    assert_eq!(batch[0].id, old_one.id);
    // Nothing is below the oldest version.
    assert!(
        repository
            .list_for_rotation(1, 100)
            .await
            .unwrap()
            .is_empty()
    );

    let mut tx = pool.begin().await.unwrap();
    assert!(
        repository
            .rewrap(&mut tx, old_one.id, 1, &rewrapped)
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();

    let rotated = repository.find(old_one.id).await.unwrap().unwrap();
    assert_eq!(rotated.data_key_wrapped, rewrapped.wrapped.wrapped);
    assert_eq!(rotated.data_key_nonce, rewrapped.wrapped.nonce);
    assert_eq!(rotated.key_version, 3);
    // The value itself is untouched (`ARCHITECTURE.md`, "Secrets", Rotation).
    assert_eq!(rotated.ciphertext, old_one.ciphertext);
    assert_eq!(rotated.nonce, old_one.nonce);
    assert_eq!(rotated.name, old_one.name);
    assert!(rotated.updated_at >= old_one.updated_at);

    // The sweep converges: one row left below version 3, then none.
    let batch = repository.list_for_rotation(3, 100).await.unwrap();
    assert_eq!(batch.iter().map(|s| s.id).collect::<Vec<_>>(), [old_two.id]);

    let mut tx = pool.begin().await.unwrap();
    repository
        .rewrap(&mut tx, old_two.id, 2, &rewrapped)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert!(
        repository
            .list_for_rotation(3, 100)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(repository.distinct_key_versions().await.unwrap(), [3]);
    assert_eq!(
        repository
            .find(newest.id)
            .await
            .unwrap()
            .unwrap()
            .key_version,
        3
    );
}

#[tokio::test]
async fn orphans_are_the_scoped_rows_whose_target_is_gone() {
    // `scope_id` has no foreign key, so deleting a user or a project leaves
    // its secrets behind for the reaper (`docs/data-model.md`, `secrets`).
    let (_postgres, pool) = common::db::test_pool().await;
    let repository = SecretRepository::new(&pool);

    let user_id = seed_user(&pool, "ada").await;
    let project_id = seed_project(&pool, "mars").await;

    let global = insert(&pool, &new_secret(ScopeRef::global(), "TOKEN", "one")).await;
    let owned = insert(&pool, &new_secret(ScopeRef::user(user_id), "TOKEN", "two")).await;
    let project = insert(
        &pool,
        &new_secret(ScopeRef::project(project_id), "TOKEN", "three"),
    )
    .await;

    assert!(repository.list_orphans().await.unwrap().is_empty());

    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(user_id)
        .execute(&pool)
        .await
        .expect("the user deletes");

    // The secret survived its user, with `created_by` nulled out by its own
    // foreign key.
    let survivor = repository.find(owned.id).await.unwrap().unwrap();
    assert_eq!(survivor.scope_id, Some(user_id));
    assert_eq!(repository.list_orphans().await.unwrap(), [owned.id]);

    sqlx::query("DELETE FROM projects WHERE id = $1")
        .bind(project_id)
        .execute(&pool)
        .await
        .expect("the project deletes");

    let mut orphans = repository.list_orphans().await.unwrap();
    orphans.sort();
    let mut expected = vec![owned.id, project.id];
    expected.sort();
    assert_eq!(orphans, expected);
    // A global secret has no target and is never an orphan.
    assert!(!orphans.contains(&global.id));
}
