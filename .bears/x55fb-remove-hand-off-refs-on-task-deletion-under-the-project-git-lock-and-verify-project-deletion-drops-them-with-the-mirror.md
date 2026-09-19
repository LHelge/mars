---
id: x55fb
title: Remove hand-off refs on task deletion under the project git lock and verify project deletion drops them with the mirror
status: done
priority: P1
created: "2026-09-16T20:44:05.955094700Z"
updated: "2026-09-19T23:34:22.104659294Z"
tags:
  - orchestrator
  - tracker
  - git
depends_on:
  - tgc55
parent: xjaah
attempts: 1
---

## Summary
Make `DELETE /projects/{pid}/tasks/{id}` acquire the project git lock before the tracker transaction, collect the task's hand-off ids, run the tracker epic's deletion (which cascades `task_handoffs`), commit, and then remove every `refs/handoffs/<id>` for the task while still holding the lock. Project deletion already removes the whole mirror under the same lock; this task verifies that and documents that no per-ref work is needed there. Orphan refs left by an interruption are the Background jobs epic's cleanup.

## Documents
- `ARCHITECTURE.md` "Git model" -> "Serialization" ("Task deletion also acquires this lock before removing hand-off refs"; git lock before the database project lock; a transaction holding the project row must never wait for the git lock), "Task tracker" -> "Code hand-offs" (committed hand-off refs survive session deletion and remain until their task or project is deleted; the orphan cleanup job removes refs without matching rows under the git lock).
- `docs/data-model.md` `task_handoffs` ("Task/project deletion removes their hand-off refs under the project git lock; cleanup retries remove orphan refs left by interrupted deletion or failed publication"), `tasks` (`current_handoff_id` FK `ON DELETE SET NULL`; `task_handoffs.task_id` `ON DELETE CASCADE`).
- `SPEC.md` "Tasks" (`DELETE /projects/{pid}/tasks/{id}` -> 204; deletion recomputes surviving dependants and emits `deleted` retaining the id).
- `ARCHITECTURE.md` "Background jobs" (orphan cleanup: remove `refs/handoffs/*` with no matching row under each project's git lock).

## Acceptance criteria
- [ ] The tracker epic's task deletion function (or the `DELETE` handler, whichever owns the transaction) is wrapped by `tracker::handoffs::delete_task_with_refs(state, project_id, task_ref, actor)`: acquire the project git lock -> `begin_mutation` -> `find_task_for_update` -> `list_handoffs(project_id, task_id)` to capture ids -> the tracker epic's deletion (events, dependant recompute) -> commit -> for each id `git::refs::remove_handoff(mirror, id)` (idempotent) -> release the lock. The handler calls this wrapper instead of the bare deletion.
- [ ] Ref removal failures after commit are logged at `warn!` with `project_id`, `task_id`, `handoff_id` and do not change the 204 response.
- [ ] Deleting a task with three hand-offs leaves `git::refs::list_handoffs(mirror)` without those ids and the objects still reachable via `refs/sessions/<sid>` (only the refs go; no gc).
- [ ] Deleting a task whose hand-off is referenced by `sessions.handoff_id` leaves that session with `handoff_id = null` (FK) and its `base_ref` commit string intact.
- [ ] Project deletion: an integration test publishes a hand-off, deletes the project through `DELETE /projects/{id}`, and asserts the mirror directory is gone; no code change expected beyond the test.
- [ ] Session deletion (`DELETE /sessions/{id}`) leaves the hand-off row (`source_session_id` null) and the ref intact: asserted by a test.

## Implementation notes
- Files: `orchestrator/src/tracker/handoffs.rs` (wrapper), `orchestrator/src/routes/tasks.rs` (handler call), tests in `orchestrator/tests/handoffs_deletion.rs`.
- A project that is not `ready` (mirror missing): skip ref removal with a `debug!` log; the tracker deletion still runs.
- The lock is taken even for tasks without hand-offs (cheap, keeps one code path); the `list_handoffs` result decides whether any ref commands run.

## Edge cases
- Deletion racing a publication for the same task: the git lock serialises them; the publication finds the task gone (`NotFound`) and discards its ref, or the deletion removes the just-published ref.
- Deletion racing a task merge of that hand-off: the merge's verifier runs under the git lock, so it either merges before the deletion or gets 404.
- Task referenced by number: resolve through `TaskRef` before locking.

## Testing
- `orchestrator/tests/handoffs_deletion.rs` via `TestApp`: publish two revisions and one forward for a task (three refs), launch-for-task style session row with `handoff_id`, `DELETE .../tasks/{id}` -> 204, refs gone, session `handoff_id` null, `deleted` event present; project deletion case; session deletion case; deletion of a task without hand-offs still 204.
- `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Task tracker: states, tasks, leases, dependencies and events": the task deletion transaction and `DELETE` handler.
- "Projects, agent profiles and shared directories": `DELETE /projects/{id}` under the git lock removing the mirror.
- "Session lifecycle: launcher, owner, recovery and sessions API": `DELETE /sessions/{id}`.
- "Background jobs": orphan cleanup of `refs/handoffs/*` without rows (not touched here).