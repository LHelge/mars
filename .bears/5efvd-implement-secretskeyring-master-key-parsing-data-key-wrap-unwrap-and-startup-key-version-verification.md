---
id: "5efvd"
title: "Implement SecretsKeyring: master-key parsing, data-key wrap/unwrap and startup key_version verification"
status: open
priority: P0
created: "2026-09-16T20:29:20.648385693Z"
updated: "2026-09-16T20:51:51.694847232Z"
tags:
  - orchestrator
  - secrets
  - core
depends_on:
  - qacxf
parent: t36d2
---

## Summary
Deliver `SecretsKeyring`: parse the versioned master keys from `SECRETS_MASTER_KEYS` or `SECRETS_MASTER_KEY_FILE`, expose wrap/unwrap of per-row data keys under a chosen key version, select the highest version for new rows, and verify at startup that every `key_version` present in `secrets` can be unwrapped, refusing to start otherwise. This is the foundation every other secrets task builds on and the reason a missing key is discovered at boot rather than at session launch.

## Documents
- `ARCHITECTURE.md` "Secrets" → "Keyring" (env format, highest version for new rows, startup verification)
- `ARCHITECTURE.md` "Orchestrator internals" (`AppState` holds the `SecretsKeyring`; crates `aes-gcm`, `rand`, `zeroize`, `base64`; `SecretsError` variant in the `Error` enum)
- `README.md` "Configuration" rows `SECRETS_MASTER_KEYS` / `SECRETS_MASTER_KEY_FILE`; "Generate a master key with `openssl rand -base64 32`"
- `docs/data-model.md` `secrets` columns `data_key_wrapped`, `data_key_nonce`, `key_version`; index `secrets_key_version_idx`
- ADR 0006

## Acceptance criteria
- [ ] `SecretsKeyring::from_config(&Config)` accepts `SECRETS_MASTER_KEYS="1=<b64>,2=<b64>"` (whitespace around entries tolerated) and, when that variable is unset, the same content read from `SECRETS_MASTER_KEY_FILE` (trailing newline tolerated). Exactly one of the two must be set; otherwise startup fails naming both variables.
- [ ] A key that is not valid base64, does not decode to exactly 32 bytes, has a non-positive or non-integer version, or repeats a version fails with `SecretsError::InvalidMasterKey(<reason, never key bytes>)`.
- [ ] `newest_version() -> i32` returns the highest configured version; `wrap_data_key(&[u8; 32]) -> Result<WrappedKey { wrapped: Vec<u8>, nonce: [u8; 12], version: i32 }>` uses it; `unwrap_data_key(wrapped: &[u8], nonce: &[u8], version: i32) -> Result<Zeroizing<[u8; 32]>>` uses that version's key, returns `SecretsError::UnknownKeyVersion(version)` when no such key is configured and `SecretsError::Decrypt` when the tag fails.
- [ ] `verify_against_db(&PgPool) -> Result<()>` selects one row per distinct `key_version` in `secrets`, unwraps each, and returns an error listing every unknown or undecryptable version; `main.rs` calls it after migrations and exits non-zero with a `tracing::error!` naming the versions (never key material).
- [ ] Master key bytes and unwrapped data keys live in `Zeroizing` buffers; `Debug` for the keyring prints versions only.
- [ ] `AppState.keyring: SecretsKeyring` (cheap `Clone`, `Arc` inside) is populated in `main.rs` and in `TestApp::spawn()` from the fixed test key.

## Implementation notes
- Files: `orchestrator/src/secrets/mod.rs` (re-exports), `orchestrator/src/secrets/keyring.rs`, `orchestrator/src/secrets/error.rs` (`SecretsError` via `thiserror`: `InvalidMasterKey(String)`, `NoKeysConfigured`, `UnknownKeyVersion(i32)`, `Decrypt`), `orchestrator/src/prelude/error.rs` (`#[from] SecretsError` → 500 generic message if not already present), the `AppState` definition under `orchestrator/src/prelude/`, `orchestrator/src/main.rs`, `orchestrator/tests/common/mod.rs`.
- `Config` should already read `SECRETS_MASTER_KEYS` / `SECRETS_MASTER_KEY_FILE` as raw optional strings (scaffolding epic); if it does not, add the two optional fields there and keep `.env.example` and the README table in sync.
- Wrap = AES-256-GCM (`aes_gcm::Aes256Gcm`) of the 32-byte data key with a fresh 12-byte nonce from `rand::rngs::OsRng`, empty AAD (the row AAD protects the value ciphertext, task 2). Output is the 48-byte wrapped key (32 + 16 tag) plus the nonce; stored in `data_key_wrapped` / `data_key_nonce`.
- Verification query (add to the schema epic's `SecretRepository`): `SELECT DISTINCT ON (key_version) key_version, data_key_wrapped, data_key_nonce FROM secrets ORDER BY key_version, id`.
- Error strings: `"SECRETS_MASTER_KEYS or SECRETS_MASTER_KEY_FILE must be set (exactly one)"`, `"secrets: key version {v} has no configured master key"`, `"secrets: rows wrapped with key version {v} cannot be unwrapped"`.
- Log `tracing::info!(versions = ?keyring.versions(), newest = keyring.newest_version(), "secrets keyring loaded")`.

## Edge cases
- Both variables set: reject (ambiguous), naming both.
- Empty `secrets` table: verification passes trivially.
- Key file with mode broader than 0600: accept but log `warn` (the file may be a mounted container secret whose mode cannot be controlled).
- A version present in the table with a configured but wrong key (tag mismatch) is reported as undecryptable, not silently accepted.
- Version numbers are compared numerically (`10` > `9`), not lexically.

## Testing
- Unit tests in `keyring.rs`: parse two versions; parse from file; reject bad base64, wrong length, duplicate version, zero/negative version, both variables set, neither set; wrap then unwrap round-trip; unwrap with an unconfigured version → `UnknownKeyVersion`; unwrap with tampered wrapped bytes → `Decrypt`; `newest_version` with versions `2,10,9` → 10.
- Integration test `orchestrator/tests/secrets_keyring.rs` via `TestApp`: insert a `secrets` row wrapped under version 99 directly through the repository, build a keyring without version 99, assert `verify_against_db` fails naming 99; with version 99 configured, assert it passes.
- `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- none: implements the documented contract as written (README row and ARCHITECTURE "Keyring" already describe the format and verification).

## Assumes from other epics
- "Repository scaffolding, tooling and CI": `Config::from_env()`, `AppState`, the `Error` enum, empty `secrets/` module.
- "Database schema, models, repositories and test harness": the `secrets` migration, `Secret` model and `SecretScope` enum, a `SecretRepository<'a>` to extend, and `TestApp::spawn()` with a fixed test master key.