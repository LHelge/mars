---
id: "47myw"
title: "Add revocation and last-administrator concurrency tests: refresh racing a password change, concurrent demotions, deletion racing demotion"
status: open
priority: P1
created: "2026-09-16T20:32:24.126809660Z"
updated: "2026-09-16T20:32:24.126809660Z"
tags:
  - orchestrator
  - auth
  - tests
depends_on:
  - "9jv8t"
  - "8pnnv"
parent: qacxf
---

## Summary
Write the cross-cutting concurrency suite the epic's acceptance criteria require: a refresh racing a password change must either commit before it (and be revoked by it) or fail, never produce a live credential under the old `auth_version`; and the last-administrator invariant must hold under concurrent demotions and under deletion racing demotion. The tests use both a deterministic form (hold the relevant lock in a manual transaction while the racing request is in flight) and a repeated-race form so the ordering rules in the repository and route tasks are verified, not just asserted.

## Documents
- `SPEC.md` "Authentication" bullet 5 ("a concurrent refresh or login cannot escape revocation with an old credential").
- `SPEC.md` "Users (`/api/users`)" last paragraph (concurrent requests cannot each remove one of the final two administrators; a rejected request changes no fields).
- `docs/data-model.md` `users` (advisory lock re-read rule; "Thus a refresh either commits before a password change and is revoked by it, or observes revocation and fails"; "Validation must cover concurrent demotions and deletion racing with demotion").
- `ARCHITECTURE.md` "User authentication and revocation"; ADR 0025.
- `CLAUDE.md` "Testing expectations".

## Acceptance criteria
- [ ] `tests/auth_revocation_race.rs`, deterministic: open a transaction on `TestApp.pool`, `SELECT ... FOR UPDATE` the user row, spawn `POST /auth/refresh` with a valid cookie (it blocks on the lock), run `UserRepository::apply_password_change` inside the locking transaction and commit; assert the refresh answers 401 with a clearing cookie and that no unrevoked refresh token exists for the user.
- [ ] `tests/auth_revocation_race.rs`, mirrored order: spawn the refresh first so it commits, then run the self-service password change through the API; assert the refresh's returned access token is rejected (401) on `GET /users/me` and its cookie answers 401 on refresh, and that exactly one unrevoked refresh token (the change's replacement) remains.
- [ ] Repeated race: for 20 iterations, `tokio::join!` a refresh and an admin password change for the same user; after each iteration assert that every access token the refresh returned (if 200) has `auth_version` lower than the row's and is rejected, and that unrevoked tokens for the user number zero (admin change issues no replacement).
- [ ] `tests/users_last_admin_race.rs`: with exactly two administrators A and B, `join!` `PUT /users/{B}` `{admin:false}` as A and `PUT /users/{A}` `{admin:false}` as B; exactly one answers 200 and one 409; `SELECT COUNT(*) FROM users WHERE admin` is 1; repeat 20 times with fresh admins. Deterministic variant: hold `pg_advisory_xact_lock(ADMIN_MEMBERSHIP_LOCK)` in a manual transaction, demote A through the pool inside it, commit, and assert the in-flight demotion of B by A returns 409 with B unchanged.
- [ ] Deletion racing demotion: A `DELETE /users/{B}` while B `PUT /users/{A}` `{admin:false}`; exactly one succeeds; at least one administrator remains; the rejected request left no field changed (username in the same `PUT` body not applied).
- [ ] Login racing a password change: a login whose password check passed before the row lock re-verifies under the lock and answers 401 when the hash changed (deterministic: lock row, change hash, commit, observe 401).
- [ ] All tests are single-scenario `#[tokio::test]`s, use `tokio::time::timeout` around blocking steps so a deadlock fails fast (10 s), and pass reliably 10 runs in a row locally.

## Implementation notes
- Files: `orchestrator/tests/auth_revocation_race.rs`, `orchestrator/tests/users_last_admin_race.rs`; helpers in `tests/common/mod.rs` (`hold_user_lock(pool, id) -> Transaction`, `hold_admin_membership_lock(pool) -> Transaction`, `unrevoked_refresh_tokens(pool, user_id) -> i64`, `admin_count(pool) -> i64`).
- `ADMIN_MEMBERSHIP_LOCK` must be `pub` from `repositories::users` so the test can take the same lock.
- Use `TestApp::create_admin` / `create_user` from the test-route task when available, otherwise repository inserts.
- Keep race iterations modest (20) to hold CI time; the deterministic variants are the guarantee, the repeated races are the smoke check.

## Edge cases
- If the refresh commits first in the repeated race and the password change then runs, the refresh's 200 is legitimate; the assertion is on the credential's validity afterwards, not on the status.
- A join where both requests return 409 indicates a bug (neither should fail when they are serialised); assert exactly one success.
- Pool size: `TestApp` must have at least 3 connections so a held transaction cannot starve the racing request and the checker.

## Testing
- This task is the test suite; the command that must pass is `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- none: verifies the documented contract.

## Assumes from other epics
- "Database schema, models, repositories and test harness": `TestApp` exposes the `PgPool` and a pool size of at least 3.