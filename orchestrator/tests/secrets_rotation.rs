//! The master-key rotation sweep (`ARCHITECTURE.md`, "Secrets", Rotation).
//!
//! `secrets::rewrap_outdated` is what `rotate-secrets` and the hourly cron job
//! both run, so what these tests assert is the whole contract of a rotation:
//!
//! - every row behind the newest key ends up on it, across batch boundaries;
//! - the values are not touched — `ciphertext` and `nonce` come out byte for
//!   byte and every value still opens, under the new wrapping;
//! - a row another writer moved between the batch and the write keeps *its*
//!   wrapping and is counted as skipped, never overwritten;
//! - a row under a version the keyring does not carry is skipped rather than
//!   selected forever, and is exactly what `remaining` reports afterwards;
//! - a second run, and a keyring with nothing to rotate, do nothing at all.
//!
//! The keyrings here are built from repeated bytes and the values are
//! `fake-value-…`: nothing in this file is or resembles a real key or a real
//! secret (`CLAUDE.md`, rule 3). The harness's own keyring (`app.state.keyring`)
//! is deliberately unused — a rotation needs two versions of its own.
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use common::TestApp;
use mars_orchestrator::models::{EncryptedValue, NewSecret, ScopeRef, Secret, SecretName};
use mars_orchestrator::prelude::*;
use mars_orchestrator::repositories::SecretRepository;
use mars_orchestrator::secrets::{
    MASTER_KEY_LEN, RotationReport, SecretsKeyring, aad_for, open, rewrap_outdated, seal,
};
use uuid::Uuid;

/// Obviously not a real master key: one repeated byte per version (rule 3).
fn fake_master_key(byte: u8) -> [u8; MASTER_KEY_LEN] {
    [byte; MASTER_KEY_LEN]
}

/// A keyring carrying exactly `versions`; the highest is the one new wrappings
/// use.
fn keyring(versions: &[u32]) -> SecretsKeyring {
    let entries = versions
        .iter()
        .map(|version| (*version, fake_master_key(0xA0 + *version as u8)))
        .collect();

    SecretsKeyring::from_entries(entries).expect("the test keyring is valid")
}

/// Not key material: an obviously fake stand-in for the four encrypted columns,
/// for a row no test ever opens.
fn fake_value(tag: &str, key_version: i32) -> EncryptedValue {
    EncryptedValue {
        ciphertext: format!("fake-ciphertext-{tag}").into_bytes(),
        nonce: format!("fake-nonce-{tag}").into_bytes(),
        data_key_wrapped: format!("fake-wrapped-data-key-{tag}").into_bytes(),
        data_key_nonce: format!("fake-wrap-nonce-{tag}").into_bytes(),
        key_version,
    }
}

/// The plaintext row `index` of a seeded batch carries.
fn plaintext(prefix: &str, index: usize) -> String {
    format!("fake-value-{prefix}-{index}")
}

fn name(prefix: &str, index: usize) -> SecretName {
    SecretName::parse(&format!("{prefix}_{index:04}")).expect("the test secret name is valid")
}

/// Seal `count` global secrets under `keyring` and insert them in one
/// transaction, returning the stored rows in insertion order.
async fn seed(pool: &PgPool, keyring: &SecretsKeyring, prefix: &str, count: usize) -> Vec<Secret> {
    let repository = SecretRepository::new(pool);
    let mut tx = pool.begin().await.expect("a transaction begins");

    let mut rows = Vec::with_capacity(count);
    for index in 0..count {
        let scope = ScopeRef::global();
        let secret_name = name(prefix, index);
        let aad = aad_for(&scope, &secret_name);
        let sealed =
            seal(keyring, &aad, plaintext(prefix, index).as_bytes()).expect("the test value seals");

        let new = NewSecret::new(scope, secret_name, sealed);
        rows.push(
            repository
                .insert(&mut tx, &new)
                .await
                .expect("the secret inserts"),
        );
    }

    tx.commit().await.expect("the transaction commits");

    rows
}

/// Insert one row whose columns are given verbatim.
async fn insert_raw(pool: &PgPool, raw_name: &str, value: EncryptedValue) -> Secret {
    let repository = SecretRepository::new(pool);
    let new = NewSecret::new(
        ScopeRef::global(),
        SecretName::parse(raw_name).expect("the test secret name is valid"),
        value,
    );

    let mut tx = pool.begin().await.expect("a transaction begins");
    let inserted = repository
        .insert(&mut tx, &new)
        .await
        .expect("the secret inserts");
    tx.commit().await.expect("the transaction commits");

    inserted
}

async fn reread(pool: &PgPool, id: Uuid) -> Secret {
    SecretRepository::new(pool)
        .find(id)
        .await
        .expect("the row reads")
        .expect("the row is still there")
}

/// The four encrypted columns of a stored row, as `crypto::open` wants them.
fn value_of(row: &Secret) -> EncryptedValue {
    EncryptedValue {
        ciphertext: row.ciphertext.clone(),
        nonce: row.nonce.clone(),
        data_key_wrapped: row.data_key_wrapped.clone(),
        data_key_nonce: row.data_key_nonce.clone(),
        key_version: row.key_version,
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[tokio::test]
async fn a_sweep_moves_every_row_onto_the_newest_key_and_leaves_the_values_alone() {
    let app = TestApp::spawn().await;
    let old = keyring(&[1]);
    let current = keyring(&[1, 2]);

    // Two and a half batches, so the loop has to come back for more.
    let before = seed(&app.pool, &old, "ROT", 250).await;

    let report = rewrap_outdated(&app.pool, &current)
        .await
        .expect("the sweep runs");
    assert_eq!(
        report,
        RotationReport {
            rewrapped: 250,
            skipped: 0,
            remaining: 0,
        }
    );

    for (index, row) in before.iter().enumerate() {
        assert_eq!(row.key_version, 1, "the rows were sealed under version 1");

        let after = reread(&app.pool, row.id).await;
        assert_eq!(after.key_version, 2, "every row is on the newest key");
        assert_ne!(
            after.data_key_wrapped, row.data_key_wrapped,
            "the data key is wrapped again, under a different master key"
        );
        assert_ne!(after.data_key_nonce, row.data_key_nonce, "a fresh nonce");

        // The whole point of envelope encryption: rotating the master key does
        // not re-encrypt a single value.
        assert_eq!(
            after.ciphertext, row.ciphertext,
            "the ciphertext is untouched"
        );
        assert_eq!(after.nonce, row.nonce, "the value's nonce is untouched");

        let opened = open(
            &current,
            &aad_for(&after.scope_ref(), &name("ROT", index)),
            &value_of(&after),
        )
        .expect("the value still opens after the rotation");
        assert_eq!(opened.as_slice(), plaintext("ROT", index).as_bytes());
    }

    // Idempotent: there is nothing left to select.
    let again = rewrap_outdated(&app.pool, &current)
        .await
        .expect("the second sweep runs");
    assert_eq!(again, RotationReport::default());
}

#[tokio::test]
async fn a_sweep_finishes_on_a_batch_boundary_and_one_row_past_it() {
    let app = TestApp::spawn().await;
    let old = keyring(&[1]);
    let current = keyring(&[1, 2]);

    // Exactly one full batch: the second select has to come back empty.
    let first = seed(&app.pool, &old, "HUNDRED", 100).await;
    let report = rewrap_outdated(&app.pool, &current)
        .await
        .expect("the sweep runs");
    assert_eq!(
        report,
        RotationReport {
            rewrapped: 100,
            skipped: 0,
            remaining: 0,
        }
    );

    // One row more than a batch, with the first hundred already current, so
    // the pending set is exactly 101.
    let second = seed(&app.pool, &old, "HUNDREDONE", 101).await;
    let report = rewrap_outdated(&app.pool, &current)
        .await
        .expect("the sweep runs");
    assert_eq!(
        report,
        RotationReport {
            rewrapped: 101,
            skipped: 0,
            remaining: 0,
        }
    );

    for row in first.iter().chain(second.iter()) {
        assert_eq!(
            reread(&app.pool, row.id).await.key_version,
            2,
            "every row from either wave ends on the newest version"
        );
    }
}

#[tokio::test]
async fn a_row_re_wrapped_by_someone_else_mid_sweep_is_skipped_and_keeps_its_wrapping() {
    let app = TestApp::spawn().await;
    let old = keyring(&[1]);
    let current = keyring(&[1, 2]);

    let rows = seed(&app.pool, &old, "RACE", 3).await;

    // The row the concurrent writer takes: the last one the sweep reaches,
    // since `list_for_rotation` orders a batch by `key_version` then `id`.
    let target = rows
        .iter()
        .max_by_key(|row| row.id)
        .expect("three rows were seeded")
        .clone();

    // What that writer stores: the same data key, wrapped under version 2 —
    // exactly what a `PUT /secrets/{id}` or a competing sweep would write.
    let data_key = current
        .unwrap_data_key(&target.data_key_wrapped, &target.data_key_nonce, 1)
        .expect("the seeded row unwraps under version 1");
    let staged = current
        .wrap_data_key(&data_key)
        .expect("the data key wraps");
    drop(data_key);

    // A trigger is how the interleaving is made deterministic rather than
    // timed: the first row the sweep writes moves the target out from under
    // it, in a committed statement of its own, between the batch's select and
    // the target's guarded update.
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "CREATE FUNCTION mars_test_concurrent_rewrap() RETURNS trigger AS $$
         BEGIN
           UPDATE secrets
              SET data_key_wrapped = decode('{wrapped}', 'hex'),
                  data_key_nonce = decode('{nonce}', 'hex'),
                  key_version = 2
            WHERE id = '{target_id}' AND key_version = 1;
           RETURN NEW;
         END;
         $$ LANGUAGE plpgsql",
        wrapped = hex(&staged.wrapped),
        nonce = hex(&staged.nonce),
        target_id = target.id,
    )))
    .execute(&app.pool)
    .await
    .expect("the trigger function is created");
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "CREATE TRIGGER mars_test_concurrent_rewrap
         BEFORE UPDATE ON secrets FOR EACH ROW
         WHEN (NEW.id <> '{target_id}')
         EXECUTE FUNCTION mars_test_concurrent_rewrap()",
        target_id = target.id,
    )))
    .execute(&app.pool)
    .await
    .expect("the trigger is created");

    let report = rewrap_outdated(&app.pool, &current)
        .await
        .expect("the sweep runs");

    sqlx::raw_sql("DROP TRIGGER mars_test_concurrent_rewrap ON secrets")
        .execute(&app.pool)
        .await
        .expect("the trigger is dropped");

    assert_eq!(
        report,
        RotationReport {
            rewrapped: 2,
            skipped: 1,
            remaining: 0,
        },
        "the row that moved is skipped, not an error and not a failure"
    );

    let after = reread(&app.pool, target.id).await;
    assert_eq!(after.key_version, 2);
    assert_eq!(
        after.data_key_wrapped, staged.wrapped,
        "the concurrent writer's wrapping stands; the sweep did not write the old data key over it"
    );
    assert_eq!(after.data_key_nonce, staged.nonce.to_vec());
    assert_eq!(after.ciphertext, target.ciphertext);

    // And the row is still readable, which is what the version guard exists to
    // protect.
    let index = rows
        .iter()
        .position(|row| row.id == target.id)
        .expect("the target is one of the seeded rows");
    let opened = open(
        &current,
        &aad_for(&after.scope_ref(), &name("RACE", index)),
        &value_of(&after),
    )
    .expect("the value opens under the concurrent writer's wrapping");
    assert_eq!(opened.as_slice(), plaintext("RACE", index).as_bytes());
}

#[tokio::test]
async fn a_row_under_a_version_with_no_key_is_skipped_and_is_what_remains() {
    let app = TestApp::spawn().await;
    let old = keyring(&[1]);
    // Version 8 is the newest, so a row under version 7 is selected — and
    // there is no key 7 to unwrap it with.
    let current = keyring(&[1, 8]);

    let rows = seed(&app.pool, &old, "STUCK", 3).await;
    let stranded = insert_raw(&app.pool, "ORPHANED_KEY", fake_value("stranded", 7)).await;

    let report = rewrap_outdated(&app.pool, &current)
        .await
        .expect("a missing key version does not fail the sweep");
    assert_eq!(
        report,
        RotationReport {
            rewrapped: 3,
            skipped: 1,
            remaining: 1,
        },
        "the rows that can move, move; the one that cannot is reported"
    );

    for (index, row) in rows.iter().enumerate() {
        let after = reread(&app.pool, row.id).await;
        assert_eq!(after.key_version, 8);
        let opened = open(
            &current,
            &aad_for(&after.scope_ref(), &name("STUCK", index)),
            &value_of(&after),
        )
        .expect("the value still opens");
        assert_eq!(opened.as_slice(), plaintext("STUCK", index).as_bytes());
    }

    let after = reread(&app.pool, stranded.id).await;
    assert_eq!(
        after.key_version, 7,
        "the stranded row is left exactly as it was"
    );
    assert_eq!(after.data_key_wrapped, stranded.data_key_wrapped);
    assert_eq!(after.ciphertext, stranded.ciphertext);

    // A second run selects that one row, re-wraps nothing, and stops rather
    // than handing itself the same row forever.
    let again = rewrap_outdated(&app.pool, &current)
        .await
        .expect("the second sweep runs");
    assert_eq!(
        again,
        RotationReport {
            rewrapped: 0,
            skipped: 1,
            remaining: 1,
        }
    );
}

#[tokio::test]
async fn a_keyring_with_one_version_has_nothing_to_rotate() {
    let app = TestApp::spawn().await;
    let only = keyring(&[1]);

    let rows = seed(&app.pool, &only, "IDLE", 5).await;

    let report = rewrap_outdated(&app.pool, &only)
        .await
        .expect("the sweep runs");
    assert_eq!(
        report,
        RotationReport::default(),
        "nothing is behind the only key there is"
    );

    for row in &rows {
        let after = reread(&app.pool, row.id).await;
        assert_eq!(after.key_version, 1);
        assert_eq!(
            after.data_key_wrapped, row.data_key_wrapped,
            "an hourly sweep with nothing to do writes nothing"
        );
        assert_eq!(after.updated_at, row.updated_at);
    }
}
