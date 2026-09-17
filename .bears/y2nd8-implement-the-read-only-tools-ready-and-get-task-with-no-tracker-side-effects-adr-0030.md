---
id: y2nd8
title: Implement the read-only tools ready and get_task with no tracker side effects (ADR 0030)
status: open
priority: P1
created: "2026-09-16T20:44:32.498263983Z"
updated: "2026-09-17T04:59:12.996632415Z"
tags:
  - orchestrator
  - mcp
  - tracker
depends_on:
  - h9u3c
  - cws3a
parent: qgj33
---

## Summary
Implement `ready` (claimable tasks in the calling profile's served states, ordered by priority then number, limited) and `get_task` (the full `TaskDetail` for a task in the caller's project). Both are plain reads: no project lock, no `task_events`, no `task_sessions` link, no touch-timestamp change. The tracker task `cws3a` owns the shared query, summary mapping and validation policy documented in `SPEC.md`; this task exposes them through MCP.

## Documents
- `SPEC.md` "MCP tool contracts" → `ready` (input `{ limit?: number = 20 }`, output `{ tasks: TaskSummary[] }`, `TaskSummary = { id, number, title, state, priority, labels, description_excerpt, attempts, depends_on_count }`, "tasks in the calling profile's served states that are not blocked and have no lease holder, ordered by `priority` then `number`. A profile that serves no states gets an empty list"), `get_task` (input `{ task }`, output `{ task: TaskDetail }` same shape as REST), intro paragraph (`ready` and `get_task` do not write tracker events, create session-task links, or advance touch timestamps).
- `SPEC.md` "Tasks" (`Task`, `TaskDetail = Task & { comments, handoffs, children, sessions }`; hand-offs ordered oldest first; `{id}` accepts UUID or per-project number).
- `ARCHITECTURE.md` "MCP design" → "Side effects"; "Task tracker" → "State is a queue" (`ready` returns claimable tasks in served states only), "Blocked is stored" (a blocked task is excluded from `ready` in every state).
- `docs/data-model.md` `tasks` (`tasks_claimable_idx (project_id, state_id, priority, number) WHERE NOT blocked AND lease_holder_session_id IS NULL`), `profile_states`, `task_sessions` (read-only calls create no rows), ADR 0030.

## Acceptance criteria
- [ ] `ready::handle`: validate `limit` (default 20, integer 1–100; otherwise `invalid_argument`), read served state ids through `TaskRepository::list_profile_states(ctx.profile.id)`, and call `tracker::leases::ready_summaries(pool, project_id, &served_state_ids, limit)` from `cws3a`. An empty served-state list returns `ReadyOutput { tasks: [] }` without a task query. Do not add a second `list_claimable` query or summary mapper.
- [ ] `description_excerpt` is produced by trimming the description, replacing each newline sequence (CRLF, LF or CR) with one space, then taking the first 200 Unicode scalar values and trimming trailing whitespace, without an ellipsis; `depends_on_count` counts outgoing dependencies of every kind (it equals `Task.depends_on.len()`).
- [ ] `get_task::handle`: `input.task.parse()?` then the tracker epic's `TaskRepository::resolve(project_id, TaskRef) -> Option<Uuid>`; `None` → `not_found` with message `task not found`; then the same `TaskDetail` loader the REST `GET /projects/{pid}/tasks/{id}` uses; output `TaskDetailOutput { task }`.
- [ ] Neither handler opens a `begin_mutation` transaction or calls `append_task_events`/`upsert_task_session`; both run with plain pool reads.
- [ ] The handler matches the `SPEC.md` `ready` contract for limits, excerpt normalization and dependency counts.
- [ ] `tests/mcp_ready_get.rs` (TestApp, a project with default states, a profile serving `ready`, tasks created through the tracker epic's REST or repository): ordering by priority then number; a `blocked` task, a held task, a task in `backlog`, and a task of another project are excluded; `limit: 2` returns two; default limit 20 and explicit limits 1 and 100 accepted; `limit: 0`, `limit: 101` and fractional limits → `invalid_argument`; a profile serving no states returns `[]`; `description_excerpt` truncation at 200 chars on a multi-byte description; `get_task` by UUID, by number `12`, by `"12"`, by `"#12"`; unknown number → `not_found`; a task of another project by UUID → `not_found`; `"#abc"` → `invalid_argument`; `TaskDetail` contains comments, hand-offs, children and sessions matching the REST response for the same task (`serde_json::Value` equality).
- [ ] ADR 0030 audit test: record `COUNT(*) FROM task_events`, `COUNT(*) FROM task_sessions` and `tasks.updated_at` for the project, call `ready` three times and `get_task` three times, and assert all three are unchanged.
- [ ] `cargo sqlx prepare` run and `.sqlx/` committed; `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Implementation notes
- Files: `orchestrator/src/mcp/tools/ready.rs`, `orchestrator/src/mcp/tools/get_task.rs`, `orchestrator/tests/mcp_ready_get.rs`, `SPEC.md`.
- Served states are read outside any lock; a concurrent profile edit may make the list one call stale, which is acceptable for a read.
- The state name comes from the join, never from a second query per row.

## Edge cases
- `description` shorter than 200 chars → returned whole, trimmed.
- A task whose state was renamed shows the new name (join by id).
- A task in a served state that is `blocked` by an open child (not only by a `blocks` edge) is excluded, because `blocked` is stored.

## Testing
- Integration tests as listed; the excerpt rule gets a unit test on the helper function.
- Command: `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- none: implements the documented `ready` limits, excerpt normalization and dependency counts as written.

## Assumes from other epics
- "Task tracker: states, tasks, leases, dependencies and events": `TaskRepository::resolve(project_id, TaskRef)`, the `TaskDetail` loader shared with REST, `list_profile_states` (from the DB epic), and a way to create tasks/dependencies in tests.
- "Code hand-offs and review": `handoffs` in `TaskDetail`.