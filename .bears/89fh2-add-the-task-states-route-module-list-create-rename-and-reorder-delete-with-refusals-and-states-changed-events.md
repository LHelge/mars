---
id: "89fh2"
title: "Add the task-states route module: list, create, rename and reorder, delete with refusals, and states_changed events"
status: open
priority: P1
created: "2026-09-16T20:41:39.156347309Z"
updated: "2026-09-16T20:41:39.156347309Z"
tags:
  - orchestrator
  - tracker
depends_on:
  - thes7
parent: "5h3y4"
---

## Summary
Expose the project's board columns over REST: `GET`, `POST`, `PUT` and `DELETE` under `/api/projects/{pid}/task-states`, each mutation running inside a `TrackerMutation` and emitting one `states_changed` event carrying the full state list after the change. The deletion refusals (human state, last queue, last terminal, state in use) and the single-human-state rule come from the repository; this task maps them to the documented statuses and adds the route-level validation.

## Documents
- `SPEC.md` "Task states (`/api/projects/{pid}/task-states`)": the four rows (`GET` → `TaskState[]` by `position`; `POST {name, kind, position?}` → 201, 400 invalid name or kind, 409 name taken or second `human`; `PUT /{name} {name?, position?}` → 200, 400 if `kind` is given, 409 new name taken; `DELETE /{name}` → 204, 409 while any task is in the state, for the `human` state, for the last `queue` state, for the last `terminal` state), `TaskState` shape, name rule `[a-z0-9][a-z0-9_-]*` 1–32, missing `position` appends and explicit one shifts states at and after it, rename by id renames everywhere, every change emits `states_changed`.
- `SPEC.md` "TaskEvent" (`states_changed`: `task_id` null, `states` = full list after the change).
- `docs/data-model.md` `task_states` (constraints, partial unique human index, refusal list, default set), `task_events` (`states_changed` written so a connected board learns about new columns).
- `ARCHITECTURE.md` "Task tracker" → "State is a queue, defined per project" (retain at least one queue, exactly one human, at least one terminal).
- ADRs 0016, 0021, 0022.

## Acceptance criteria
- [ ] `orchestrator/src/routes/task_states.rs` exports `routes() -> Router<AppState>` nested at `/projects/{pid}/task-states`; all handlers require a JWT user (`CurrentUser`), any authenticated user may edit states (no admin gate).
- [ ] `GET` → 200 `TaskState[]` ordered by `position`; 404 for an unknown project. No lock, no event.
- [ ] `POST` body `{name, kind, position?}`: name validated through `TaskStateName::parse` (400 with the model message), `kind` must deserialise to `queue|human|terminal` (400 `invalid state kind` on anything else), negative `position` → 400; inside one `TrackerMutation` call `insert_state`, then `list_states` and `emit_states_changed`; commit; 201 with the new `TaskState`. Repository `Conflict` messages (`state name already taken`, `project already has a human state`) pass through as 409.
- [ ] `PUT /{name}` body `{name?, position?}`: the path `{name}` resolves the state by name (404 if absent); a body containing `kind` (any value, even the current one) → 400 `kind is immutable`; an empty body → 200 with the unchanged state and no event (no-op rule); rename runs `rename_state` (409 `state name already taken`), reorder runs `move_state` (positions re-packed to `0..n`); one `states_changed` event per request that changed something; 200 with the updated `TaskState`.
- [ ] `DELETE /{name}` → 204; the four refusals return 409 with the repository messages (`cannot delete the human state`, `cannot delete the last queue state`, `cannot delete the last terminal state`, `state is in use by tasks`); success emits `states_changed` with the remaining list.
- [ ] Renaming or deleting a state that a profile serves needs no extra handling (links are by id; deletion cascades `profile_states`), but the route documents in a comment that the profile's `serves_states` changes accordingly.
- [ ] Router mounted in the API router; `cargo sqlx prepare` if new queries were added.

## Implementation notes
- Files: `orchestrator/src/routes/task_states.rs`, `orchestrator/src/routes/mod.rs`, `orchestrator/src/tracker/states.rs` (domain functions `create_state`, `update_state`, `delete_state` taking `&mut TrackerMutation` so the MCP epic or tests can reuse them without HTTP).
- Request DTOs are private to the route module: `CreateStateRequest { name: String, kind: TaskStateKind, position: Option<i32> }`, `UpdateStateRequest { name: Option<String>, position: Option<i32>, kind: Option<serde_json::Value> }` (the `kind` field exists only to detect and reject its presence).
- Actor for every event is `TaskActor::User { user_id }`.
- Use `#[serde(deny_unknown_fields)]` only if the rest of the API does; otherwise detect `kind` explicitly as above.

## Edge cases
- `POST` with `position` beyond the end appends (repository behaviour).
- `PUT` renaming a state to its current name and no `position` → no-op, no event.
- `PUT` with `position` equal to the current position → no-op, no event.
- Deleting a state while a concurrent request moves a task into it: both take the project lock; the delete either happens first (the move then fails with unknown state, 400) or observes the task and returns 409.
- Path `{name}` is a name, not an id; UUIDs in the path are 404.

## Testing
- Integration tests in `orchestrator/tests/task_states_api.rs` through `TestApp` (login helper, ready project): list returns the seven defaults in order; create a queue state without position appends at 7, with `position: 1` shifts `ready` to 2; invalid name (`Ready`, 33 chars, empty) → 400; unknown kind → 400; duplicate name → 409; second `human` → 409; rename `review` to `qa` → 200 and tasks in it report `state = "qa"`; `PUT` with `kind` → 400; reorder re-packs positions; delete `done` with `cancelled` present → 204; delete the last terminal, last queue (after deleting the others), the human state, and a state holding a task → 409 with the exact messages; every successful mutation appends exactly one `states_changed` row with `task_id` null and the full list; no row on the rejected ones; unauthenticated → 401.
- `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Database schema, models, repositories and test harness": `TaskRepository` state helpers (`insert_state`, `rename_state`, `move_state`, `delete_state`, `list_states`, `find_state_by_name`) and `TaskStateName::parse`.
- "Authentication, users, invites and email": the `CurrentUser` extractor and `TestApp` login helpers.
- "Projects, agent profiles and shared directories": project creation seeding the default states (tests may seed through the repository directly until then).