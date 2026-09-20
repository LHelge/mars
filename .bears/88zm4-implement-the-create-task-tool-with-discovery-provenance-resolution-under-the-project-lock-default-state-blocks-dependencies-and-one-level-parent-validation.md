---
id: "88zm4"
title: Implement the create_task tool with discovery provenance resolution under the project lock, default state, blocks dependencies and one-level parent validation
status: in_progress
priority: P1
created: "2026-09-16T20:47:08.167675829Z"
updated: "2026-09-20T08:01:45.980516758Z"
tags:
  - orchestrator
  - mcp
  - tracker
depends_on:
  - "4cvu4"
parent: qgj33
attempts: 1
---

## Summary
Implement `create_task`: an agent files a follow-up, bug or sub-task with `created_by_session_id` set, in the named state or the project's default state, with optional `blocks` dependencies and a one-level parent, and with discovery provenance resolved under the project lock: omission infers the sole held task, records nothing when none is held, and fails when several are held; an explicit origin must be held by the caller; the parent link suffices when the origin is the parent, otherwise a `discovered_from` edge is added. Any validation failure creates neither the task nor any edge.

## Documents
- `SPEC.md` "MCP tool contracts" → `create_task` (input `{ title, description?, state?, priority? = 2, labels?, parent?, depends_on?, discovered_from? }`, output `{ task }`; `state` defaults to the project's default state (first `queue` state), unknown → `invalid_argument`; `depends_on` creates `blocks`; `created_by_session_id` set; provenance rules after taking the project lock; same one-level parent validation as REST; any failure creates nothing).
- `SPEC.md` "Tasks" (`POST` semantics, `priority` 0–3 default 2, `parent_id` one level, `number` from the project counter), "TaskEvent" (`created`, `dependency_added`, `blocked`).
- `ARCHITECTURE.md` "Task tracker" → "Discovery provenance", "Parents", "Blocked is stored", "One mutation at a time per project"; ADR 0023 (provenance decision), ADR 0021, ADR 0030.
- `docs/data-model.md` `tasks` (`title` 1–200, `labels` pattern, `created_by_session_id`, `number` allocation), `task_dependencies` (MCP provenance paragraph; PK includes `kind`; `blocks` may coexist with `discovered_from`), `task_states` (default state = queue state with the lowest position), `task_sessions` (upsert on create).

## Acceptance criteria
- [ ] Validation before the lock: `non_empty("title")` and title length 1–200 via the tracker model (`invalid_argument` with the model's message), `validate_priority` (default 2), labels via the model pattern, parse every `TaskArg` (`parent`, `depends_on[]`, `discovered_from`) → `invalid_argument` on malformed values.
- [ ] Inside `begin_mutation(project_id)`: resolve `state` by name (`invalid_argument` `unknown state "<name>"; valid states: ...`) or `default_state(tx, project_id)`; resolve `parent` and each `depends_on` entry within the project (`not_found` `task not found` naming the argument, e.g. `parent task not found`, `depends_on task #7 not found`); read the caller's held tasks: `SELECT id FROM tasks WHERE project_id = $1 AND lease_holder_session_id = $2`; provenance: explicit `discovered_from` not in the held set → `invalid_argument` `discovered_from must be a task you currently hold`; omitted with several held → `invalid_argument` `you hold several tasks; specify discovered_from`; omitted with one held → that task; omitted with none → no origin; then the tracker service's `create_task(tx, NewTask { ..., created_by: Session(ctx.session_id) })` which allocates `number`, validates the parent (one level, same project, different task → `invalid_argument`), inserts `blocks` edges (duplicates in `depends_on` collapsed), inserts the `discovered_from` edge to the origin unless `origin == parent`, computes `blocked` from the new edges and recomputes the parent's `blocked`, appends `created`, one `dependency_added` per edge and `blocked` when set, upserts the caller's link on the new task; commit; output `TaskOutput` with `depends_on` listing both kinds.
- [ ] A `blocks` edge to the origin (origin listed in `depends_on`) coexists with the `discovered_from` edge for the same pair.
- [ ] Any failure after the lock rolls back everything: no task row, no edges, no events, `projects.next_task_number` unchanged.
- [ ] `tests/mcp_create_task.rs`: minimal input → task in `backlog` (default state), `priority = 2`, `created_by_session_id = caller`, `number` allocated, `created` event with actor session, link row; explicit `state: "ready"`; unknown state → `invalid_argument` listing names; `priority: 4` → `invalid_argument`; title of 201 chars → `invalid_argument`; `parent: "#3"` where `#3` has children → OK; `parent` where the parent itself has a parent → `invalid_argument`; `depends_on: ["#1", 2]` → two `blocks` edges, `blocked = true`, `dependency_added` twice and `blocked` once; unknown `depends_on` → `not_found` and no task created (number counter unchanged); provenance: caller holds one task and omits `discovered_from` → `discovered_from` edge to it; holds none → no edge; holds two and omits → `invalid_argument`, nothing created; holds two and names one → edge to that one; names a task held by another session → `invalid_argument`; origin equals `parent` → no `discovered_from` edge; origin also in `depends_on` → both edges present; REST `GET /projects/{pid}/tasks/{id}` shows the same `depends_on` list.
- [ ] `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Implementation notes
- Files: `orchestrator/src/mcp/tools/create_task.rs`, `orchestrator/tests/mcp_create_task.rs`.
- Provenance is resolved in the handler (it is MCP-specific) but inside the tracker transaction, before calling the tracker's `create_task`; pass the resolved `origin: Option<Uuid>` to the tracker service, which owns edge insertion and events. If the tracker epic implements provenance itself, call it and delete the handler's copy; one implementation only.
- `created_by_user_id` stays `NULL`; exactly one creator column is set.
- The event actor is `{ kind: "session", session_id }`; no email is sent for creation.

## Edge cases
- `depends_on` containing the new task's parent: allowed (a parent blocks nothing; a `blocks` edge to it is legal but odd); no special casing.
- `discovered_from` naming a task in another project (by UUID) → not in the held set → `invalid_argument`.
- `labels` with duplicates → deduplicated by the model.
- `description` omitted → `''`.

## Testing
- Integration tests as listed.
- Command: `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Task tracker: states, tasks, leases, dependencies and events": `create_task(tx, NewTask)` with parent validation, number allocation, edge insertion including `discovered_from`, blocked computation and events; `default_state`; the task model's title and label validation.