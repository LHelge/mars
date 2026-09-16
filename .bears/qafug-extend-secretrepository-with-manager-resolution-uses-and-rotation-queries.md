---
id: qafug
title: Extend SecretRepository with manager, resolution, uses and rotation queries
status: open
priority: P1
created: "2026-09-16T20:30:14.314571587Z"
updated: "2026-09-16T20:30:14.314571587Z"
tags:
  - orchestrator
  - secrets
depends_on:
  - xzahq
parent: t36d2
---

## Summary
Extend the schema epic's `SecretRepository<'a>` with every query the secrets manager, the launch resolver, the git credential lookup and the rotation sweep need, all through `sqlx::query!`/`query_as!` with the scope in the `WHERE` clause and helpers accepting the caller's transaction. Refresh `.sqlx/` so CI builds offline.

## Documents
- `docs/data-model.md` "Secrets" (`secrets`, `secret_uses`, indexes, `UNIQUE NULLS NOT DISTINCT (scope, scope_id, name)`, `CHECK ((scope = 'global') = (scope_id IS NULL))`, `secret_uses.purpose` values `launch` / `git`)
- `SPEC.md` "Secrets (`/api/secrets`)" (`SecretMeta` includes `last_used_at`; uses list shape `{session_id, user_id, purpose, at}`)
- `ARCHITECTURE.md` "Secrets" → "Rotation" (batches of 100, one statement per row), "Resolution at launch"
- `CLAUDE.md` "Backend conventions" (repositories hold all SQL; `cargo sqlx prepare`)

## Acceptance criteria
- [ ] Two read shapes: `SecretRow` (all columns) and `SecretMetaRow` (all columns except the four crypto `BYTEA`s, plus `last_used_at: Option<DateTime<Utc>>` = `MAX(secret_uses.at)`). No method returns ciphertext to a caller that only needs metadata.
- [ ] `insert(tx, NewSecret { id, scope, scope_id, name, encrypted: &EncryptedSecret, orchestrator_only, created_by }) -> Result<SecretMetaRow>` maps a unique violation on `(scope, scope_id, name)` to `Error::Conflict("secret already exists")`.
- [ ] `find_meta(id) -> Option<SecretMetaRow>`; `find_row_for_update(tx, id) -> Option<SecretRow>` (`SELECT ... FOR UPDATE`, for rename/replace); `list_meta(filter: SecretListFilter { scope: Option<SecretScope>, scope_id: Option<Uuid>, user_ids: UserFilter::All | UserFilter::Only(Vec<Uuid>) })` ordered by `scope, name`.
- [ ] `update_value(tx, id, &EncryptedSecret)` sets the five crypto columns and `updated_at = NOW()`; `update_name_and_value(tx, id, name, orchestrator_only, &EncryptedSecret)` sets name, flag, `ciphertext`, `nonce`, `updated_at` in one statement (rename), mapping the unique violation to `Conflict`; `update_flag(tx, id, orchestrator_only)`; `delete(tx, id) -> bool`.
- [ ] `find_for_resolution(names: &[String], project_id: Uuid, user_id: Option<Uuid>) -> Vec<SecretRow>` returns every row whose `name = ANY($1)` and whose `(scope, scope_id)` is `('global', NULL)`, `('project', $2)` or `('user', $3)` in one query.
- [ ] `find_by_scope_name(scope, scope_id, name) -> Option<SecretRow>` and `exists_by_scope_name(scope, scope_id, name) -> bool` (used for `GIT_CREDENTIAL` and `has_credential`).
- [ ] `insert_use(tx, secret_id, session_id: Option<Uuid>, user_id: Option<Uuid>, purpose: &str)` and `list_uses(secret_id, limit: i64) -> Vec<SecretUseRow { session_id, user_id, purpose, at }>` ordered `at DESC` (uses `secret_uses_secret_idx`).
- [ ] `select_rotation_batch(newest_version: i32, limit: i64) -> Vec<RotationRow { id, key_version, data_key_wrapped, data_key_nonce }>` with `WHERE key_version < $1 ORDER BY id LIMIT $2`; `update_wrap(id, expected_old_version, data_key_wrapped, data_key_nonce, new_version) -> bool` with `WHERE id = $1 AND key_version = $2` so a concurrent replace is never overwritten; `count_below_version(newest) -> i64`.
- [ ] `distinct_key_version_samples() -> Vec<(key_version, data_key_wrapped, data_key_nonce)>` (the startup verification query from task 1, if not already added there).
- [ ] `scope_exists(scope, scope_id) -> bool` checks `users` or `projects` by id (`global` → true).
- [ ] `cargo sqlx prepare` output committed under `orchestrator/.sqlx/`; CI with `SQLX_OFFLINE=true` builds.

## Implementation notes
- File: the schema epic's repository file (`orchestrator/src/repositories/secrets.rs` or its actual name); keep the `XRepository<'a>` borrowing `&PgPool` pattern; transactional helpers take `&mut PgConnection` (works for both a transaction and the pool).
- `last_used_at` select: `SELECT s.id, s.scope AS "scope: SecretScope", ..., (SELECT MAX(u.at) FROM secret_uses u WHERE u.secret_id = s.id) AS last_used_at FROM secrets s ...` (avoid `GROUP BY` over the `BYTEA`s).
- `list_meta` visibility is computed by the service (task 4); the repository only applies the filter: `($1::secret_scope IS NULL OR scope = $1) AND ($2::uuid IS NULL OR scope_id = $2) AND (scope <> 'user' OR $3::bool OR scope_id = ANY($4))`.
- Unique violation detection: `sqlx::Error::Database(e) if e.is_unique_violation()`; match on the constraint name from the migration to be precise.
- Purpose strings are constants `SECRET_USE_LAUNCH = "launch"` and `SECRET_USE_GIT = "git"` in `orchestrator/src/models/secret.rs`.
- No project or session row locks here: secrets are not part of the tracker or event transactions and take only their own row locks.

## Edge cases
- `find_for_resolution` with `user_id = None` (session created by a deleted user) must not match any `user` row; write `(scope = 'user' AND $3::uuid IS NOT NULL AND scope_id = $3)` explicitly.
- A CHECK-constraint violation on `scope`/`scope_id` maps to `Error::BadRequest`, but the service validates before the insert.
- `update_wrap` returning `false` is not an error; the caller counts it as skipped.
- `list_uses` with `limit <= 0` is the caller's bug; clamp to 1.

## Testing
- Integration tests in `orchestrator/tests/secrets_repository.rs` via `TestApp` (seal test values with `crypto::seal` and the TestApp keyring): insert then duplicate → `Conflict`; `find_for_resolution` returns only the three scopes for the given ids and nothing for a foreign project/user; `list_meta` filters for each `UserFilter`; `list_uses` ordering and limit; `select_rotation_batch` excludes newest rows and honours the limit; `update_wrap` with a stale expected version returns `false`; `last_used_at` is `None` before and set after `insert_use`; `scope_exists` for a deleted user is `false`; `delete` returns `false` for an unknown id.
- `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Database schema, models, repositories and test harness": the `secrets` migration with its constraint names, the basic `SecretRepository`, `Secret`/`SecretScope` models with `sqlx::Type`, `TestApp`.