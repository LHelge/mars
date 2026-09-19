---
id: "7erm7"
title: "Add the login-credential module: issue, rotate and revoke under the user lock with lifetimes, cookie and claims in one place; routes become HTTP translation"
status: in_progress
priority: P2
created: "2026-09-17T20:01:08.063968601Z"
updated: "2026-09-19T11:42:57.533083814Z"
tags:
  - orchestrator
  - auth
  - architecture
  - docs
depends_on:
  - "48ke5"
  - trxaf
parent: wju32
attempts: 1
---

## Summary
Concentrate credential issuance in one module so that "lock the user row, revalidate what was read, mutate, commit, then mint the access token and refresh cookie" is a function a route calls, not a sequence a route remembers. The five handlers in `routes/auth.rs` (login, refresh, request reset, reset, accept invite) and the two in `routes/users.rs` (password changes) become thin: parse the request, call the module, shape the response. Lifetimes, the cookie name and attributes, and the `Claims` minting have one home.

## Documents
- `ARCHITECTURE.md` "Orchestrator internals" (module tree; `AppState` fields) and "User authentication and revocation".
- `SPEC.md` "Authentication" (15-minute access token, 30-day refresh cookie, cookie attributes, invite 7 days, reset 1 hour, throttling) and "Auth (`/api/auth`)".
- `docs/data-model.md` `refresh_tokens`, `password_reset_tokens`, `user_invites`, `users` (`auth_version`).
- ADRs 0013, 0025, 0026.

## Acceptance criteria
- [ ] New module `orchestrator/src/auth/` (`mod.rs`, `credentials.rs`, `cookies.rs` moved from `routes/`) exposing a `Credentials` (or `LoginCredentials`) type built from `AppState` with operations of the shape: `login(username, password, client) -> Result<IssuedPair>`, `refresh(presented_cookie) -> Result<IssuedPair>`, `logout(presented_cookie)`, `accept_invite(token, username, password) -> Result<IssuedPair>`, `reset_password(token, password)`, `change_password(user_id, current, new, replace_for_browser: bool) -> Result<Option<IssuedPair>>`, `request_password_reset(identifier)`. `IssuedPair { user, access_token, refresh_cookie: Cookie }`.
- [ ] Every operation that issues a pair does so only after its transaction commits, and every operation that reads a credential row re-reads it under `lock_user` before mutating; this is inside the module, so no route can get it wrong.
- [ ] `Claims::encode`, `refresh_cookie`, `clear_refresh_cookie` and `OpaqueToken::generate` have no production caller outside `auth/` (tests may still use them).
- [ ] The refresh lifetime exists once: `REFRESH_TOKEN_TTL` is passed as a bound parameter; the `INTERVAL '30 days'` literal in `repositories/users.rs::apply_password_change` is gone. `INVITE_TTL` in the prelude and `INVITE_TTL_DAYS` in `user_invites.rs` become one constant with one `expires_at` helper. `PASSWORD_RESET_TTL` likewise. The dead `Config::public_url_is_https` goes and `cookies::is_https` is the one rule.
- [ ] The cookie name is read only inside `auth/` (`presented_token` moves in).
- [ ] The login throttle is driven from inside `login` (check before hashing, `record_failure` on each rejection, `record_success` after commit); `BlockedUntil::retry_after` either sets a `Retry-After` header on the 429 (then `SPEC.md` "Authentication" says so) or is deleted.
- [ ] `routes/auth.rs` and `routes/users.rs` contain no `pool.begin()`, `lock_user`, `issue_pair` or `Claims` usage.
- [ ] `cargo sqlx prepare` run and `.sqlx/` committed.

## Implementation notes
- Files: `orchestrator/src/auth/{mod,credentials,cookies}.rs`, `orchestrator/src/routes/auth.rs`, `orchestrator/src/routes/users.rs`, `orchestrator/src/routes/cookies.rs` (removed), `orchestrator/src/repositories/users.rs`, `orchestrator/src/repositories/user_invites.rs`, `orchestrator/src/prelude/mod.rs`, `orchestrator/src/lib.rs`, `ARCHITECTURE.md`, `SPEC.md` (only if `Retry-After` is added).
- `extractors.rs` and `authenticate_access_token` stay where they are; they are the read side and already the deepest seam in the area.
- `apply_password_change` keeps owning the ADR 0025 quartet (bump version, revoke, spend reset tokens, optional replacement) but takes the expiry as a parameter or computes it from the one constant.
- `LogEmailClient` link logging (ADR 0026) is unchanged; the link is built in one helper (`public_url` join with the trailing slash trimmed once) used by invite and reset.

## Edge cases
- `accept_invite` locks the invite row, not a user row (the user does not exist yet); the module documents this as the one exception to "lock the user".
- `change_own_password` must not issue a pair before `apply_password_change` commits; with the pair issued inside the module this is structural.
- A 401 on refresh still clears the cookie; the module returns the clearing cookie as part of its error path or the route asks the module for it, never builds it.

## Testing
- The existing `tests/auth_*.rs` and `tests/users_password.rs` keep passing through HTTP with no assertion changes except where a message string moved.
- New `tests/auth_credentials.rs` drives the module directly on `TestApp`: login issues a pair with the documented cookie attributes (assert once here, not in three route tests); refresh rotates and revokes the presented token; a refresh racing a password change loses (reuse `tests/common/races.rs`); reset spends the token and revokes every refresh token.
- `cd orchestrator && cargo fmt && cargo clippy --all-targets -- -D warnings && cargo clippy --all-targets --features integration-tests -- -D warnings && cargo test --features integration-tests` passes.

## Documentation
- `ARCHITECTURE.md` "Orchestrator internals": add `├── auth/   login credentials: issue, rotate, revoke; cookie; throttle wiring` to the module tree; state that `AppState` holds the throttle and rate limit as before.
- `ARCHITECTURE.md` "User authentication and revocation": one sentence naming the module as the only issuer of access tokens and refresh cookies.
- `SPEC.md` "Authentication": only if `Retry-After` is added.

## Assumes from other epics
- Nothing new; auth epic (`qacxf`) is done.