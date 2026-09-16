---
id: w5re4
title: Add test-only POST /api/test/users behind the integration-tests feature and TestApp login helpers
status: open
priority: P1
created: "2026-09-16T20:31:31.374433180Z"
updated: "2026-09-16T20:31:31.374433180Z"
tags:
  - orchestrator
  - auth
  - tests
depends_on:
  - "99sgv"
parent: qacxf
---

## Summary
Provide the route Playwright and backend tests use to create users without the invite flow: `POST /api/test/users` creates a user with `must_change_password = false`, issues a token pair and sets the refresh cookie, and is compiled only with the `integration-tests` cargo feature. Alongside it, add `TestApp` conveniences (`create_user`, `create_admin`, `authenticated_client`) so every later epic's tests get a logged-in user in one call.

## Documents
- `SPEC.md` "Test-only routes" (`POST /test/users` `{username, email, password, admin?}` → `{user, access_token}`, 201, sets the refresh cookie, `must_change_password` false; never in a release build).
- `CLAUDE.md` "Testing expectations" (Frontend E2E creates fresh users per test through this endpoint; `TestApp::spawn()`).
- `ARCHITECTURE.md` "Orchestrator internals" (mocks and test routes behind `integration-tests`).

## Acceptance criteria
- [ ] `orchestrator/src/routes/test.rs` exists only under `#[cfg(feature = "integration-tests")]` and is nested at `/api/test` only in that configuration; `cargo build` without the feature contains no `/test` route (test: `cargo build` and a `#[cfg(not(feature = "integration-tests"))]` compile-time assertion or a grep in CI is acceptable; at minimum the module is cfg-gated).
- [ ] `POST /api/test/users` `{ username, email, password, admin?: bool }`: validates through `UserError` (400), normalises email, inserts the user with `must_change_password = false`, `admin = admin.unwrap_or(false)`, `notify_email = true`; duplicate username or email → 409; success → 201 `{ user, access_token }` with the `refresh_token` cookie, using the shared `issue_pair`.
- [ ] `TestApp` helpers: `create_user(username, email, password) -> AuthenticatedUser { user, access_token, refresh_cookie }`, `create_admin(..)`, `create_gated_user(..)` (inserts with `must_change_password = true` through the repository for gate tests), and `TestApp::as_user(&AuthenticatedUser) -> TestServer`-style request builder that sets the bearer header.
- [ ] `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes, and `cargo clippy -- -D warnings` without the feature also passes.

## Implementation notes
- Files: `orchestrator/src/routes/test.rs` (new), `orchestrator/src/routes/mod.rs` (`#[cfg(feature = "integration-tests")] pub mod test;` and conditional nesting), `orchestrator/tests/common/mod.rs` (helpers).
- Reuse `hash_password` (in `spawn_blocking`), `UserRepository::insert`, `issue_pair`, `refresh_cookie`, `TokenPairResponse`.
- No authentication on the route; it exists only in test builds. Do not add any other test-only routes here unless `SPEC.md` "Test-only routes" lists them.

## Edge cases
- Creating an admin through this route does not consult the last-administrator lock (it only adds administrators).
- Username/email whitespace: trim; email lower-cased.
- The seeded `admin` row is removed by `TestApp::spawn()`; creating a user named `admin` through this route must therefore succeed in tests.

## Testing
- `tests/test_routes.rs`: happy path 201 with cookie and `must_change_password: false`; the returned access token works on `GET /users/me`; the cookie refreshes; duplicate username 409; duplicate email 409; invalid password 400; `admin: true` yields a user who can call `GET /users`.
- Command: `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests && cargo clippy -- -D warnings`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Database schema, models, repositories and test harness": `TestApp::spawn()` and the `integration-tests` feature flag in `Cargo.toml`.
- "End-to-end tests with Playwright" consumes this route; no change here.