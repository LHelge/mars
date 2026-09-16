---
id: "22tk6"
title: Add stream token authentication (?token=) and periodic re-authorization shared by the WebSocket and SSE handlers
status: open
priority: P0
created: "2026-09-16T20:44:23.709611292Z"
updated: "2026-09-16T20:51:52.036638938Z"
tags:
  - orchestrator
  - realtime
  - auth
depends_on:
  - s52qg
  - "5h3y4"
parent: h8kw9
---

## Summary
Provide `routes/stream_auth.rs`: the `?token=` extraction and validation both stream endpoints use at open, validated exactly like the `Authorization` header (signature, expiry, current user, `auth_version`, password-change gate), and the `reauthorize` check the streams run before each WebSocket application message and at ping/keepalive ticks (account exists, `auth_version` unchanged, gate not set). It fixes the one rule both handlers share: a database failure during a check never authorizes.

## Documents
- `SPEC.md` "Authentication": "WebSocket and SSE endpoints cannot receive headers from the browser, so they accept the access token as `?token=` and validate it exactly like the header"; "Token signature and expiry are checked when a stream opens; an open stream is not closed merely because that token later expires. Account existence, `auth_version` and the password-change gate are checked again before every incoming WebSocket application message (including terminal bytes), and at the existing WebSocket ping / SSE keepalive ticks (30 / 15 seconds). A failed authorization check closes the stream ... Database failures must not authorize input or continued streaming; close and let normal retry handle them."; the gate answers 403 with error `password change required`.
- `ARCHITECTURE.md` "User authentication and revocation".
- `docs/data-model.md` `users` (ordinary authorization reads current state without the mutation lock).
- ADR 0025.

## Acceptance criteria
- [ ] `pub struct StreamToken(pub String)` implements `FromRequestParts<AppState>`: reads `token` from the query string; missing or empty → `Error::Unauthorized` (401 `{ "status": 401, "error": "authentication required" }`). An `Authorization: Bearer` header is also accepted when present (tests and non-browser clients); the query parameter takes precedence when both exist.
- [ ] `pub async fn authenticate_stream(state: &AppState, token: &str) -> Result<User>`: calls `authenticate_access_token` (401 on invalid/expired token, missing user or `auth_version` mismatch) and then returns `Error::Forbidden("password change required")` when `user.must_change_password` is true. Same outcomes as the `CurrentUser` extractor for the same token.
- [ ] `pub struct StreamPrincipal { pub user_id: Uuid, pub auth_version: i64 }` captured at open, and `pub async fn reauthorize(state: &AppState, principal: &StreamPrincipal) -> Result<(), StreamAuthFailure>`: `UserRepository::find_by_id`; `Err(StreamAuthFailure::Revoked)` when the row is missing, `auth_version` differs or `must_change_password` is true; `Err(StreamAuthFailure::Unavailable)` on any database error. Both variants mean "close the stream"; handlers must never treat `Unavailable` as success.
- [ ] `reauthorize` does not decode or re-validate the JWT (expiry of the opening token is deliberately not rechecked) and takes no row lock.
- [ ] `pub const AUTH_REQUIRED: &str = "authentication required";` is exported so the WebSocket `error` message and the 401 body share one string.
- [ ] `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Implementation notes
- Files: `orchestrator/src/routes/stream_auth.rs` (new), `orchestrator/src/routes/mod.rs` (`pub mod stream_auth;`); reuse `authenticate_access_token` from `routes/extractors.rs`.
- Query parsing through a private `#[derive(Deserialize)] struct TokenQuery { token: Option<String> }` with `axum::extract::Query`; other query parameters (`after`) are parsed by the handlers themselves, so use `Query<TokenQuery>` with `#[serde(default)]` and ignore unknown fields.
- Never log the token; log failed checks at `debug` with `user_id = %id` only. Implement `Debug` for `StreamToken` by hand printing `StreamToken(<redacted>)`.
- The `TraceLayer` span already excludes the query string (startup task); do not add request logging here.

## Edge cases
- Token present in both header and query with different values: the query value is used; no error.
- `reauthorize` racing a password change: before the change commits the check passes, after it fails; the one-tick lag is documented as acceptable (`SPEC.md` "Authentication": "Revocation detection on idle streams can lag by one heartbeat interval").
- Deleted user: `find_by_id` returns `None` → `Revoked`.

## Testing
- Integration tests in `orchestrator/tests/stream_auth.rs` through a `#[cfg(feature = "integration-tests")]` probe route `GET /api/test/stream-whoami` that takes `StreamToken`, calls `authenticate_stream` and returns `{ "user_id" }`: no `token` → 401; garbage → 401; expired → 401; `auth_version` one behind → 401; `must_change_password` → 403 `password change required`; valid → 200; header-only → 200. Tests on `TestApp.pool` for `reauthorize`: valid principal → `Ok`; after `UPDATE users SET auth_version = auth_version + 1` → `Revoked`; after `must_change_password = true` → `Revoked`; after deleting the row → `Revoked`; with a closed pool (`pool.close().await` on a second pool built for the test) → `Unavailable`.
- Command: `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Authentication, users, invites and email": `authenticate_access_token(state, token) -> Result<User>` in `routes/extractors.rs`, `Error::Unauthorized`, `Error::Forbidden(msg)`, `UserRepository::find_by_id`, and `TestApp::token_for` / `expired_token_for` helpers.