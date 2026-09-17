---
id: bptgh
title: "Implement TestApp::spawn() with mocks, migrated database and a health smoke test"
status: done
priority: P0
created: "2026-09-16T20:29:38.437708995Z"
updated: "2026-09-17T07:08:11.301056154Z"
tags:
  - orchestrator
  - core
  - tests
depends_on:
  - yy5rt
  - suzac
  - xbjn4
parent: p5tsd
assignee: claude-opus-subagent
attempts: 1
---

## Summary
Build the integration-test entry point every backend epic uses: `TestApp::spawn()` in `tests/common/mod.rs` starts a testcontainers Postgres, applies migrations, removes the seeded administrator, constructs `AppState` with the mock engine, mock email client, mock git credential provider and the fixed test master key, and serves the router through `axum-test`. A smoke test calls `GET /api/health` through it.

## Documents
- `CLAUDE.md` "Testing expectations" (`TestApp::spawn()`: testcontainers Postgres, migrations applied, seeded admin removed, mock engine, email and git credentials, fixed test master key; assert with `response.assert_status()` and `response.json::<T>()`).
- `ARCHITECTURE.md` "Orchestrator internals" (`AppState` contents; `tests/` layout; crate table: `axum-test`, `testcontainers-modules`, `tempfile`).
- `SPEC.md` "Health" (`GET /api/health` → `{ orchestrator: true, database: bool, engine: bool }`, 200 or 503), "Test-only routes" (compiled only with `integration-tests`).
- `README.md` "Configuration" (the variables `Config` needs).

## Acceptance criteria
- [ ] `tests/common/mod.rs` exposes `pub struct TestApp { pub server: axum_test::TestServer, pub pool: PgPool, pub state: AppState, pub engine: Arc<MockContainerEngine>, pub email: Arc<MockEmailClient>, pub git: Arc<MockGitCredentialProvider>, pub data_dir: tempfile::TempDir, _db: <container guard> }` and `pub async fn spawn() -> TestApp`.
- [ ] `spawn()` uses `common::db::test_pool()` (migrations applied), then executes `DELETE FROM users WHERE id = '00000000-0000-0000-0000-000000000001'` so no test depends on the seeded admin.
- [ ] `spawn()` builds `Config` programmatically (not from the process environment) with obviously fake values: `PUBLIC_URL=http://localhost`, `JWT_SECRET=test-jwt-secret-not-for-production`, `DATABASE_URL` from the container, `DATA_DIR` = `DATA_DIR_HOST` = the `TempDir` path, `SECRETS_MASTER_KEYS` from `SecretsKeyring::test_key()`, `GIT_BOT_NAME=Mars Test Bot`, `GIT_BOT_EMAIL=bot@example.test`, `SESSION_IMAGE_DEFAULT=mars-session-stub:test`, no `RESEND_API_KEY`, ports 0.
- [ ] The router comes from the library (`mars_orchestrator::app(state)` or whatever the scaffolding named it), including the `integration-tests`-only routes, so tests exercise the real middleware stack.
- [ ] `TestApp` offers `pub fn mock_engine(&self)`, `pub fn mock_email(&self)`, `pub fn mock_git(&self)` returning the concrete mocks (downcast through `as_any()` is available for callers that only hold the `Arc<dyn Trait>`).
- [ ] `tests/health.rs`: `GET /api/health` returns 200 with `database: true` and `engine: true` (mock `ping` succeeds) and `orchestrator: true`.
- [ ] `tests/common/mod.rs` compiles in every test binary that declares `mod common;` even when only a subset is used (`#![allow(dead_code)]`).
- [ ] `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes; without the feature the `tests/` binaries that need mocks are gated with `#![cfg(feature = "integration-tests")]`.

## Implementation notes
- Files: `orchestrator/tests/common/mod.rs` (reuse `common/db.rs` from the harness task), `orchestrator/tests/health.rs`.
- Add `axum-test` and `tempfile` as dev-dependencies with `cargo add --dev` if the scaffolding did not.
- Do not start the real listeners or cron jobs; `TestServer::new(router)` drives the router in-process. Session owners, listeners and background jobs are started explicitly by the tests of the epics that own them.
- The event fan-out broadcast senders and `SessionRegistry` in `AppState` are constructed with their `Default`/`new()`; document in a comment which fields future epics may need to swap.
- Keep `spawn()` fast: one container per test is expected; the pool size default of the harness is enough.

## Edge cases
- If `Config` has required fields this task does not know how to fill, fail loudly with `expect("test config")` so the gap is visible, and note it in the task on completion.
- The `TempDir` must outlive the app; keep it in the struct.
- `DELETE` of the seeded admin must run after migrations and before the router is built so the first request never sees it.

## Testing
- `tests/health.rs` smoke test as above; also assert `TestApp::spawn()` twice in one test binary works in parallel (two independent containers).
- `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` must pass.

## Documentation
- None: implements `CLAUDE.md` "Testing expectations" as written.

## Assumes from other epics
- "Repository scaffolding, tooling and CI": `Config` constructible in code (a `Config { .. }` struct or a builder besides `from_env()`), `AppState` with the fields listed in `ARCHITECTURE.md`, a library function returning the `Router` with `/api/health` mounted, and the `integration-tests` feature.