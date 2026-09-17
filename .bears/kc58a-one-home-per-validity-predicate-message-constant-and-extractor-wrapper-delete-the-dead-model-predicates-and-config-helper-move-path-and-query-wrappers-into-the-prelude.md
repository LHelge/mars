---
id: kc58a
title: "One home per validity predicate, message constant and extractor wrapper: delete the dead model predicates and Config helper, move Path and Query wrappers into the prelude"
status: open
priority: P2
created: "2026-09-17T20:03:18.554114346Z"
updated: "2026-09-17T20:03:18.554114346Z"
tags:
  - orchestrator
  - auth
  - architecture
  - docs
depends_on:
  - "7erm7"
parent: wju32
---

## Summary
Remove the second copies the survey found around auth and the HTTP layer. `UserInvite::is_usable` and `PasswordResetToken::is_usable` duplicate the `WHERE` clauses of `find_open_by_hash` and `find_valid_by_hash_for_user` and have no production caller, while `RefreshToken::is_usable` is checked in Rust; pick one rule (the query decides validity for lookups; the model has no predicate) and apply it to all three. `AUTHENTICATION_REQUIRED` and `ADMIN_REQUIRED` are spelled in three files and the frontend routes on the exact text. `ARCHITECTURE.md` promises that extractor rejections answer as `{ "status", "error" }`, but only `routes/secrets.rs` has the `Path`/`Query` wrappers that make it true; `auth.rs` and `users.rs` answer path rejections in axum's plain text.

## Documents
- `ARCHITECTURE.md` "Orchestrator internals" → Errors (extractor rejections convert into `BadRequest`; the prelude's `Json<T>` wrapper).
- `SPEC.md` "REST API" (error shape), "Auth" and "Users" (401/403 messages the frontend matches on).
- `docs/data-model.md` `user_invites`, `password_reset_tokens`, `refresh_tokens` (validity: unaccepted and unexpired; single use).

## Acceptance criteria
- [ ] `prelude/error.rs` (or `prelude/extract.rs`) exports `Path<T>` and `Query<T>` wrappers with `Error` as rejection, beside the existing `Json<T>`; `routes/secrets.rs` drops its local ones; `routes/auth.rs`, `routes/users.rs`, `routes/health.rs` and every future route import the prelude's. A test per route module (or one shared test) asserts that a malformed path id answers `400 { "status": 400, "error": ... }`.
- [ ] `UserInvite::is_usable`, `PasswordResetToken::is_usable` and `RefreshToken::is_usable` are removed; the repositories expose `find_usable_by_hash_for_user` for refresh tokens with the validity in SQL, and the credential module's refresh path uses it under the lock. `docs/data-model.md` states for each token table that validity is decided by the lookup query.
- [ ] `AUTHENTICATION_REQUIRED` and `ADMIN_REQUIRED` are defined once in `prelude` (or `auth/`) and imported everywhere; `SPEC.md` "Authentication" quotes the two strings so the frontend contract is written down.
- [ ] `Config::public_url_is_https` and its tests are deleted (the cookie rule in `auth/cookies.rs` is the one home); `throttle.rs` loses `BlockedUntil::retry_after` if the previous task did not wire `Retry-After`.
- [ ] `cargo sqlx prepare` run and `.sqlx/` committed.

## Implementation notes
- Files: `orchestrator/src/prelude/{error,mod}.rs`, `orchestrator/src/routes/{auth,users,secrets,health,extractors}.rs`, `orchestrator/src/models/user.rs`, `orchestrator/src/repositories/{refresh_tokens,user_invites,password_reset_tokens}.rs`, `orchestrator/src/prelude/config.rs`, `orchestrator/src/routes/throttle.rs`, `SPEC.md`, `docs/data-model.md`.
- The refresh path today locks the user, re-reads the token and checks `is_usable` in Rust so that the check happens under the lock; `find_usable_by_hash_for_user(tx, ...)` run inside the locked transaction preserves that.

## Edge cases
- `tests/error_mapping.rs` and `tests/extractors.rs` may already cover parts of this; extend rather than duplicate.
- The frontend does not exist yet beyond a placeholder; the message strings are a contract for the frontend foundation epic, hence the `SPEC.md` line.

## Testing
- `cd orchestrator && cargo fmt && cargo clippy --all-targets -- -D warnings && cargo clippy --all-targets --features integration-tests -- -D warnings && cargo test --features integration-tests` passes.

## Documentation
- `SPEC.md` "Authentication": the exact `401`/`403` error strings the frontend matches on.
- `docs/data-model.md`: one sentence per token table that the lookup query decides validity.
- `ARCHITECTURE.md` Errors paragraph: mention `Path<T>` and `Query<T>` beside `Json<T>`.