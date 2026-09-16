---
id: xbjn4
title: Define mock engine, email, git-credential and keyring stubs behind the integration-tests feature
status: open
priority: P1
created: "2026-09-16T20:27:50.753387584Z"
updated: "2026-09-16T20:27:50.753387584Z"
tags:
  - orchestrator
  - core
  - tests
  - engine
  - secrets
  - git
parent: p5tsd
---

## Summary
Give `TestApp` the collaborators it must inject into `AppState`: the minimal trait signatures for `ContainerEngine`, `EmailClient` and `GitCredentialProvider` (each with `as_any()` for downcasting), one mock implementation of each compiled only with the `integration-tests` feature, and a `SecretsKeyring` constructor that takes a fixed test master key. The engine, email, git and secrets epics extend these traits with their real methods and production implementations; this task only establishes the shape the test harness relies on.

## Documents
- `ARCHITECTURE.md` "Orchestrator internals" (`AppState` holds `Arc<dyn ContainerEngine>`, `Arc<dyn EmailClient>`, `Arc<dyn GitCredentialProvider>`, the `SecretsKeyring`; every `Arc<dyn Trait>` has a mock behind the `integration-tests` feature), "Secrets" ("Keyring": `SECRETS_MASTER_KEYS` holds `<version>=<base64 32 bytes>` entries), "Git model" ("Commit identity": `GitCredentialProvider::commit_identity`).
- `CLAUDE.md` "Backend conventions" (trait + production implementation + mock with `as_any()`), rule 3 (fake values only), "Testing expectations" (invite flows asserted through the mock email client's captured messages).

## Acceptance criteria
- [ ] `engine::ContainerEngine: Send + Sync` exists with at least `fn as_any(&self) -> &dyn Any` and `async fn ping(&self) -> Result<()>` (used by `GET /api/health` for `engine: bool`); `engine::mock::MockContainerEngine` (feature-gated) records calls in a `Mutex<Vec<...>>` and answers `ping` with `Ok(())`.
- [ ] `email::EmailClient: Send + Sync` exists with `as_any()` and `async fn send(&self, message: EmailMessage) -> Result<()>` where `EmailMessage { to: String, subject: String, text: String }`; `email::mock::MockEmailClient` (feature-gated) stores every message in `Mutex<Vec<EmailMessage>>` and exposes `fn sent(&self) -> Vec<EmailMessage>`.
- [ ] `git::GitCredentialProvider: Send + Sync` exists with `as_any()`, `async fn credential(&self, project_id: Uuid) -> Result<Option<GitCredential>>` and `fn commit_identity(&self) -> CommitIdentity { name, email }`; `git::mock::MockGitCredentialProvider` (feature-gated) returns a configurable credential defaulting to the obviously fake `x-access-token` / `ghp_FAKE_TEST_TOKEN_0000000000` and identity `Mars Test Bot <bot@example.test>`.
- [ ] `secrets::SecretsKeyring::from_entries(Vec<(u32, [u8; 32])>) -> Result<Self>` and `SecretsKeyring::parse(&str)` (the `<version>=<base64>` comma-separated format) exist; `SecretsKeyring::test_key()` (feature-gated) returns a keyring with version 1 and a fixed 32-byte key, e.g. bytes `0x01..=0x20`.
- [ ] `EngineError`, `EmailError`, `GitError`, `SecretsError` exist as `thiserror` enums with at least one variant each and are `#[from]` variants of `prelude::Error` (500 unless the owning epic refines the mapping).
- [ ] Mocks are `pub` only under `#[cfg(feature = "integration-tests")]`; a release build contains none of them.
- [ ] `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes, and `cargo clippy -- -D warnings` without the feature also passes.

## Implementation notes
- Files: `orchestrator/src/engine/mod.rs` (+ `engine/mock.rs`), `src/email/mod.rs` (+ `email/mock.rs`), `src/git/mod.rs` (+ `git/mock.rs`), `src/secrets/mod.rs` (`keyring.rs`), `src/prelude/error.rs`.
- Use `async_trait` only if the scaffolding already chose it; otherwise use native `async fn` in traits with `Send` bounds (Rust edition 2024, stable supports `async fn` in traits; for `dyn` compatibility prefer returning `Pin<Box<dyn Future + Send + '_>>` through a small macro or the `async_trait` crate added with `cargo add`). Record the choice in the module doc comment so the engine/email/git epics follow it.
- If the scaffolding epic already created placeholder traits or `AppState` fields with these names, extend them in place rather than duplicating.
- The keyring stores keys in a `zeroize`-wrapped buffer; `parse` rejects a version that is not a `u32`, a key that is not exactly 32 bytes after base64 decoding, and duplicate versions, with `SecretsError::InvalidMasterKey(String)` naming the version (never the key material). The "verify one row per key_version at start" check belongs to the secrets epic.
- The mocks must be cheap to construct (`Default`) and cloneable through `Arc`.

## Edge cases
- `MockEmailClient::sent()` must clone out of the mutex so tests never hold the lock across awaits.
- `SecretsKeyring::parse("")` returns `SecretsError::InvalidMasterKey`; whitespace around entries is trimmed.
- Do not log key material or the fake PAT anywhere (rule 3).

## Testing
- Unit tests: keyring parsing (valid entry, wrong length, bad base64, duplicate version, highest version selected as `current_version()`); mock email captures order; mock engine records calls.
- `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` must pass.

## Documentation
- None: implements the documented contract as written. The engine, email, git and secrets epics document their full traits.

## Assumes from other epics
- "Repository scaffolding, tooling and CI": `AppState` in `prelude/` with fields of these trait-object types (or placeholders to be replaced by these traits), the `integration-tests` feature, `zeroize`, `base64`, `thiserror` dependencies.