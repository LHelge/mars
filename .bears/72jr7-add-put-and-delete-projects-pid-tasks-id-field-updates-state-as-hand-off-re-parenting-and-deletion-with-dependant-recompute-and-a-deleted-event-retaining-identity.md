---
id: "72jr7"
title: "Add PUT and DELETE /projects/{pid}/tasks/{id}: field updates, state as hand-off, re-parenting, and deletion with dependant recompute and a deleted event retaining identity"
status: open
priority: P1
created: "2026-09-16T20:43:37.553652768Z"
updated: "2026-09-16T20:43:37.553652768Z"
tags:
  - orchestrator
  - tracker
depends_on:
  - cuw5s
  - "6jycg"
parent: "5h3y4"
---

## Summary
Implement the user's edit and delete surface for tasks. `PUT` updates any supplied field, treats a different `state` as a hand-off through `change_state`, re-parents under the one-level rules, and emits `updated` only when something actually changed and `state_changed` only when the state changed. `DELETE` captures dependants and parent, deletes the row, recomputes surviving dependants, emits `dependency_removed` per surviving edge and a `deleted` event that keeps the original `task_id`. The domain function `tracker::tasks::update_task` is reused by the MCP `update` tool and wrapped by the Code hand-offs epic for `handoff` inputs.

## Documents
- `SPEC.md` "Tasks": `PUT /projects/{pid}/tasks/{id} {title?, description?, state?, priority?, labels?, parent_id?, assignee_user_id?, handoff?: HandoffInput}` → 200 `Task`; `DELETE` → 204; a user may set any state and is not bound by leases; a different state clears the lease and resets `attempts`; terminal sets `closed_at` and unblocks dependants; non-terminal on a closed task reopens; current state is a no-op preserving lease, `attempts`, `closed_at`, emitting no state-change or escalation event, other fields still updated; a request with no effective changes emits no task event; parent rules (400); deleting a prerequisite removes incident edges and recomputes surviving dependants in the same transaction, emits `dependency_removed` per affected surviving dependant plus `blocked`/`unblocked`; the deleted task retains its event identity.
- `SPEC.md` "Code hand-offs and review": `handoff` requires a different target `state` in the same update (400 otherwise) and a non-empty `comment`.
- `SPEC.md` "TaskEvent": `updated`, `state_changed`, `deleted` (carries the original UUID in `task_id`, omits `task`), `dependency_removed`, `blocked`, `unblocked`.
- `docs/data-model.md` `tasks` (parent checks on re-parenting; `parent_id` FK `ON DELETE SET NULL`; before deleting capture dependants and parent, recompute after the cascades), `task_events` (`deleted` allowed for a missing task, no FK).
- `ARCHITECTURE.md` "Task tracker" → "Parents" (flag recomputed when a child is re-parented or deleted), "Blocked is stored".
- ADRs 0022, 0023, 0030.

## Acceptance criteria
- [ ] `tracker::tasks::update_task(m, task: &Task, input: UpdateTaskInput) -> Result<UpdateOutcome { task: TaskDto, changed: bool }>` with `UpdateTaskInput { title?, description?, priority?, labels?, parent: Option<Option<Uuid>> (Some(None) clears), assignee_user_id: Option<Option<Uuid>>, state: Option<String>, needs_human_reason: Option<String> }`. Order: (1) field update through `TaskRepository::update_task` (returns whether any column changed; re-parenting runs the one-level checks → 400 messages from the repository); (2) if `parent` changed, `recompute_blocked` on the old parent and the new parent; (3) if `state` is given, `resolve_state` (400) then `change_state` (no-op when equal); (4) emit `updated` with the task after all changes only if step 1 changed a column, and `state_changed` comes from step 3; no events at all when nothing changed; `touch_actor` only when something changed.
- [ ] Event order when both fields and state change: `updated` first, then `state_changed`, then flips. (Documented in the function's doc comment; the frontend only refreshes so the order matters for history readability, not correctness.)
- [ ] `PUT` route: body `UpdateTaskRequest` private to the route; `assignee_user_id` naming a missing user → 400 `unknown assignee` (map the FK violation); `handoff` present: if `state` is missing or equals the current state → 400 `handoff requires a different target state`; if `comment` is empty → 400 `handoff comment must not be empty`; otherwise call `tracker::handoffs::publish` if the hand-off epic has provided it, else return 400 `code hand-offs are not available yet` (leave a clearly marked extension point; the Code hand-offs epic replaces it).
- [ ] `tracker::tasks::delete_task(m, task: &Task) -> Result<()>`: `graph::capture_before_delete` → `TaskRepository::delete_task` → for each captured `(dependant, kind)` emit `dependency_removed` (payload: actor + the dependant's task after the cascade) → `recompute_blocked(dependants ∪ parent)` → `emit_deleted(task.id)` last. Children keep existing with `parent_id = NULL` (FK) and receive no event (their `blocked` cannot change from losing a parent).
- [ ] `DELETE` route → 204; 404 unknown task; a held task can be deleted by a user (lease is not a barrier; the holder session will get `not_found` on its next call).
- [ ] `cargo sqlx prepare` run if queries were added.

## Implementation notes
- Files: `orchestrator/src/tracker/tasks.rs` (extend), `orchestrator/src/routes/tasks.rs` (extend), `orchestrator/src/tracker/handoffs.rs` (stub module with `pub async fn publish(...) -> Result<...>` returning `BadRequest("code hand-offs are not available yet")`, `#[allow(unused)]` until the hand-off epic fills it).
- Distinguish "field absent" from "field null" with `Option<Option<T>>` and `#[serde(default, deserialize_with = "double_option")]` for `parent_id` and `assignee_user_id`.
- Deleting a task with hand-off refs is the Code hand-offs epic's concern (refs removal under the git lock); this task must not take the git lock. Leave a doc comment: "hand-off ref cleanup is added by the Code hand-offs epic before this function is called".
- The `deleted` event's payload is `{actor}`; `task_id` column carries the UUID; `append_task_events` skips the existence check for `deleted`.

## Edge cases
- `PUT` with `{state: <current>}` and nothing else → 200, no event, lease and `attempts` intact (the explicit acceptance criterion of the epic).
- `PUT` with `{title: <same title>}` → no column change → no event.
- `PUT` with `parent_id: null` on a top-level task → no change; on a child → change, old parent recomputed.
- `PUT` setting `parent_id` to a task that is the task's own child → 400 (the task has children).
- `PUT` state to a terminal state on a task with open children: allowed (users may do anything); the task keeps `blocked = true` but is closed; its parent closure logic runs as usual.
- `DELETE` of a parent: children become top-level; `DELETE` of a child that was the last open child does not close the parent (closure is only triggered by a state change; document this as intended: the parent's `blocked` becomes false and a person closes it).
- `DELETE` racing a claim: the project lock serialises; the claim fails with `not_found` after the delete.

## Testing
- Extend `orchestrator/tests/tasks_api.rs`: `PUT` title only → `updated` only; `PUT` state `ready` → `state_changed` with `from/to`, lease cleared, `attempts = 0`; `PUT` current state on a held task with `attempts = 2` → no event, lease and attempts unchanged; `PUT` title + state → `updated` then `state_changed`; `PUT` to `done` → `closed_at` set and dependant `unblocked`; `PUT` from `done` to `backlog` → `closed_at` null and dependant `blocked`; re-parent from A to B → both parents' `blocked` recomputed with events; each parent-rule violation → 400 with the repository message; `handoff` without a state change → 400 `handoff requires a different target state`; `assignee_user_id` unknown → 400; `DELETE` of a prerequisite with two dependants (one also `discovered_from`) → 204, two `dependency_removed` for `blocks`, one for `discovered_from`, `unblocked` for the dependants that had no other open prerequisite, and a final `deleted` event whose `task_id` equals the deleted UUID and whose JSON has no `task` key; earlier events for that task still carry the UUID; `DELETE` unknown → 404; unauthenticated → 401.
- `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Database schema, models, repositories and test harness": `update_task` reporting whether a column changed and running the re-parent checks, `delete_task`.
- "Code hand-offs and review": replaces the `tracker::handoffs::publish` stub and adds ref cleanup before `delete_task`.