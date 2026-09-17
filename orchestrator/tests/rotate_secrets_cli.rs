//! The `mars-orchestrator rotate-secrets` subcommand (`ARCHITECTURE.md`,
//! "Secrets", Rotation; `README.md`, "Operating notes").
//!
//! The contract here is a process, not a function: `secrets::rewrap_outdated`
//! has its own tests in `tests/secrets_rotation.rs`, and what these assert is
//! the part only the binary can answer — that the subcommand bootstraps like
//! the server does, that its stdout line is exactly
//! `rewrapped=<n> skipped=<n> remaining=<n>`, and that the three exit codes a
//! script switches on (0 done, 1 failed, 64 bad arguments) are the ones it
//! hands back.
//!
//! Exit 2 — `remaining > 0` after a sweep that itself succeeded — has no test
//! here because no arrangement reaches it from the outside: the only rows the
//! sweep leaves behind are rows under a version the keyring does not carry,
//! which the startup verification refuses first (exit 1), and rows a concurrent
//! writer moved between a batch and its write, which is a race a spawned
//! process cannot be made to lose. `rewrap_outdated`'s own tests cover the
//! report that produces it (`tests/secrets_rotation.rs`).
//!
//! So each test spawns the built binary (`CARGO_BIN_EXE_mars-orchestrator`)
//! against the `TestApp`'s own Postgres container. The child's environment is
//! cleared and rebuilt from the same obviously fake values the harness uses
//! (rule 3), with `SECRETS_MASTER_KEYS` rendered from the keyrings rather than
//! pasted, so the child and the test can never disagree about a key. Its
//! working directory is the harness's temporary data directory, so no
//! developer's `.env` can reach it.
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use std::path::Path;
use std::process::{Output, Stdio};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use common::TestApp;
use mars_orchestrator::models::{EncryptedValue, NewSecret, ScopeRef, Secret, SecretName};
use mars_orchestrator::prelude::*;
use mars_orchestrator::repositories::SecretRepository;
use mars_orchestrator::secrets::{MASTER_KEY_LEN, SecretsKeyring, aad_for, open, seal};

/// The version the sweep moves rows onto: one above the harness's own keyring,
/// which is version 1 (`SecretsKeyring::test_key`).
const NEW_VERSION: u32 = 2;

/// A version no keyring in this file carries, for the row that makes the
/// startup verification fail.
const ABSENT_VERSION: i32 = 99;

/// Obviously not a real master key: one repeated byte (rule 3).
fn fake_master_key(byte: u8) -> [u8; MASTER_KEY_LEN] {
    [byte; MASTER_KEY_LEN]
}

/// Not key material: an obviously fake stand-in for the four encrypted columns
/// of a row nothing ever opens (rule 3).
fn fake_value(key_version: i32) -> EncryptedValue {
    EncryptedValue {
        ciphertext: b"fake-ciphertext-absent-version".to_vec(),
        nonce: b"fake-nonce-absent-version".to_vec(),
        data_key_wrapped: b"fake-wrapped-data-key-absent-version".to_vec(),
        data_key_nonce: b"fake-wrap-nonce-absent-version".to_vec(),
        key_version,
    }
}

/// `SECRETS_MASTER_KEYS` as the process reads it, rendered from the keys
/// themselves so the string and the keyrings cannot drift (the harness's
/// `test_config` renders it the same way).
fn render_keys(entries: &[(u32, [u8; MASTER_KEY_LEN])]) -> String {
    entries
        .iter()
        .map(|(version, key)| format!("{version}={}", STANDARD.encode(key)))
        .collect::<Vec<_>>()
        .join(",")
}

/// The harness's version-1 key, read back out of the keyring the app was built
/// with rather than reconstructed.
fn harness_key(app: &TestApp) -> (u32, [u8; MASTER_KEY_LEN]) {
    let version = app.state.keyring.current_version();
    let key = *app
        .state
        .keyring
        .key(version)
        .expect("the harness keyring carries its current key");

    (version, key)
}

/// Seal `count` global secrets under `keyring` and insert them, returning the
/// stored rows in insertion order.
async fn seed(pool: &PgPool, keyring: &SecretsKeyring, count: usize) -> Vec<Secret> {
    let repository = SecretRepository::new(pool);
    let mut tx = pool.begin().await.expect("a transaction begins");

    let mut rows = Vec::with_capacity(count);
    for index in 0..count {
        let scope = ScopeRef::global();
        let name =
            SecretName::parse(&format!("FAKE_ROTATE_{index:04}")).expect("the test name is valid");
        let aad = aad_for(&scope, &name);
        let sealed = seal(keyring, &aad, plaintext(index).as_bytes()).expect("the value seals");

        let new = NewSecret::new(scope, name, sealed);
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

/// The obviously fake plaintext row `index` carries (rule 3).
fn plaintext(index: usize) -> String {
    format!("fake-value-{index}")
}

/// Insert one row whose encrypted columns are given verbatim.
async fn insert_raw(pool: &PgPool, raw_name: &str, value: EncryptedValue) {
    let new = NewSecret::new(
        ScopeRef::global(),
        SecretName::parse(raw_name).expect("the test name is valid"),
        value,
    );

    let mut tx = pool.begin().await.expect("a transaction begins");
    SecretRepository::new(pool)
        .insert(&mut tx, &new)
        .await
        .expect("the secret inserts");
    tx.commit().await.expect("the transaction commits");
}

/// How long a run of the binary is given before the test gives up on it.
///
/// Nothing here starts a listener or waits for a signal, so every run finishes
/// in well under a second once its Postgres answers; the generous limit is for
/// a loaded machine. It exists because the failure it catches is a *hang*: a
/// binary that fell through to the server path would sit on its listeners until
/// the whole suite timed out, with nothing said about why. Reaching this is a
/// failed assertion, never a slow success.
const RUN_TIMEOUT: Duration = Duration::from_secs(120);

/// Run the binary with `arguments` and the harness's configuration, plus
/// `SECRETS_MASTER_KEYS` built from `keys`.
async fn run(
    app: &TestApp,
    arguments: &[&str],
    keys: &[(u32, [u8; MASTER_KEY_LEN])],
) -> ProcessResult {
    let data_dir = app.data_dir.path();

    run_with_env(arguments, data_dir, child_env(app, data_dir, keys)).await
}

/// Spawn the binary, wait for it to exit and collect what it said.
///
/// `env_clear` first: the child gets exactly the variables in `env`, so a
/// `RESEND_API_KEY` or a `RUST_LOG` in the developer's shell cannot change what
/// it does. Its working directory is the harness data directory for the same
/// reason — `Config::from_env` looks for `.env` and `../.env` beside the
/// process, and there is none there.
///
/// `kill_on_drop` pairs with [`RUN_TIMEOUT`]: the child a timed-out run leaves
/// behind is killed when the future is dropped rather than outliving the suite.
async fn run_with_env(
    arguments: &[&str],
    working_dir: &Path,
    env: Vec<(String, String)>,
) -> ProcessResult {
    let child = tokio::process::Command::new(env!("CARGO_BIN_EXE_mars-orchestrator"))
        .args(arguments)
        .current_dir(working_dir)
        .env_clear()
        .envs(env)
        .kill_on_drop(true)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the orchestrator binary starts");

    let output = tokio::time::timeout(RUN_TIMEOUT, child.wait_with_output())
        .await
        .unwrap_or_else(|_| {
            panic!(
                "`mars-orchestrator {}` did not exit within {} seconds; a subcommand run never \
                 serves and never waits for a signal",
                arguments.join(" "),
                RUN_TIMEOUT.as_secs(),
            )
        })
        .expect("the orchestrator binary runs");

    ProcessResult::from(output)
}

/// The same obviously fake values `tests/common/app.rs::test_config` uses, as
/// the environment a child process reads them from (rule 3). `DATABASE_URL` is
/// the only one pointing at anything real: the harness's own container.
///
/// `API_PORT` and `MCP_PORT` are carried so the child parses the configuration
/// the harness would; `rotate-secrets` binds neither.
fn child_env(
    app: &TestApp,
    data_dir: &Path,
    keys: &[(u32, [u8; MASTER_KEY_LEN])],
) -> Vec<(String, String)> {
    let data_dir = data_dir.display().to_string();
    let config = &app.state.config;

    [
        ("PUBLIC_URL", "http://localhost".to_string()),
        (
            "JWT_SECRET",
            "test-jwt-secret-not-for-production".to_string(),
        ),
        ("DATABASE_URL", config.database_url.clone()),
        (
            "DOCKER_HOST",
            "unix:///nonexistent/mars-test/podman.sock".to_string(),
        ),
        ("DATA_DIR", data_dir.clone()),
        ("DATA_DIR_HOST", data_dir),
        ("SECRETS_MASTER_KEYS", render_keys(keys)),
        ("GIT_BOT_NAME", "Mars Test Bot".to_string()),
        ("GIT_BOT_EMAIL", "bot@example.test".to_string()),
        (
            "SESSION_IMAGE_DEFAULT",
            "mars-session-stub:test".to_string(),
        ),
        ("API_PORT", config.api_port.to_string()),
        ("MCP_PORT", config.mcp_port.to_string()),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_string(), value))
    .collect()
}

/// What one run of the binary said and how it ended.
struct ProcessResult {
    code: i32,
    stdout: String,
    stderr: String,
}

impl From<Output> for ProcessResult {
    fn from(output: Output) -> Self {
        ProcessResult {
            code: output
                .status
                .code()
                .expect("the orchestrator exits rather than being signalled"),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }
}

impl ProcessResult {
    /// The single report line, with nothing else on stdout.
    fn report(&self) -> &str {
        self.stdout.trim_end_matches('\n')
    }
}

/// The `key_version` of every row, ascending.
async fn key_versions(pool: &PgPool) -> Vec<i32> {
    sqlx::query_scalar::<_, i32>("SELECT key_version FROM secrets ORDER BY key_version")
        .fetch_all(pool)
        .await
        .expect("the key versions read")
}

#[tokio::test]
async fn rotate_secrets_rewraps_every_row_and_exits_zero() {
    let app = TestApp::spawn().await;
    let (old_version, old_key) = harness_key(&app);
    let new_key = fake_master_key(0xB2);

    // Sealed under the harness keyring, which is the only version configured
    // when the rows are written.
    let rows = seed(&app.pool, &app.state.keyring, 3).await;

    let result = run(
        &app,
        &["rotate-secrets"],
        &[(old_version, old_key), (NEW_VERSION, new_key)],
    )
    .await;

    assert_eq!(result.code, 0, "stderr: {}", result.stderr);
    assert_eq!(result.report(), "rewrapped=3 skipped=0 remaining=0");

    assert_eq!(
        key_versions(&app.pool).await,
        vec![NEW_VERSION as i32; 3],
        "every row is on the newest key version",
    );

    // The rotation moved the wrapping, not the value: each row still opens, to
    // the plaintext it was sealed with, under the new keyring.
    let rotated =
        SecretsKeyring::from_entries(vec![(old_version, old_key), (NEW_VERSION, new_key)])
            .expect("the two-version keyring is valid");
    let repository = SecretRepository::new(&app.pool);
    for (index, row) in rows.iter().enumerate() {
        let stored = repository
            .find(row.id)
            .await
            .expect("the row reads")
            .expect("the row is still there");
        let value = EncryptedValue {
            ciphertext: stored.ciphertext.clone(),
            nonce: stored.nonce.clone(),
            data_key_wrapped: stored.data_key_wrapped.clone(),
            data_key_nonce: stored.data_key_nonce.clone(),
            key_version: stored.key_version,
        };
        let name = SecretName::parse(&stored.name).expect("the stored name is valid");
        let aad = aad_for(&stored.scope_ref(), &name);
        let opened = open(&rotated, &aad, &value).expect("the rotated value opens");

        assert_eq!(
            String::from_utf8(opened.to_vec()).expect("the plaintext is utf-8"),
            plaintext(index),
        );
    }
}

#[tokio::test]
async fn rotate_secrets_on_an_empty_table_reports_zeros() {
    let app = TestApp::spawn().await;
    let (old_version, old_key) = harness_key(&app);

    // The newest key is configured and no row uses it yet: nothing to do.
    let result = run(
        &app,
        &["rotate-secrets"],
        &[(old_version, old_key), (NEW_VERSION, fake_master_key(0xB2))],
    )
    .await;

    assert_eq!(result.code, 0, "stderr: {}", result.stderr);
    assert_eq!(result.report(), "rewrapped=0 skipped=0 remaining=0");
}

#[tokio::test]
async fn rotate_secrets_exits_one_when_a_row_is_under_an_unconfigured_version() {
    let app = TestApp::spawn().await;
    let (old_version, old_key) = harness_key(&app);

    seed(&app.pool, &app.state.keyring, 2).await;
    insert_raw(&app.pool, "FAKE_ABSENT_VERSION", fake_value(ABSENT_VERSION)).await;

    let result = run(
        &app,
        &["rotate-secrets"],
        &[(old_version, old_key), (NEW_VERSION, fake_master_key(0xB2))],
    )
    .await;

    // The startup verification refuses before the sweep begins: a version with
    // no configured key is a configuration fault, not a row to skip.
    assert_eq!(result.code, 1, "stdout: {}", result.stdout);
    assert!(
        result.stdout.is_empty(),
        "no report line is printed for a failed run: {}",
        result.stdout,
    );
    assert!(
        result.stderr.contains(&ABSENT_VERSION.to_string()),
        "the error names the offending version: {}",
        result.stderr,
    );

    assert_eq!(
        key_versions(&app.pool).await,
        vec![old_version as i32, old_version as i32, ABSENT_VERSION],
        "nothing was rewrapped",
    );
}

#[tokio::test]
async fn rotate_secrets_exits_one_when_the_database_is_unreachable() {
    let app = TestApp::spawn().await;
    let (old_version, old_key) = harness_key(&app);

    let data_dir = app.data_dir.path();
    let mut env = child_env(&app, data_dir, &[(old_version, old_key)]);
    for entry in &mut env {
        if entry.0 == "DATABASE_URL" {
            // An obviously fake URL on a port nothing listens on (rule 3).
            entry.1 = "postgres://fake:fake@127.0.0.1:1/fake".to_string();
        }
    }

    let result = run_with_env(&["rotate-secrets"], data_dir, env).await;

    assert_eq!(result.code, 1, "stdout: {}", result.stdout);
    assert!(result.stdout.is_empty(), "stdout: {}", result.stdout);
}

#[tokio::test]
async fn a_bogus_argument_prints_the_usage_line_and_exits_sixty_four() {
    let app = TestApp::spawn().await;
    let (old_version, old_key) = harness_key(&app);

    let result = run(&app, &["rotate-everything"], &[(old_version, old_key)]).await;

    assert_eq!(result.code, 64, "stdout: {}", result.stdout);
    assert_eq!(
        result.stderr.trim_end_matches('\n'),
        "usage: mars-orchestrator [rotate-secrets]",
    );
    assert!(result.stdout.is_empty(), "stdout: {}", result.stdout);
}
