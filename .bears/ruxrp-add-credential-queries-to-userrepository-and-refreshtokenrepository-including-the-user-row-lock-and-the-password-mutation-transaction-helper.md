---
id: ruxrp
title: Add credential queries to UserRepository and RefreshTokenRepository, including the user-row lock and the password-mutation transaction helper
status: open
priority: P0
created: "2026-09-16T20:27:42.652342083Z"
updated: "2026-09-16T20:27:42.652342083Z"
tags:
  - orchestrator
  - auth
parent: qacxf
---

## Summary
Extend the `UserRepository` and `RefreshTokenRepository` delivered by the schema epic with every query the auth flows need: lookup by username or email, `SELECT ... FOR UPDATE` on the user row, the refresh-token insert/lookup/revoke set, and one transaction-scoped helper that performs the atomic password mutation (hash update, `auth_version + 1`, clear `must_change_password`, revoke all refresh tokens, invalidate all reset tokens, optional replacement refresh token). All SQL goes through `sqlx::query!`/`query_as!` and every helper takes the caller's transaction so login, refresh, change and reset can compose them under the user-row lock.

## Documents
- `docs/data-model.md` `users` (paragraphs "Password changes and resets atomically ..." and "Login, refresh, reset-link issuance ... lock the user row ..."), `refresh_tokens`, `password_reset_tokens` (invalidation on password change).
- `ARCHITECTURE.md` "User authentication and revocation"; "Orchestrator internals" (repository conventions, scope in `WHERE`).
- `CLAUDE.md` "Backend conventions" (repositories accept the caller's transaction; `cargo sqlx prepare`).
- ADR 0025.

## Acceptance criteria
- [ ] `UserRepository`: `find_by_username(&str)`, `find_by_email(&str)` (both lower-cased/trimmed lookups; username exact), `find_by_username_or_email(identifier)`, `lock_for_update(tx, id) -> Option<User>` (`SELECT ... FOR UPDATE`), `update_notify_email(id, bool) -> Option<User>`, and `apply_password_change(tx, id, new_hash, replacement: Option<&str /*token hash*/>) -> Result<User>`.
- [ ] `apply_password_change` executes, in the caller's transaction, in this order: `UPDATE users SET password_hash = $2, auth_version = auth_version + 1, must_change_password = FALSE, updated_at = NOW() WHERE id = $1 RETURNING *`; `UPDATE refresh_tokens SET revoked_at = NOW() WHERE user_id = $1 AND revoked_at IS NULL`; `UPDATE password_reset_tokens SET used_at = NOW() WHERE user_id = $1 AND used_at IS NULL`; then, if `replacement` is `Some`, inserts the replacement refresh token with `expires_at = NOW() + 30 days`. Returns the updated user (new `auth_version`).
- [ ] `RefreshTokenRepository`: `insert(tx, user_id, token_hash, expires_at) -> RefreshToken`, `find_by_hash(hash) -> Option<RefreshToken>` (unlocked), `find_by_hash_for_user(tx, hash, user_id) -> Option<RefreshToken>` (re-read under the user lock; `WHERE token_hash = $1 AND user_id = $2`), `revoke(tx, id)` (`SET revoked_at = NOW() WHERE id = $1 AND revoked_at IS NULL`, returns whether a row changed), `revoke_by_hash(hash)`, `revoke_all_for_user(tx, user_id)`, `delete_expired()` (for the cron epic).
- [ ] Every mutation puts its scope in the `WHERE` clause; no read-then-check-then-write outside the lock.
- [ ] `.sqlx/` is refreshed with `cargo sqlx prepare` and committed; CI builds with `SQLX_OFFLINE=true`.
- [ ] `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Implementation notes
- Files: `orchestrator/src/repositories/users.rs`, `orchestrator/src/repositories/refresh_tokens.rs` (create if the schema epic did not), `orchestrator/.sqlx/`.
- Transaction type: `&mut sqlx::Transaction<'_, sqlx::Postgres>` (alias `Tx<'_>` in the prelude if the schema epic defined one; otherwise define it there in this task).
- `RefreshToken` model fields mirror the table: `id, user_id, token_hash, expires_at, revoked_at, created_at`.
- Locking order for callers (document in a module doc comment): user row lock first, then that user's token rows. Ordinary request authorisation (`find_by_id`) never takes the lock.
- `find_by_username_or_email` is used by password-reset requests: `WHERE username = $1 OR email = $1` after the caller trims and lower-cases the identifier for the email comparison; username comparison stays exact.

## Edge cases
- `apply_password_change` on a missing id returns `Error::NotFound` (no row from `RETURNING`).
- `lock_for_update` on a deleted user returns `None`; callers turn that into 401 or 404 as their contract says.
- Revoking an already-revoked token is a no-op returning `false`, never an error.
- Do not update `updated_at` when only tokens change; it is a `users` column.

## Testing
- Repository integration tests against `TestApp::spawn().pool` (one `#[tokio::test]` per scenario): insert user + two refresh tokens, `apply_password_change` with a replacement, assert `auth_version` incremented by exactly 1, `must_change_password` false, both old tokens have `revoked_at`, outstanding reset tokens have `used_at`, and exactly one unrevoked refresh token remains (the replacement); the same without replacement leaves zero unrevoked tokens.
- `find_by_hash_for_user` returns `None` for a hash belonging to another user.
- `lock_for_update` blocks a second transaction until commit (open two transactions, assert the second `SELECT ... FOR UPDATE` completes only after the first commits, using `tokio::time::timeout`).
- Command: `cd orchestrator && cargo sqlx prepare && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Database schema, models, repositories and test harness": the `users` migration (tables `users`, `refresh_tokens`, `user_invites`, `password_reset_tokens`), `UserRepository<'a>` with basic CRUD (`find_by_id`, `insert`, `list`, `delete`), the `RefreshToken` model struct and `TestApp::spawn()` exposing the pool.