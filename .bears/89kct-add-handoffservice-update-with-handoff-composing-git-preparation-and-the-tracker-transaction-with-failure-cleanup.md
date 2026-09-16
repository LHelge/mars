---
id: "89kct"
title: "Add HandoffService::update_with_handoff composing git preparation and the tracker transaction with failure cleanup"
status: open
priority: P1
created: "2026-09-16T20:42:24.782704435Z"
updated: "2026-09-16T20:42:24.782704435Z"
tags:
  - orchestrator
  - tracker
  - git
depends_on:
  - sgwwv
parent: xjaah
---

## Summary
Compose the two halves into the single entry point the REST `PUT` handler and the MCP `update` tool call when `handoff` is present: resolve the task and states, validate the input against the caller, acquire the project git lock, prepare (sync and pin), open the tracker transaction, publish, commit, release the lock, and on any failure after pinning discard the ref so the failure leaves the task, lease and refs unchanged. This is the only code path that publishes or forwards a hand-off.

## Documents
- `ARCHITECTURE.md` "Task tracker" -> "Code hand-offs" (sync failure or stale task returns an error with the task and lease unchanged; failed publication is safe to retry after reading the task's current state; a crash before the database commit may leave an unreferenced ref that the orphan cleanup job removes), "One mutation at a time per project" (git lock before the database project lock; keep transactions short; never acquire a git lock from inside the tracker transaction).
- `ARCHITECTURE.md` "Git model" -> "Serialization" (hand-off publication is one of the lock-covered operations; composite operations acquire it once and hold it through completion; the final current-hand-off check of a task merge is serialised against hand-off changes under the same lock).
- `SPEC.md` "Tasks" (concurrent requests wait, then succeed against the resulting state or return the existing validation/conflict error; a partial change is never published), "Code hand-offs and review", "MCP tool contracts" -> `update`.
- ADRs 0018, 0021.

## Acceptance criteria
- [ ] `tracker::handoffs::HandoffService` (constructed from `AppState` parts: pool, `DataPaths`, `Arc<ProjectGitLocks>`, `Arc<GitService>`) with `update_with_handoff(&self, project_id, task: TaskRef, update: TaskUpdateRequest, handoff: HandoffInput, caller: HandoffCaller) -> Result<Task>`.
- [ ] Order: (1) `find_task` -> `NotFound`; (2) resolve `update.state` by name -> `BadRequest("unknown state <name>; valid states: a, b, c")` reusing the tracker epic's message; (3) `HandoffInput::require_state_change` and `validate(caller)`; (4) `projects.status` must be `ready` -> `Conflict("project is not ready")`; (5) acquire the git lock; (6) `prepare`; (7) `begin_mutation`; (8) `publish_in_transaction`; (9) commit; (10) release the lock; (11) return the task. Steps 1-4 run without any lock and have no side effects.
- [ ] Any error from steps 7-9 (including a serialisation error or pool timeout) calls `discard_prepared` before returning; the original error is returned, not the cleanup outcome.
- [ ] The git lock is held from step 5 through step 10 (the guard lives across the transaction); the transaction never waits for the git lock.
- [ ] Rejected requests write no `task_events`, no `task_sessions` rows and no hand-off rows (ADR 0030), asserted by tests.
- [ ] `AppState` exposes the service (`state.handoffs`) or a constructor `HandoffService::from_state(&AppState)` following the `GitService` pattern.
- [ ] Log at `info` with `project_id`, `task_id`, `handoff_id`, `kind = "revision" | "forward"` on success; never log the comment.

## Implementation notes
- Files: `orchestrator/src/tracker/handoffs.rs` (service struct), `orchestrator/src/prelude/state.rs` (field or constructor).
- Concurrency contract to write in the module doc: two callers on the same project serialise on the git lock, then on the project row; the second re-reads under both locks and either succeeds against the new state or gets the `Conflict` from preparation/recheck.
- `TaskRef` parsing (UUID or number) is the tracker/repository epic's; accept it as a parameter so both REST and MCP can pass what they parsed.

## Edge cases
- Project `cloning` or `error`: 409 before any git work.
- Task deleted between step 1 and step 8: `find_task_for_update` returns none -> `NotFound`; the ref is discarded.
- Database commit succeeds but the process dies before returning: the ref and rows are consistent; the client retries and gets a stale-state conflict, which is correct.
- Commit fails after `publish_in_transaction` returned `Ok` (connection lost): treat as an error path, discard the ref, return `Internal`; the orphan job covers the case where discard also fails.

## Testing
- Integration tests in `orchestrator/tests/handoffs_service.rs` with real bare repositories through `TestApp`: end-to-end revision by a session caller (commit in the work clone, call the service, assert ref, rows, lease cleared, events); tip mismatch -> `Conflict`, task unchanged, no ref, no events; stale forward -> `Conflict`, no ref; recheck failure simulated by mutating the task state in a separate transaction between preparation and publication (inject with a test-only hook or by holding the project row in another connection until preparation completes) -> `Conflict` and `git::refs::list_handoffs` empty; project not `ready` -> `Conflict` without git activity; two concurrent revisions for one task from two sessions (only one holds the lease) -> exactly one succeeds.
- `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Task tracker: states, tasks, leases, dependencies and events": `TaskUpdateRequest`, state lookup by name with the documented unknown-state message, `TaskRef`.
- "Git operations: mirror, clones, integration and REST API": `ProjectGitLocks`, `GitService`.
- "MCP server and agent tools": the `update` tool calls this service with `HandoffCaller::Session` and maps `Conflict` to `conflict`, `BadRequest`/`TaskError` to `invalid_argument`.