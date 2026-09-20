//! The hourly secret rotation job (`ARCHITECTURE.md`, "Background jobs", and
//! "Secrets" → Rotation).
//!
//! The sweep itself is asserted in `tests/secrets_rotation.rs`; what is
//! asserted here is what the job adds to it: that it runs the sweep over the
//! state's own pool and keyring, that the sweep's three counters arrive in the
//! common `JobReport` the way an operator reads them, and that a sweep which
//! fails as a whole becomes `Err` rather than a report of zeros.
//!
//! Every job takes its `now` from the caller, so each scenario passes one; this
//! job never looks at it, which is itself part of the contract.
//!
//! The keyrings here are built from repeated bytes and the values are
//! `fake-value-…`: nothing in this file is or resembles a real key or a real
//! secret (`CLAUDE.md`, rule 3). The harness's own keyring holds one version,
//! so every scenario but the last builds an `AppState` of its own over the same
//! pool — a rotation needs two versions.
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use chrono::Utc;
use common::TestApp;
use mars_orchestrator::cron::{CronService, JobReport};
use mars_orchestrator::models::{NewSecret, ScopeRef, Secret, SecretName};
use mars_orchestrator::prelude::*;
use mars_orchestrator::repositories::SecretRepository;
use mars_orchestrator::secrets::{MASTER_KEY_LEN, SealedSecret, SecretIdentity, SecretsKeyring};
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

/// The app's state with a keyring of this test's own, over the same pool: what
/// the process would hold had the operator added a second master key.
fn state_with(app: &TestApp, keyring: SecretsKeyring) -> AppState {
    AppState {
        keyring,
        ..app.state.clone()
    }
}

fn plaintext(prefix: &str, index: usize) -> String {
    format!("fake-value-{prefix}-{index}")
}

/// Seal `count` global secrets under `keyring` and insert them, returning the
/// stored rows in insertion order.
async fn seed(pool: &PgPool, keyring: &SecretsKeyring, prefix: &str, count: usize) -> Vec<Secret> {
    let repository = SecretRepository::new(pool);
    let mut tx = pool.begin().await.expect("a transaction begins");

    let mut rows = Vec::with_capacity(count);
    for index in 0..count {
        let scope = ScopeRef::global();
        let secret_name = SecretName::parse(&format!("{prefix}_{index:04}"))
            .expect("the test secret name is valid");
        let sealed = SealedSecret::seal(
            keyring,
            SecretIdentity::new(&scope, &secret_name),
            plaintext(prefix, index).as_bytes(),
        )
        .expect("the test value seals");

        rows.push(
            repository
                .insert(&mut tx, &NewSecret::new(sealed))
                .await
                .expect("the secret inserts"),
        );
    }

    tx.commit().await.expect("the transaction commits");

    rows
}

async fn reread(pool: &PgPool, id: Uuid) -> Secret {
    SecretRepository::new(pool)
        .find(id)
        .await
        .expect("the row reads")
        .expect("the row is still there")
}

#[tokio::test]
async fn the_job_moves_every_outdated_row_onto_the_newest_key() {
    let app = TestApp::spawn().await;
    let old = keyring(&[1]);
    let before = seed(&app.pool, &old, "CRONROT", 5).await;

    let cron = CronService::new(state_with(&app, keyring(&[1, 2])));
    let report = cron
        .secret_rotation(Utc::now())
        .await
        .expect("the job runs");

    assert_eq!(
        report,
        JobReport {
            items: 5,
            skipped: 0,
            failures: 0,
        },
        "the sweep's `rewrapped` is the job's `items`"
    );

    for row in &before {
        assert_eq!(row.key_version, 1, "the rows were sealed under version 1");

        let after = reread(&app.pool, row.id).await;
        assert_eq!(after.key_version, 2, "every row is on the newest key");
        assert_ne!(
            after.data_key_wrapped, row.data_key_wrapped,
            "the data key is wrapped again, under a different master key"
        );

        // Rotation changes which master key protects a data key, never the
        // value: the encrypted columns come back byte for byte.
        assert_eq!(after.ciphertext, row.ciphertext);
        assert_eq!(after.nonce, row.nonce);
    }

    // Idempotent: the next hour finds nothing to do.
    let again = cron
        .secret_rotation(Utc::now())
        .await
        .expect("the second run runs");
    assert_eq!(again, JobReport::default());
}

#[tokio::test]
async fn a_row_whose_key_version_has_no_key_is_skipped_rather_than_failed() {
    let app = TestApp::spawn().await;
    // Sealed under version 1, which the operator then dropped from the
    // environment while versions 2 and 3 remain. The row is selectable and
    // cannot be re-wrapped, which is the one case the job warns about.
    let stranded = seed(&app.pool, &keyring(&[1]), "STRANDED", 1).await;
    let movable = seed(&app.pool, &keyring(&[2]), "MOVABLE", 2).await;

    let report = CronService::new(state_with(&app, keyring(&[2, 3])))
        .secret_rotation(Utc::now())
        .await
        .expect("a version with no key is a skip, not a failure");

    assert_eq!(
        report,
        JobReport {
            items: 2,
            skipped: 1,
            failures: 0,
        },
        "the rows that can move still move; the stranded one is skipped"
    );

    // The sweep reported `remaining = 1`, which is what the job's single
    // `warn!` counts. There is no log capture in this harness, so the report is
    // what is asserted: the row is still on the version nothing can unwrap.
    assert_eq!(
        reread(&app.pool, stranded[0].id).await.key_version,
        1,
        "a row with no key for its version keeps its wrapping"
    );
    for row in &movable {
        assert_eq!(reread(&app.pool, row.id).await.key_version, 3);
    }
}

#[tokio::test]
async fn a_sweep_failure_propagates_and_keeps_what_it_committed() {
    let app = TestApp::spawn().await;
    let rows = seed(&app.pool, &keyring(&[1]), "WRONGKEY", 3).await;

    // The same version, a different key: the operator replaced version 1's
    // value instead of adding a version. No amount of sweeping fixes that, so
    // it is an error rather than a skip, and the scheduler retries next hour.
    let wrong =
        SecretsKeyring::from_entries(vec![(1, fake_master_key(0x11)), (2, fake_master_key(0xA2))])
            .expect("the test keyring is valid");

    let error = CronService::new(state_with(&app, wrong))
        .secret_rotation(Utc::now())
        .await
        .expect_err("a wrapping that does not verify fails the job");
    assert!(
        !format!("{error}").contains("fake"),
        "no key material or value reaches the error: {error}"
    );

    for row in &rows {
        assert_eq!(
            reread(&app.pool, row.id).await.key_version,
            1,
            "nothing was written before the failure"
        );
    }
}

#[tokio::test]
async fn a_single_version_keyring_reports_zeros() {
    let app = TestApp::spawn().await;
    // The harness's keyring: one version, which is every hour on a deployment
    // where nobody is rotating anything.
    seed(&app.pool, &app.state.keyring, "QUIET", 3).await;

    assert_eq!(
        app.cron()
            .secret_rotation(Utc::now())
            .await
            .expect("the job runs"),
        JobReport::default(),
    );
}
