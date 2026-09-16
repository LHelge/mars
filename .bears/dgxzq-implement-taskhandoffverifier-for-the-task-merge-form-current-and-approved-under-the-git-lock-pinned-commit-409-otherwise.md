---
id: dgxzq
title: "Implement TaskHandoffVerifier for the task merge form: current and approved under the git lock, pinned commit, 409 otherwise"
status: open
priority: P1
created: "2026-09-16T20:43:25.886302639Z"
updated: "2026-09-16T20:43:25.886302639Z"
tags:
  - orchestrator
  - tracker
  - git
  - tests
depends_on:
  - tgc55
parent: xjaah
---

## Summary
Replace the git epic's `NoHandoffs` placeholder with the real `HandoffVerifier` backed by `tasks.current_handoff_id` and `task_handoffs`, so `POST /projects/{pid}/git/merge` in its task form and (through the same `GitService::merge_handoff`) the MCP `merge` tool verify under the project git lock that `handoff_id` is the task's current hand-off and is `approved`, then merge exactly the pinned commit without syncing the source branch. Stale or unapproved hand-offs return 409 / MCP `conflict` with the target untouched.

## Documents
- `SPEC.md` "Git" (`POST .../merge` 409 for stale or unapproved task hand-off; `MergeInput = { target, message? } & ({ source } | { task_id, handoff_id })`; the task form requires the task's current hand-off to match `handoff_id` and be `approved`, and merges its exact retained commit without syncing a newer source-session tip; internal hand-off refs are not returned as branches), "MCP tool contracts" -> `merge` (task form rejects stale or unapproved hand-offs and uses the pinned commit without syncing; `task_id` also accepts a per-project number in MCP).
- `ARCHITECTURE.md` "Git model" (task merge paragraph: verify current and approved under the git lock, merge the pinned commit, no fetch of a newer tip, stale/unapproved -> conflict without updating the target; the final current-hand-off check and target-ref write are serialised against hand-off changes under the same lock; generic branch merges do not record approval), "Task tracker" -> "Review approval" (the UI's task-merge action and the MCP merge tool's task form require the current hand-off to be approved and merge only its pinned commit).
- ADR 0018.

## Acceptance criteria
- [ ] `tracker::handoffs::TaskHandoffVerifier { pool }` implements `git::service::HandoffVerifier::approved_commit(project_id, task_id: TaskRef, handoff_id) -> Result<ApprovedHandoff { commit, source_branch }>`: task not in project -> `NotFound`; `task.current_handoff_id != Some(handoff_id)` -> `Conflict("handoff_id is not the task's current hand-off")`; `review_status != approved` -> `Conflict("hand-off is not approved")`; otherwise the row's `commit` and `source_branch`. Plain reads, no row locks (the caller holds the git lock, which every publication holds through its commit, so the read is stable).
- [ ] `AppState` wires `TaskHandoffVerifier` into `GitService` at startup (replacing `NoHandoffs`, which is removed or kept only for the git epic's unit tests).
- [ ] `GitService::merge_handoff` (git epic) calls the verifier after acquiring the lock and before `merge_commit`, passes `source_label = "handoff:<id>"` (used in the default message `Merge handoff <id> (<source_branch>) into <target>`) and never calls `sync_session_silent` for the source; verify this and adjust in the same commit if the git epic's implementation differs.
- [ ] The merge's `git` outcome event (if the source session still exists) carries `detail.source = "refs/handoffs/<id>"`; the tracker records nothing on merge (no task event, no state change: merging is not a hand-off).
- [ ] REST: `task_id` in `MergeInput` is a UUID (a task number is MCP-only, per the git routes task); MCP passes a `TaskRef`.
- [ ] `.sqlx/` refreshed for the verifier query.

## Implementation notes
- Files: `orchestrator/src/tracker/handoffs.rs` (verifier), `orchestrator/src/prelude/state.rs` (wiring), `orchestrator/src/git/service.rs` (only if `merge_handoff` needs the label or ordering fix).
- One query: `SELECT h.commit, h.source_branch, h.review_status, t.current_handoff_id FROM tasks t LEFT JOIN task_handoffs h ON h.id = $3 AND h.task_id = t.id WHERE t.id = $2 AND t.project_id = $1` (or number-based lookup through `find_task` first).
- Do not lock the project row: a transaction holding the project row must never wait for the git lock, and the reverse order here (git lock, then a plain read) is the documented one.

## Edge cases
- Hand-off belongs to the task but is not current (superseded by a forward): 409 stale, even if approved.
- Current hand-off approved, then a new revision published between the UI reading the task and clicking merge: 409 stale (the id changed).
- Source session branch advanced after approval: the merge uses the pinned commit; the new commits are not merged (assert by content).
- Target branch already contains the commit: `merge_commit` returns the current tip ("already up to date"); 200.
- Merge conflict between the pinned commit and the target: 422 with `conflicts`, refs unchanged; the task's hand-off stays approved (a rebase and a new revision are the way forward).

## Testing
- `orchestrator/tests/handoffs_merge.rs` through `TestApp` and the REST endpoint, publishing hand-offs with the API from the previous task: publish revision A, forward with `approved`, advance the session branch to B and sync, `POST .../git/merge {task_id, handoff_id, target: "main"}` -> 200 and `refs/heads/main` contains A's file but not B's; unapproved (`unreviewed`) -> 409 and `main` unchanged; `changes_requested` -> 409; stale id after a second revision -> 409; unknown task -> 404; hand-off of another task in the project -> 409 stale; conflict -> 422 with `conflicts` and `main` unchanged; branch form on the same session still works and records no approval (task unchanged).
- `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Documentation
- `SPEC.md` "Git": add the default task-merge commit message format (`Merge handoff <id> (<source_branch>) into <target>`) next to `MergeInput`.

## Assumes from other epics
- "Git operations: mirror, clones, integration and REST API": `HandoffVerifier` trait, `ApprovedHandoff`, `GitService::merge_handoff` / `merge_commit`, the merge route and its 400/422 handling.
- "MCP server and agent tools": the `merge` tool's task form calls the same `merge_handoff` with `GitActor::Session`.