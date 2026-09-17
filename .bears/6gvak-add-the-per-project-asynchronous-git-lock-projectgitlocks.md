---
id: "6gvak"
title: Add the per-project asynchronous git lock (ProjectGitLocks)
status: done
priority: P0
created: "2026-09-16T20:27:28.658116156Z"
updated: "2026-09-17T23:27:48.533555076Z"
tags:
  - orchestrator
  - git
depends_on:
  - t36d2
parent: z4u4e
assignee: claude-opus-subagent
attempts: 1
---

## Summary
Provide the one per-project asynchronous lock that serialises every orchestrator mutation of a project repository: initialisation, upstream fetch, session fetch-back, hand-off publication, merge, rebase, push and project deletion. It lives in `AppState` so REST handlers, MCP tools, the session launcher, the cron jobs and deletion paths share it. The locking order (git lock before any database lock, multiple projects in UUID order) is part of the contract.

## Documents
- `ARCHITECTURE.md` "Git model" -> "Serialization"
- `ARCHITECTURE.md` "Task tracker" -> "One mutation at a time per project" (git locks acquired in UUID order before database locks; a transaction holding the project row must never wait for the git lock)
- ADR 0017 (mutations serialised per project), ADR 0021

## Acceptance criteria
- [ ] `git::lock::ProjectGitLocks` exposes `async fn lock(&self, project_id: Uuid) -> ProjectGitGuard` (an owned guard, `Send`, droppable across `.await`), `async fn lock_many(&self, ids: &[Uuid]) -> Vec<ProjectGitGuard>` acquiring in ascending UUID order, and `fn forget(&self, project_id: Uuid)` for project deletion.
- [ ] Two concurrent `lock(p)` calls run strictly one after the other; `lock(p)` and `lock(q)` run concurrently (tested).
- [ ] The guard type is documented as "hold through completion of a composite operation; helpers take `&ProjectGitGuard` and never reacquire".
- [ ] `AppState` holds `Arc<ProjectGitLocks>` and `TestApp` exposes it.
- [ ] `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Implementation notes
- File: `orchestrator/src/git/lock.rs`. Shape: `std::sync::Mutex<HashMap<Uuid, Arc<tokio::sync::Mutex<()>>>>`; `lock` clones the `Arc` under the std mutex, releases it, then `lock_owned().await` so the map lock is never held across an await. `ProjectGitGuard { project_id: Uuid, _guard: OwnedMutexGuard<()> }`.
- Helpers in later tasks take `&ProjectGitGuard` as a proof-of-lock parameter rather than the id alone, so that the compiler makes "acquire once, hold through completion" visible at call sites.
- `forget` removes the map entry after project deletion; a late caller simply creates a fresh entry (the repository directory is gone by then and the operation fails on I/O).
- Add `git_locks: Arc<ProjectGitLocks>` to `AppState` (`orchestrator/src/prelude/state.rs` or wherever the DB epic placed `AppState`).
- Instrument `lock()` with a `tracing::debug_span!("git_lock", project_id = %project_id)` and log at `debug` when waiting longer than 5 s.

## Edge cases
- Never call `lock()` while a database transaction holding the project row (`SELECT ... FOR UPDATE`) is open; document this on the type with a `# Ordering` doc section.
- Cancellation: if a task holding the guard is cancelled, the guard drops and the lock releases; callers must leave the repository consistent (temp clones cleaned by the orphan-cleanup job).

## Testing
- Unit tests with `tokio::test(flavor = "multi_thread")`: serialisation on one id (second lock resolves only after the first guard drops, checked with a shared counter and `tokio::time::timeout`), independence between two ids, `lock_many` ordering (assert guards are returned sorted by UUID regardless of input order).
- Command: `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Database schema, models, repositories and test harness": `AppState` and `TestApp::spawn()` exist so the field can be added.