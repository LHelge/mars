---
id: dm7hz
title: Add the secret models and SecretRepository with scoped CRUD and usage audit rows
status: done
priority: P1
created: "2026-09-16T20:33:06.510749584Z"
updated: "2026-09-17T07:38:11.496161580Z"
tags:
  - orchestrator
  - core
  - secrets
depends_on:
  - yn4pr
parent: p5tsd
assignee: claude-opus-subagent
attempts: 1
---

## Summary
Add the `Secret` and `SecretUse` domain types with validation (`SecretScope`, the environment-variable-style name pattern, the scope/scope_id rule, the `purpose` values) and `SecretRepository<'a>` with the scoped CRUD, the value/rename update statements, the rotation sweep query and the `secret_uses` audit insert. The envelope crypto, AAD, resolution order and the keyring startup check are the secrets epic's; this repository only moves the encrypted columns.

## Documents
- `docs/data-model.md` "Secrets" (`secrets`, `secret_uses`, AAD sentence: renaming re-encrypts under the new AAD in one transaction), "Enums" (`secret_scope`).
- `ARCHITECTURE.md` "Secrets" ("Rotation": batches of 100 rows with `key_version < newest`, one statement per row updating `data_key_wrapped`, `data_key_nonce`, `key_version`; "Resolution at launch": lookup order global, project, user; a `secret_uses` row per injected secret).
- `SPEC.md` "Secrets" (`SecretMeta` fields incl. `last_used_at`; `GET /secrets/{id}/uses` `?limit=`).

## Acceptance criteria
- [ ] `SecretScope { Global, User, Project }` derives `sqlx::Type` (`secret_scope`) and serde snake_case; `SecretName::parse` enforces `^[A-Z][A-Z0-9_]{0,127}$` (`SecretError::InvalidName`); `ScopeRef::new(scope, scope_id)` enforces `scope == Global` iff `scope_id.is_none()` (`SecretError::InvalidScope`); `SecretUsePurpose { Launch, Git }` serialises as `launch` / `git` and is stored as TEXT.
- [ ] `Secret` row struct carries every column; `ciphertext`, `nonce`, `data_key_wrapped`, `data_key_nonce` are `Vec<u8>` and `#[serde(skip)]`, `Debug` is redacted for those fields. `SecretMeta` (the DTO shape) is a separate struct with `last_used_at: Option<DateTime<Utc>>`.
- [ ] `SecretError` is a `#[from]` variant of `prelude::Error` (400).
- [ ] `SecretRepository<'a>`: `insert(tx, &NewSecret)` (unique `secrets_scope_scope_id_name_key` → `Conflict("secret already exists")`), `find(id) -> Option<Secret>`, `find_by_name(scope, scope_id, name) -> Option<Secret>`, `list_meta(scope, scope_id) -> Vec<SecretMeta>` (joins `MAX(secret_uses.at)` as `last_used_at`, ordered by `name`), `update_value(tx, id, EncryptedValue { ciphertext, nonce, data_key_wrapped, data_key_nonce, key_version })`, `rename(tx, id, name, EncryptedValue)` (one `UPDATE` setting `name` and the re-encrypted columns together; unique violation → `Conflict`), `set_orchestrator_only(tx, id, bool)`, `delete(tx, id) -> bool`; every write sets `updated_at = NOW()`.
- [ ] `list_for_rotation(below_version: i32, limit: i64 = 100) -> Vec<Secret>` and `rewrap(tx, id, data_key_wrapped, data_key_nonce, key_version)` (one statement; `ciphertext` untouched); `distinct_key_versions() -> Vec<i32>` for the startup check.
- [ ] `list_orphans() -> Vec<Uuid>`: `user`-scoped rows whose `scope_id` is not in `users`, `project`-scoped rows whose `scope_id` is not in `projects` (for the token-cleanup reaper).
- [ ] `insert_use(tx, secret_id, session_id: Option<Uuid>, user_id: Option<Uuid>, purpose)` and `list_uses(secret_id, limit) -> Vec<SecretUse>` ordered by `at DESC`.
- [ ] Nothing in this module logs a value, ciphertext or key (`tracing` fields are `secret_id` and `name` only).
- [ ] `cargo sqlx prepare` run and `.sqlx/` committed.

## Implementation notes
- Files: `orchestrator/src/models/secret.rs`, `src/repositories/secrets.rs`, `src/prelude/error.rs`.
- `BYTEA` binds as `&[u8]` / `Vec<u8>`.
- `find_by_name` must handle the NULL `scope_id` case with `scope_id IS NOT DISTINCT FROM $2`.
- Resolution (`global` → `project` → `user`, last found wins, orchestrator-only exclusion) is composed by the secrets epic from `find_by_name`; do not implement it here.

## Edge cases
- `rename` to the same name is a no-op that still must re-encrypt? No: the caller decides; the repository just writes what it is given.
- `list_meta` for a scope with no rows returns an empty vector, not `NotFound`.
- `insert_use` with both `session_id` and `user_id` `None` is valid (mirror-fetch job).

## Testing
- Unit tests: name pattern (accepts `A`, `GIT_CREDENTIAL`, 128 chars; rejects lowercase, leading digit, 129 chars, empty), scope rule, purpose serialisation.
- Integration tests in `tests/repositories_secrets.rs` on `common::db::test_pool()` with fake byte values: insert/find/list/update/rename/delete; duplicate `(scope, scope_id, name)` → `Conflict` including two `global` rows with the same name (NULLS NOT DISTINCT); `list_for_rotation` returns only rows below the version and at most `limit`; `rewrap` leaves `ciphertext` unchanged; `insert_use` + `list_meta.last_used_at`; `list_orphans` after deleting the owning user.
- `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` must pass.

## Documentation
- None: implements the documented contract as written.

## Assumes from other epics
- none.