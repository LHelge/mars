---
id: fcezm
title: Add the user, token and invite models and UserRepository with user-row and admin-membership locks
status: open
priority: P1
created: "2026-09-16T20:29:13.327304385Z"
updated: "2026-09-16T20:29:13.327304385Z"
tags:
  - orchestrator
  - core
  - auth
depends_on:
  - suzac
parent: p5tsd
---

## Summary
Add `User`, `RefreshToken`, `UserInvite` and `PasswordResetToken` domain types with `UserError` and the documented validation (username 3–32 chars, password 10–128 chars, email trimmed and lower-cased), and `UserRepository<'a>` with basic CRUD plus the two locking primitives the auth epic builds on: the user-row `FOR UPDATE` lock and the transaction-scoped advisory lock for administrator membership. Token-table queries beyond row types are the auth epic's.

## Documents
- `docs/data-model.md` "Users and authentication" (all four tables; the paragraphs on the admin-membership advisory lock and on locking the user row before token rows).
- `SPEC.md` "User-facing features" (passwords are 10–128 characters), "Users" (`User` DTO fields), "Authentication".
- `ARCHITECTURE.md` "Orchestrator internals" (repositories: one `XRepository<'a>` per aggregate borrowing `&PgPool`, scope in the `WHERE` clause, helpers accept the caller's transaction; `Error` has `#[from] UserError`).
- `CLAUDE.md` "Backend conventions".

## Acceptance criteria
- [ ] `models/user.rs`: `User` row struct with every column (`password_hash` and `auth_version` are `#[serde(skip)]`; the API DTO is `{ id, username, email, admin, must_change_password, notify_email, created_at }`), `Username::parse` (trim; 3–32 chars; `UserError::InvalidUsername`), `Password::parse` (10–128 chars; `UserError::InvalidPassword`; `Debug` redacted), `Email::parse` (trim, lower-case, exactly one `@` with non-empty local and domain parts; `UserError::InvalidEmail`).
- [ ] Row structs `RefreshToken`, `UserInvite`, `PasswordResetToken` mirror their tables.
- [ ] `UserError` is a `#[from]` variant of `prelude::Error` mapping to 400.
- [ ] `repositories/users.rs`: `UserRepository<'a> { pool: &'a PgPool }` with `insert(&self, tx: &mut PgConnection, user: &NewUser) -> Result<User>` (unique violation on `users_username_key` / `users_email_key` → `Error::Conflict("username already taken")` / `Error::Conflict("email already registered")`), `find(&self, id) -> Result<Option<User>>`, `find_by_username`, `find_by_email`, `list() -> Vec<User>` ordered by `created_at`, `update(&self, tx, id, UserUpdate { username?, admin?, notify_email? })` setting `updated_at = NOW()`, `delete(&self, tx, id) -> Result<bool>`, `count_admins(&self, tx) -> Result<i64>`.
- [ ] `lock_user(&self, tx, id) -> Result<Option<User>>` runs `SELECT ... FROM users WHERE id = $1 FOR UPDATE` and returns the row read under the lock.
- [ ] `lock_admin_membership(&self, tx) -> Result<()>` runs `SELECT pg_advisory_xact_lock($1)` with the constant `ADMIN_MEMBERSHIP_LOCK_KEY: i64` defined once in the repository; it must be called before any user-row lock in the same transaction (documented on the function).
- [ ] A `repositories/mod.rs` helper `fn unique_violation(err: &sqlx::Error) -> Option<&str>` returns the violated constraint name so every repository maps duplicates to `Error::Conflict` the same way.
- [ ] `cargo sqlx prepare` run and `.sqlx/` committed.

## Implementation notes
- Files: `orchestrator/src/models/user.rs`, `src/repositories/mod.rs`, `src/repositories/users.rs`, `src/prelude/error.rs`.
- Helpers take `&mut PgConnection` (obtained from `&mut *tx` on a `Transaction<'_, Postgres>`) so that callers can pass either a transaction or a pool connection; pool-only reads (`find`, `list`) use `self.pool` directly.
- All SQL through `sqlx::query!` / `query_as!`; enum-free table so no custom types here.
- `NewUser { id: Uuid, username: Username, email: Email, password_hash: String, admin: bool, must_change_password: bool }`; hashing itself is the auth epic's job.
- Write `updated_at = NOW()` in every `UPDATE`.
- Logging: `tracing::debug!(user_id = %id, ...)`, never the email in `info` or above, never a hash.

## Edge cases
- `delete` returns `Ok(false)` when no row matched (the route decides 404); it never checks the admin invariant itself, that composition (advisory lock → re-count → delete) is the auth epic's, but a repository test must show that two transactions calling `lock_admin_membership` serialize.
- `Email::parse` must not accept surrounding whitespace or uppercase in the stored value; the stored form is what the unique index sees.
- `Username::parse` counts `chars()`, not bytes.

## Testing
- Unit tests for every validation boundary (2/3/32/33-char usernames, 9/10/128/129-char passwords, email normalisation and rejection).
- Integration tests in `tests/repositories_users.rs` using `common::db::test_pool()`: insert/find/list/update/delete round trip; duplicate username → `Conflict`; duplicate email → `Conflict`; `lock_user` returns the row and a second transaction's `lock_user` on the same id waits until the first commits (use two pool connections and a `tokio::time::timeout`); `lock_admin_membership` serializes two transactions; `count_admins` reflects the seeded admin (1) then 0 after deleting it.
- `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` must pass.

## Documentation
- None: implements the documented contract as written.

## Assumes from other epics
- "Repository scaffolding, tooling and CI": `prelude::Error` with `Conflict(String)`, `NotFound`, `Internal(String)` variants and `#[from] sqlx::Error`.