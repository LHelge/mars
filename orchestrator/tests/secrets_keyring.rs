//! The startup key check against a real table
//! (`verify_keyring_at_startup`; `ARCHITECTURE.md`, "Secrets", Keyring).
//!
//! > At start the keyring verifies it can unwrap one row per `key_version`
//! > present in the table and refuses to start otherwise, because a missing
//! > key version would only be discovered at session launch.
//!
//! The unit tests in `src/secrets/keyring.rs` cover the wrap/unwrap algebra;
//! what needs a database is the sampling: a version that exists only in the
//! table, a keyring that does not carry it, and the message the operator gets.
//! Each test wraps a data key under a keyring that *does* carry the version,
//! stores the wrapping in a row, and then verifies with a different keyring —
//! which is exactly the shape of a restart with the wrong environment.
//!
//! The value bytes are never opened here, but every row is sealed for real
//! under the keyring that carries its version, because the wrapping columns
//! are what the check reads. Every key and every value in this file is
//! obviously fake (`CLAUDE.md`, rule 3).
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use common::TestApp;
use mars_orchestrator::models::{NewSecret, ScopeRef, SecretName};
use mars_orchestrator::repositories::SecretRepository;
use mars_orchestrator::secrets::{
    MASTER_KEY_LEN, SealedSecret, SecretIdentity, SecretsKeyring, verify_keyring_at_startup,
};

/// The version that exists in the table but not, by default, in the
/// environment.
const STORED_VERSION: i32 = 99;

/// Obviously fake master keys: 32 bytes of one repeated value each.
const KEY_99: [u8; MASTER_KEY_LEN] = [0x99; MASTER_KEY_LEN];
const OTHER_KEY_99: [u8; MASTER_KEY_LEN] = [0x77; MASTER_KEY_LEN];

/// Obviously fake value; nothing decrypts a value in this file.
const FAKE_VALUE: &[u8] = b"not-a-real-token";

/// One global row, sealed under `wrapping` so its `key_version` is that
/// keyring's newest.
fn sealed_under(wrapping: &SecretsKeyring, raw_name: &str) -> SealedSecret {
    let name = SecretName::parse(raw_name).expect("the test secret name is valid");

    SealedSecret::seal(
        wrapping,
        SecretIdentity::new(&ScopeRef::global(), &name),
        FAKE_VALUE,
    )
    .expect("the test value seals")
}

/// Insert one already-sealed row in its own committed transaction.
async fn insert(app: &TestApp, sealed: SealedSecret) {
    let mut tx = app.pool.begin().await.expect("a transaction begins");
    SecretRepository::new(&app.pool)
        .insert(&mut tx, &NewSecret::new(sealed))
        .await
        .expect("the secret inserts");
    tx.commit().await.expect("the transaction commits");
}

/// The test keyring plus a key for [`STORED_VERSION`].
///
/// Built from the app's own version 1 so the two keyrings differ in exactly
/// one version, which is what each assertion is about.
fn keyring_with_99(app: &TestApp, key_99: [u8; MASTER_KEY_LEN]) -> SecretsKeyring {
    let current = app.state.keyring.current_version();
    let key_1 = *app
        .state
        .keyring
        .key(current)
        .expect("the test keyring carries its current key");

    let version_99 =
        u32::try_from(STORED_VERSION).expect("the stored version is a positive integer");

    SecretsKeyring::from_entries(vec![(current, key_1), (version_99, key_99)])
        .expect("two distinct versions are a valid keyring")
}

/// Store one secret whose data key is wrapped under [`STORED_VERSION`].
async fn insert_row_wrapped_under_99(app: &TestApp, wrapping: &SecretsKeyring) {
    let sealed = sealed_under(wrapping, "OLD_VERSION_TOKEN");
    assert_eq!(
        sealed.wrapped.version, STORED_VERSION,
        "the wrapping keyring must use its highest version"
    );

    insert(app, sealed).await;
}

#[tokio::test]
async fn an_empty_table_verifies() {
    let app = TestApp::spawn().await;

    verify_keyring_at_startup(&app.pool, &app.state.keyring)
        .await
        .expect("a table with no secrets has nothing to unwrap");
}

#[tokio::test]
async fn a_row_the_keyring_can_unwrap_verifies() {
    let app = TestApp::spawn().await;
    let keyring = keyring_with_99(&app, KEY_99);

    insert_row_wrapped_under_99(&app, &keyring).await;

    verify_keyring_at_startup(&app.pool, &keyring)
        .await
        .expect("the version that wrapped the row is configured");
}

#[tokio::test]
async fn a_missing_key_version_is_reported_by_number() {
    let app = TestApp::spawn().await;
    insert_row_wrapped_under_99(&app, &keyring_with_99(&app, KEY_99)).await;

    // The app's own keyring carries version 1 only: the restart the operator
    // did without adding the key.
    let message = verify_keyring_at_startup(&app.pool, &app.state.keyring)
        .await
        .expect_err("version 99 has no configured master key")
        .to_string();

    assert!(message.contains("99"), "{message}");
    assert!(
        message.contains("no configured master key"),
        "the operator has to be told the key is absent, not merely wrong: {message}"
    );
}

#[tokio::test]
async fn a_configured_but_wrong_key_is_reported_as_undecryptable() {
    let app = TestApp::spawn().await;
    insert_row_wrapped_under_99(&app, &keyring_with_99(&app, KEY_99)).await;

    // Version 99 is configured, but with a different key: the row must not be
    // silently accepted, and the message must not claim the key is missing.
    let message = verify_keyring_at_startup(&app.pool, &keyring_with_99(&app, OTHER_KEY_99))
        .await
        .expect_err("the configured key for version 99 did not wrap the row")
        .to_string();

    assert!(message.contains("99"), "{message}");
    assert!(message.contains("cannot be unwrapped"), "{message}");
    assert!(!message.contains("no configured master key"), "{message}");
}

#[tokio::test]
async fn every_bad_version_is_named_in_one_message() {
    let app = TestApp::spawn().await;
    let keyring = keyring_with_99(&app, KEY_99);
    insert_row_wrapped_under_99(&app, &keyring).await;

    // A second version, also absent from the app's keyring, so the operator
    // can add both keys in one go rather than restarting per version.
    let other_version: u32 = 42;
    let other = SecretsKeyring::from_entries(vec![(other_version, [0x42; MASTER_KEY_LEN])])
        .expect("one entry is a valid keyring");
    insert(&app, sealed_under(&other, "OLDER_TOKEN")).await;

    let message = verify_keyring_at_startup(&app.pool, &app.state.keyring)
        .await
        .expect_err("neither version is configured")
        .to_string();

    assert!(message.contains("42"), "{message}");
    assert!(message.contains("99"), "{message}");
}

#[tokio::test]
async fn the_check_reads_the_wrapping_and_no_key_material_reaches_the_message() {
    let app = TestApp::spawn().await;
    insert_row_wrapped_under_99(&app, &keyring_with_99(&app, KEY_99)).await;

    let message = verify_keyring_at_startup(&app.pool, &app.state.keyring)
        .await
        .expect_err("version 99 has no configured master key")
        .to_string();

    // Rule 3: the message names versions and nothing else. Both a raw byte
    // run and its base64 would be key material in a log line.
    for byte in [0x99u8, 0x42u8] {
        assert!(
            !message.contains(&format!("{byte}, {byte}")),
            "the message must not echo key bytes: {message}"
        );
    }
    assert!(!message.contains("mZmZ"), "{message}");
}
