---
id: w2uhm
title: Implement GitCredentialProvider with the PAT provider, temp-config transport, secret_uses audit and mock
status: done
priority: P0
created: "2026-09-16T20:28:43.060799825Z"
updated: "2026-09-18T00:02:43.769656705Z"
tags:
  - orchestrator
  - git
  - secrets
depends_on:
  - bv7a5
  - escp8
parent: z4u4e
assignee: claude-opus-subagent
attempts: 1
---

## Summary
Implement the `GitCredentialProvider` trait from ADR 0002 with its v1 PAT implementation: read the project-scoped, orchestrator-only `GIT_CREDENTIAL` secret, turn it into an `http.extraHeader` value, and hand it to commands only through a temporary mode-0600 config file selected with `GIT_CONFIG_GLOBAL` that is deleted afterwards, even on failure. The provider also returns the bot commit identity from `GIT_BOT_NAME`/`GIT_BOT_EMAIL`, records a `secret_uses` row with `purpose = 'git'`, and has a mock behind the `integration-tests` feature.

## Documents
- `ARCHITECTURE.md` "Git model" -> "Credentials" and "Commit identity"
- `ARCHITECTURE.md` "Secrets" -> "Credential handling and transcripts" (zeroize buffers, secret names only in spans, no values in logs)
- `ARCHITECTURE.md` "Orchestrator internals" (`AppState` holds `Arc<dyn GitCredentialProvider>`; mocks with `as_any()`)
- `docs/data-model.md` `projects` (credential is the `GIT_CREDENTIAL` secret), `secret_uses` (`purpose = 'git'`, `session_id` for MCP, `user_id` for REST, neither for the mirror-fetch job)
- `README.md` "Configuration" (`GIT_BOT_NAME`, `GIT_BOT_EMAIL`)
- ADR 0002 (trait shape), ADR 0011 (superseded `-c` note)
- `CLAUDE.md` rule 3

## Acceptance criteria
- [ ] Trait in `orchestrator/src/git/credentials.rs`:
  ```rust
  #[async_trait] pub trait GitCredentialProvider: Send + Sync {
      async fn credential_for(&self, project_id: Uuid, actor: &GitActor, min_ttl: Duration) -> Result<Option<GitCredential>>;
      async fn commit_identity(&self, project_id: Uuid) -> Result<CommitIdentity>;
      fn as_any(&self) -> &dyn Any;
  }
  pub enum GitActor { User(Uuid), Session(Uuid), System }
  pub struct CommitIdentity { pub name: String, pub email: String }
  pub struct GitCredential { header_value: Zeroizing<String>, pub expires_at: Option<DateTime<Utc>> }
  ```
  `Ok(None)` means the project has no `GIT_CREDENTIAL` (public remote); callers run the command without a config file.
- [ ] `PatCredentialProvider` resolves the secret with scope `project`, `scope_id = project_id`, `name = "GIT_CREDENTIAL"` through the secrets module, builds `Authorization: Basic <base64("x-access-token:" + PAT)>`, and inserts a `secret_uses` row (`secret_id`, `purpose = 'git'`, `session_id` for `GitActor::Session`, `user_id` for `GitActor::User`, neither for `System`) in the same call.
- [ ] `CredentialConfig::write(&GitCredential) -> Result<CredentialConfig>` creates a file under `DATA_DIR/tmp/` with mode `0600` (created with `OpenOptions::new().mode(0o600).create_new(true)`) containing exactly `[http]\n\textraHeader = <header_value>\n`; `path()` is passed to `GitCommand::config_global`; `Drop` deletes the file (best effort, `warn!` on failure) so it disappears on error and on panic; a `close()` method deletes eagerly and reports errors.
- [ ] `GitCredential` never implements `Debug`/`Display` that reveals the header; `Zeroizing` clears it on drop; the PAT string is zeroized after building the header.
- [ ] `commit_identity` returns `CommitIdentity { name: config.git_bot_name, email: config.git_bot_email }`; `Config::from_env()` requires both variables (fail fast naming the missing one).
- [ ] `MockGitCredentialProvider` (feature `integration-tests`): configurable `Option<GitCredential>` per project (default `None`), fixed identity `Mars Bot <bot@example.invalid>`, records every `credential_for` call (`project_id`, `actor`) in a `Mutex<Vec<_>>` for assertions; `as_any()` downcast supported; wired into `TestApp`.
- [ ] `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Implementation notes
- Files: `orchestrator/src/git/credentials.rs` (trait, `GitActor`, `CommitIdentity`, `GitCredential`, `CredentialConfig`, `PatCredentialProvider`), `orchestrator/src/git/mock.rs` behind `#[cfg(feature = "integration-tests")]`, `AppState` field `git_credentials: Arc<dyn GitCredentialProvider>`, `Config` fields `git_bot_name`, `git_bot_email`.
- Add `zeroize`, `base64`, `tempfile` (regular dependency, for the tmp dir helper if preferred over manual `create_new`) with `cargo add`; `async-trait` if not already present.
- Secret read goes through the Secrets epic's decrypt path (something like `SecretsService::read_orchestrator_only(scope, scope_id, name) -> Result<Option<Zeroizing<String>>>`); if the winning row is not `orchestrator_only`, still use it (the project create path always stores it orchestrator-only; do not fail on a user-edited flag) but `warn!` with the secret name only.
- `min_ttl` is unused by the PAT implementation (PATs have no known expiry): return `expires_at: None`. The parameter exists so a GitHub App provider can refuse or refresh.
- Tracing: `info_span!("git_credential", project_id = %project_id, secret = "GIT_CREDENTIAL")`; never log the header or the PAT.
- `DATA_DIR/tmp` is created at startup if missing (the orphan-cleanup job empties it; stale `gitcfg-*` files are safe to delete).

## Edge cases
- Secret missing: `Ok(None)`; the caller proceeds unauthenticated and the remote decides (a 401/403 from GitHub surfaces as `GitError::Command`, mapped 500 with a generic message, logged with stderr; stderr from git does not echo the header).
- Two concurrent commands for one project each get their own config file (unique name `gitcfg-<uuid>`); they never share or reuse files.
- `secret_uses` insert failing must fail the operation (audit is mandatory), inside the same async call; no transaction spans the git command.
- PAT containing whitespace or newline: reject with `GitError::CredentialUnavailable` (would break the config syntax).

## Testing
- Unit tests: header encoding for a known fake PAT (`ghp_fakefakefake`), file mode is `0o600` (`std::os::unix::fs::PermissionsExt`), file removed after `drop`, file removed when a command run with it fails (run `git -c` nothing; e.g. `git fetch` against a nonexistent path), no `Debug` leak (`format!("{:?}", cred)` does not contain the PAT).
- Integration test via `TestApp` with the real `PatCredentialProvider` against the test keyring: store a fake `GIT_CREDENTIAL` for a project through the secrets repository, call `credential_for` with `GitActor::User(u)` and assert a `secret_uses` row with `purpose = 'git'`, `user_id = u`, `session_id IS NULL`; repeat with `GitActor::Session(s)` and `System`.
- Assert the argv recorded in a `GitError::Command` produced with a credential config attached contains no `Authorization` string.
- Command: `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- none: implements the documented contract as written (`README.md` already lists `GIT_BOT_NAME`/`GIT_BOT_EMAIL`; `.env.example` must carry them if the scaffolding epic did not add them).

## Assumes from other epics
- "Secrets manager": a decrypting read of a named project secret and the `secret_uses` repository insert (`SecretRepository::record_use` or equivalent).
- "Database schema, models, repositories and test harness": `Config`, `AppState`, `TestApp::spawn()` with a fixed master key and secrets tables.