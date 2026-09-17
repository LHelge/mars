---
id: qx67f
title: Implement CurrentUser, UngatedUser and AdminUser extractors with auth_version check and the must-change-password gate
status: in_progress
priority: P0
created: "2026-09-16T20:29:01.213257292Z"
updated: "2026-09-17T09:08:36.486605835Z"
tags:
  - orchestrator
  - auth
depends_on:
  - rb2xf
parent: qacxf
assignee: claude-opus-subagent
attempts: 1
---

## Summary
Implement the Axum extractors every authenticated route uses. `CurrentUser` validates the bearer JWT's signature and expiry, loads the current user row, requires the claim's `auth_version` to match, and applies the `must_change_password` gate (403 `password change required`). `UngatedUser` does the same without the gate for the documented exceptions (`GET /users/me`, `POST /users/{id}/password`). `AdminUser` wraps `CurrentUser` and requires `admin` from the database row, never from the token. A shared `authenticate_access_token` function carries the header-independent part so the WebSocket/SSE epic can validate `?token=` identically.

## Documents
- `SPEC.md` "Authentication", bullets 2 and 3 (validation order, 401 on missing user or version mismatch, current DB values for `admin`/`must_change_password`, allowed routes under the gate, error string `password change required`).
- `SPEC.md` "REST API" status table (401, 403).
- `docs/data-model.md` `users` (`auth_version`, `must_change_password`, "Ordinary request authorization reads current user state without taking this mutation lock").
- `ARCHITECTURE.md` "User authentication and revocation".
- ADR 0025.

## Acceptance criteria
- [ ] `pub async fn authenticate_access_token(state: &AppState, token: &str) -> Result<User>`: decode claims (401 on invalid/expired), `UserRepository::find_by_id(claims.sub)` (401 when missing), compare `claims.auth_version == user.auth_version` (401 on mismatch); no row lock; errors are `Error::Unauthorized` rendering `{ "status": 401, "error": "authentication required" }`.
- [ ] `UngatedUser(pub User)` implements `FromRequestParts<AppState>`: reads `Authorization: Bearer <token>` (missing or malformed header → 401) and calls `authenticate_access_token`.
- [ ] `CurrentUser(pub User)` does what `UngatedUser` does and then answers 403 with error `password change required` when `user.must_change_password` is true.
- [ ] `AdminUser(pub User)` extends `CurrentUser` and answers 403 with error `admin required` when `user.admin` is false in the database row, ignoring the token's `admin` claim.
- [ ] Demotion takes effect on the next request (403 on an admin route) and deletion on the next request (401), with no token revocation involved.
- [ ] Add `Error::Unauthorized` (401) to the prelude `Error` enum if it does not exist; `Forbidden` carries an optional message so both `password change required` and `admin required` render.
- [ ] `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Implementation notes
- Files: `orchestrator/src/routes/extractors.rs` (new), `orchestrator/src/routes/mod.rs` (re-export), `orchestrator/src/prelude/error.rs` (variants and `IntoResponse` mapping).
- Use `axum::extract::FromRequestParts` with `S = AppState` (`AppState: Clone`), read the header through `axum_extra::TypedHeader<Authorization<Bearer>>` or manual parsing of `HeaderMap`; the scheme comparison is case-insensitive (`Bearer`).
- Route usage convention for later tasks: `GET /users/me` and `POST /users/{id}/password` take `UngatedUser`; every other authenticated handler takes `CurrentUser` or `AdminUser`; `POST /auth/refresh` and `POST /auth/logout` take no extractor (cookie only).
- The realtime epic reuses `authenticate_access_token` for `?token=` and re-runs `find_by_id` + version + gate checks at heartbeat ticks; keep the function free of header parsing.
- Log failed authentications at `debug` with `user_id = %claims.sub` only; never the token.

## Edge cases
- `Authorization` header with a non-Bearer scheme or empty token → 401.
- Token valid but `sub` not a UUID → 401 (already enforced by `Claims::decode`).
- Gate and admin checks both failing: the gate check runs first (a gated admin gets `password change required`, not `admin required`).
- Database error while loading the user → 500 generic; never authorises on failure.

## Testing
- Integration tests through `TestApp` on a trivial authenticated route (use `GET /users/me` once it exists; until then a `#[cfg(feature = "integration-tests")]` probe route under `/test/whoami` is acceptable and removed later): no header → 401; garbage token → 401; expired token (mint with `exp` in the past using the test `Config`) → 401; token for a deleted user → 401; token whose `auth_version` is one behind the row → 401; user with `must_change_password` on a gated route → 403 `password change required` and on `UngatedUser` route → 200; non-admin on an `AdminUser` route → 403 `admin required`; token minted with `admin: true` for a user whose row says `admin = false` → 403 (DB wins).
- `TestApp` helper: `TestApp::token_for(&User) -> String` and `TestApp::expired_token_for(&User)` minting claims directly from the harness `Config`.
- Command: `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Database schema, models, repositories and test harness": `UserRepository::find_by_id(Uuid) -> Result<Option<User>>` and a `TestApp` that can insert a user row directly (through the repository) for these tests.
- "Repository scaffolding, tooling and CI": prelude `Error` enum and `IntoResponse` skeleton.