---
id: rujpc
title: Implement discovery-provenance resolution for session-created tasks (discovered_from inference, ambiguity rejection, parent-link suffices)
status: done
priority: P2
created: "2026-09-16T20:45:57.878683911Z"
updated: "2026-09-19T17:49:50.902236370Z"
tags:
  - orchestrator
  - tracker
  - mcp
depends_on:
  - "6jycg"
  - cws3a
parent: "5h3y4"
attempts: 1
---

## Summary
Extend `tracker::tasks::create_task` with the provenance rules the MCP `create_task` tool needs: when a session creates a task, infer `discovered_from` from the single task the session holds, require it explicitly when it holds several, record nothing when it holds none, validate an explicit origin against the caller's held tasks, and skip the redundant edge when the origin is the new task's parent. All of it is validated under the project lock before the task row is inserted, so a rejected request creates neither a task nor edges.

## Documents
- `ARCHITECTURE.md` "Task tracker" → "Discovery provenance" (infer only with exactly one held task; several → must supply; supplied origin must be held by the caller in the same project; none → no origin; validate under the project lock before insertion; parent link suffices when origin == parent; otherwise record `discovered_from` even when `blocks` connects the same pair).
- `SPEC.md` "MCP tool contracts" → `create_task` (input `discovered_from?: string | number`; provenance resolved after taking the project lock; multiple held → `invalid_argument` unless given; explicit origin must be currently held by the caller; parent already records provenance; a `blocks` edge to the same origin may coexist; `created_by_session_id` set; any validation failure creates nothing).
- `docs/data-model.md` `task_dependencies` (the "On MCP task creation, resolve `discovered_from`..." paragraph; `PRIMARY KEY (task_id, depends_on_task_id, kind)`).
- ADR 0023.

## Acceptance criteria
- [ ] `CreateTaskInput` gains `discovered_from: Option<TaskRef>`; `create_task` resolves provenance only when `created_by` is `CreatedBy::Session(session_id)` (user creations ignore the field: REST has no such input).
- [ ] Resolution under the lock, before `insert_task`: `held = TaskRepository::list_by_lease_holder(conn, session_id)` filtered to `project_id`; explicit `discovered_from` → resolve the `TaskRef` in this project (404 `Error::NotFound` if absent) and require it to be in `held` (`Error::BadRequest("discovered_from must be a task this session currently holds")`); omitted with `held.len() == 1` → that task; omitted with `held.len() > 1` → `Error::BadRequest("this session holds several tasks; pass discovered_from to name the originating task")`; omitted with none → no origin.
- [ ] After the row insert and the `blocks` edges: if `origin == parent` no edge is added; otherwise `insert_dependency(new, origin, DiscoveredFrom)` and one `dependency_added` event (informational kind: no `blocked` recompute); a `blocks` edge to the same origin from `depends_on` coexists.
- [ ] `created_by_session_id` is set and `created_by_user_id` null for session creations; `touch(new_task.id, session_id)` is recorded (the session created the task).
- [ ] The MCP error mapping for `BadRequest` is `invalid_argument` (the MCP epic maps it); the messages above are the ones agents will read, so keep them as written.

## Implementation notes
- Files: `orchestrator/src/tracker/tasks.rs` (extend `create_task`), possibly `orchestrator/src/tracker/provenance.rs` for `resolve_origin(m, session_id, explicit: Option<TaskRef>, parent: Option<Uuid>) -> Result<Option<Uuid>>` so it is unit-testable in isolation.
- The origin task is not modified: no `updated` event on the origin, no touch on it beyond the existing lease link.
- `depends_on` numbers and `discovered_from` numbers are resolved inside the same locked transaction as the insert.
- Event order for a session creation: `created` → `dependency_added` (blocks edges in order) → `dependency_added` (discovered_from) → `blocked` flip if any.

## Edge cases
- Explicit origin equals the new parent: parent link only, no edge, no extra event.
- Explicit origin held by the session but in another project: cannot happen (sessions belong to one project) but the `project_id` filter makes it 404 anyway.
- Held tasks that are terminal? Impossible (a state change clears the lease), but the query filters by holder only.
- A session creating a task while holding one task and passing `discovered_from` naming that same task: accepted (explicit and inferred agree).
- Explicit `discovered_from` naming the new task's future number is impossible (allocated after validation).

## Testing
- Integration tests in `orchestrator/tests/tracker_provenance.rs` on the test pool: session holding exactly one task creates without `discovered_from` → a `discovered_from` edge to it and a `dependency_added` event, `created_by_session_id` set; holding two → `BadRequest` with the exact message and no task row (count unchanged); holding two with explicit origin (by number) → edge to that one; explicit origin not held → `BadRequest`, no row; explicit origin unknown → `NotFound`; holding none → no edge; origin == `parent` → no edge; `depends_on` including the origin plus inferred provenance → both `blocks` and `discovered_from` rows on the pair; `task_sessions` row for the new task and the creating session.
- `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Database schema, models, repositories and test harness": `list_by_lease_holder`, `insert_dependency` with kind.
- "MCP server and agent tools": the `create_task` tool passes `CreatedBy::Session` and the optional `discovered_from`; no MCP transport code here.