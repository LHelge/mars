---
id: xfu5s
title: "Add end-to-end project lifecycle integration tests: create to ready, error and retry, fetch, branches, profiles, shared dirs and delete cleaning disk"
status: open
priority: P1
created: "2026-09-16T20:33:51.965205095Z"
updated: "2026-09-16T20:33:51.965205095Z"
tags:
  - orchestrator
  - projects
  - tests
  - git
depends_on:
  - xdz2q
  - sdvbh
  - cpmj2
  - "98che"
parent: pkaee
---

## Summary
Add one integration test file that drives the whole project lifecycle through the public REST API against real local bare repositories, proving the epic's three acceptance criteria end to end rather than per endpoint: a project reaches `ready` with a discovered `default_branch`, an unreachable remote reaches `error` and `retry-clone` recovers once the remote exists, and deleting a project removes every row and every on-disk artefact. It also adds a shared `tests/common/git.rs` helper for creating bare fixtures so the Session lifecycle and Git epics can reuse it, and a sweep asserting every endpoint in the three tables answers 401 unauthenticated.

## Documents
- Epic `pkaee` "Acceptance criteria" (all three).
- `SPEC.md` "Projects", "Shared directories", "Agent profiles" (every row; statuses 401/400/404/409).
- `ARCHITECTURE.md` "Storage" (what must be gone after deletion), "Git model" → "Project clone".
- `CLAUDE.md` "Testing expectations" (`TestApp::spawn()`, one `#[tokio::test]` per scenario, git never mocked, real bare repositories in `tempfile` directories).

## Acceptance criteria
- [ ] `orchestrator/tests/common/git.rs` provides `BareFixture::new() -> BareFixture` (a `tempfile::TempDir` with `origin.git` initialised bare with `git init --bare --initial-branch=main`, one commit on `main` made through a throwaway work clone, `HEAD` → `main`), `BareFixture::url() -> String` (`file://` absolute), `add_branch(name)`, `add_commit(branch, filename)`, `remove()` (deletes the directory to simulate an unreachable remote) and `recreate()`; all through the `git` binary with argv arrays, `user.name`/`user.email` set per command.
- [ ] `orchestrator/tests/project_lifecycle.rs` scenarios, each its own `#[tokio::test]`:
  - `creates_project_and_reaches_ready_with_discovered_branch`: `POST /projects` with only `name` and `remote_url` → 201 `cloning`, `default_branch: null`; `wait_for_clone` → `ready`, `default_branch: "main"`, `status_message: null`, `last_fetched_at` set; `GET /projects/{id}/branches` lists `main` (head) and `origin/main` (upstream) with equal commits; the layout `repo.git`, `claude/`, `shared/` exists under `DATA_DIR/projects/<id>/`.
  - `unreachable_remote_reaches_error_and_retry_recovers`: fixture removed before `POST` → `error` with a non-empty `status_message` containing no `@`; `retry-clone` → 200 `cloning` → `error` again; `recreate()` the fixture; `retry-clone` → `ready` with discovered `main`; `retry-clone` on `ready` → 409.
  - `supplied_default_branch_is_validated`: fixture with `main` and `develop`; `default_branch: "develop"` → `ready` with `develop` and bare `HEAD` → `refs/heads/develop`; `default_branch: "nope"` → `error` with the exact message from the clone-job task.
  - `fetch_refreshes_upstream_without_moving_heads`: after `ready`, `add_commit("main", ...)` in the fixture, `POST fetch` → `origin/main` advanced, `main` unchanged; `last_fetched_at` advanced.
  - `delete_removes_rows_and_disk`: on a `ready` project add a profile, a shared dir with a populated on-disk directory, a project secret, a `GIT_CREDENTIAL`, a task with a comment and a dependency (through the repositories if the tracker routes do not exist yet), a `parked` session row with `DATA_DIR/sessions/<sid>/work/` populated; `DELETE` → 204; every table listed in the delete task has zero rows for the id; `DATA_DIR/projects/<id>` and `DATA_DIR/sessions/<sid>` are gone; `GET /projects/{id}` → 404.
  - `delete_refused_while_session_running`: `running` session row → 409 and the directory remains; set it to `done` → 204.
  - `unauthenticated_requests_are_rejected`: a table-driven sweep over every method/path in the Projects, Shared directories and Agent profiles tables (17 routes) asserting 401 without a token.
- [ ] The suite runs in CI with `SQLX_OFFLINE=true` (no new queries; if any were needed, `.sqlx/` is refreshed).
- [ ] Total runtime of the file stays under 60 s on CI (git operations on tiny repositories; polling at 50 ms).

## Implementation notes
- Files: `orchestrator/tests/common/git.rs` (new; `pub mod git;` in `tests/common/mod.rs`), `orchestrator/tests/project_lifecycle.rs` (new).
- Use `projects::clone_job::wait_for_clone` (feature-gated) via the `TestApp`'s `AppState` handle, or an HTTP polling helper `TestApp::wait_for_project_status(id, status, timeout)` if the app state is not reachable from tests; add the latter to `tests/common/mod.rs` if needed.
- Read `DATA_DIR` from `TestApp` (expose `data_dir()` if the harness does not yet) to assert on-disk state.
- Bare `HEAD` assertion: `git -C repo.git symbolic-ref HEAD` through `std::process::Command` in the test.
- Do not duplicate per-endpoint validation tests already written in the route tasks; this file is about sequences that cross tasks.

## Edge cases
- `file://` URLs are accepted only with the `integration-tests` feature (project model task); the fixture must produce absolute paths.
- Timing: the clone job is asynchronous; never assert `ready` right after the 201.
- Parallel tests each own a `TestApp` (own database and own `DATA_DIR`), so fixtures must not share paths.

## Testing
- This task is the test; `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` must pass, and `cargo test --features integration-tests --test project_lifecycle` must pass alone.

## Documentation
- none: verifies the documented contract.

## Assumes from other epics
- "Database schema, models, repositories and test harness": `TestApp::spawn()` with per-test `DATA_DIR`, authenticated-user helper, repository access for seeding tracker rows.
- "Authentication, users, invites and email": JWT issuance for test users.
- "Git operations": clone/fetch primitives used by the clone job and fetch endpoint.
- "Task tracker": if its REST routes exist, tasks may be created through them; otherwise through the repositories.