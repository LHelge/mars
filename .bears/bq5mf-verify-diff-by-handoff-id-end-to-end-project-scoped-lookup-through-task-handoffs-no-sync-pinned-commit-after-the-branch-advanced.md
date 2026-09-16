---
id: bq5mf
title: "Verify diff by handoff_id end to end: project-scoped lookup through task_handoffs, no sync, pinned commit after the branch advanced"
status: open
priority: P2
created: "2026-09-16T20:43:43.810994158Z"
updated: "2026-09-16T20:43:43.810994158Z"
tags:
  - orchestrator
  - git
  - tracker
  - tests
depends_on:
  - tgc55
parent: xjaah
---

## Summary
Close the loop on `GET /projects/{pid}/git/diff?handoff_id=&base=`: make sure the git epic's `DiffSelector::Handoff` path checks that the hand-off belongs to the URL project through `task_handoffs` (404 otherwise), resolves `refs/handoffs/<id>` without any fetch-back, and test it against hand-offs published through the API rather than seeded rows. This is what the task detail's revision view uses.

## Documents
- `SPEC.md` "Git" (`GET .../diff` accepts exactly one of `head` or `handoff_id`, 400 otherwise; a hand-off must belong to the URL project and selects its immutable commit without fetch-back; `Diff` shape; `base` defaults to the project's default branch).
- `ARCHITECTURE.md` "Git model" -> "Diff" (supplying `handoff_id` selects that project's retained hand-off commit and never syncs a moving branch; the internal fetch-back for session heads emits no `git` event), "Serialization" (ref resolutions for read-only diffs are captured under the lock).
- `SPEC.md` "Frontend" -> "Hand-off controls" (the diff endpoint accepts `handoff_id` to view the retained commit without syncing a live branch).

## Acceptance criteria
- [ ] `GitService::diff` with `DiffSelector::Handoff(id)` first calls `TaskRepository::find_handoff(project_id, id)` (project scope derived through the task) and returns `NotFound` when absent; only then resolves `GitRef::Handoff(id)` under the lock. If the git epic implemented this already, keep one implementation and add the tests.
- [ ] `Diff.head` for a hand-off diff is the hand-off id (the `GitRef::Handoff` API name) and `Diff.merge_base` is the merge base of `base` and the pinned commit.
- [ ] No session sync happens: the source session's work clone can be deleted and the diff still succeeds; the `events` table of the source session gains no `git` event.
- [ ] After the source branch advances and is synced, the diff by `handoff_id` still shows only the pinned revision; `?head=<session>` shows the new tip.
- [ ] A hand-off id from another project -> 404; a random UUID -> 404; `head` and `handoff_id` together -> 400 (already in the git epic; re-asserted here).

## Implementation notes
- Files: `orchestrator/src/git/service.rs` (scope check, if missing), `orchestrator/tests/handoffs_diff.rs` (new).
- The lookup is a plain read outside any transaction; the lock is taken afterwards only for resolution, as the git epic specifies.

## Edge cases
- Hand-off whose ref was removed by an interrupted task deletion while the row still exists (only possible mid-deletion): `resolve` fails -> 400 `unknown ref`; acceptable and documented in the test as the invariant boundary.
- `base` naming a session ref: 400 per the ref-kind rules (`is_base()` is heads and upstream only).

## Testing
- `orchestrator/tests/handoffs_diff.rs` via `TestApp`: publish revision A through `PUT .../tasks/{id}`, advance the session to B and `POST /sessions/{id}/sync`, then assert `diff?handoff_id=A.id` lists only A's files and `diff?head=<sid>` lists both; remove the work dir and repeat the hand-off diff; 404 and 400 cases; event count on the session unchanged by the hand-off diff.
- `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Git operations: mirror, clones, integration and REST API": `GitService::diff`, `DiffSelector`, the diff route.
- "Session lifecycle: launcher, owner, recovery and sessions API": `POST /sessions/{id}/sync` (or call `GitService::sync_session` directly in the test if the route is not yet available).