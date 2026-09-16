---
id: rvddq
title: "Add TaskRepository row helpers: tasks with number allocation, dependencies, comments, hand-offs and session links"
status: open
priority: P1
created: "2026-09-16T20:32:42.530907366Z"
updated: "2026-09-16T20:32:42.530907366Z"
tags:
  - orchestrator
  - core
  - tracker
depends_on:
  - s4nmf
parent: p5tsd
---

## Summary
Extend `TaskRepository<'a>` with the basic row operations for `tasks`, `task_dependencies`, `task_comments`, `task_handoffs` and `task_sessions`, all designed to run inside the project-locked mutation transaction from `begin_mutation`: task insert with number allocation and default state, lookups by id or per-project number, field updates, deletion, dependency edge insert/delete with the same-project check, comment insert with the author rule, hand-off insert with the same-project validation, and the `task_sessions` upsert with the first/last-touched semantics. Claims, releases, blocked recomputation, cycle checks, parent closure and event payload assembly stay in the tracker and hand-off epics.

## Documents
- `docs/data-model.md` `tasks` (number allocation, default state, constraints, repository checks for parents), `task_dependencies` (same-project check in the repository; edge identity includes `kind`), `task_comments` (author rule enforced at insert), `task_handoffs` (repository validation: same project, exactly one creation actor, reviewer rule), `task_sessions` (upsert preserving `first_touched_at`, advancing `last_touched_at` only for an actual change; ADR 0030).
- `SPEC.md` "Tasks" (`{id}` accepts UUID or per-project number; list filters `state`, `label`, `priority`, `parent`, `held`; ordering by priority then number), "Code hand-offs and review" (`Handoff` fields).
- `ARCHITECTURE.md` "Task tracker" ("Parents": one level, validated on creation and re-parenting under the project lock).

## Acceptance criteria
- [ ] `TaskRef` enum (`Id(Uuid)` | `Number(i32)`) with `FromStr` (all-digits → `Number`, else UUID) and `find_task(project_id, TaskRef) -> Option<Task>` / `find_task_for_update(tx, project_id, TaskRef)` (adds `FOR UPDATE`).
- [ ] `insert_task(tx, project_id, NewTask) -> Task`: allocates `number` through `ProjectRepository::allocate_task_number`, resolves `state_id` to `default_state(project_id)` when the caller gives none, and validates a `parent_id` under the lock: same project, parent has no `parent_id`, the new task has no children (trivially true on insert) → otherwise `BadRequest("parent must be a top-level task of the same project")`.
- [ ] `update_task(tx, project_id, id, TaskUpdate { title?, description?, priority?, labels?, assignee_user_id?, parent_id?: Option<Option<Uuid>> }) -> Task` sets only the supplied fields plus `updated_at = NOW()`; re-parenting re-runs the parent checks and additionally rejects when the task itself has children (`BadRequest("a task with children cannot get a parent")`) or the parent already has a parent (`BadRequest("a task with a parent cannot receive children")`). Returns `Ok(None)`-style signal (or a `changed: bool`) when no column actually changed, so callers can skip the `updated` event (ADR 0030).
- [ ] `set_task_state_fields(tx, project_id, id, StateFields { state_id, lease: Option<(Uuid, DateTime)>, attempts, closed_at, needs_human_reason, current_handoff_id, blocked })` is one generic `UPDATE` the tracker epic composes hand-offs, releases and escalations from; every field is `Option` and only supplied fields change.
- [ ] `list_tasks(project_id, TaskFilter { state_id?, label?, priority?, parent_id?, held?: bool }) -> Vec<Task>` ordered by `priority, number`; `list_tasks_by_state_kind(kind) -> Vec<Task>` across projects (dashboard: `?state_kind=human`); `list_children(project_id, parent_id)`; `list_by_lease_holder(session_id)`.
- [ ] `delete_task(tx, project_id, id) -> Result<bool>`; the caller captures dependants and children before calling it (documented).
- [ ] Dependencies: `insert_dependency(tx, project_id, task_id, depends_on, kind)` verifying both tasks belong to `project_id` (`BadRequest("dependency must reference tasks of the same project")`), PK violation → `Conflict("dependency already exists")`, self-edge → `BadRequest`; `delete_dependency(tx, project_id, task_id, depends_on, kind) -> bool`; `list_dependencies(project_id, task_id) -> Vec<TaskDependency>` (outgoing); `list_dependants(project_id, task_id, kind) -> Vec<Uuid>` (incoming, for `blocks` lists and recomputation).
- [ ] Comments: `insert_comment(tx, project_id, NewTaskComment) -> TaskComment` running `NewTaskComment::validate` and verifying the task belongs to the project; `list_comments(project_id, task_id)` ordered by `created_at`.
- [ ] Hand-offs: `insert_handoff(tx, project_id, NewTaskHandoff) -> TaskHandoff` running the model validation and verifying task, comment (belongs to the task) and any session ids (belong to the project) with `EXISTS` queries; `find_handoff(project_id, id)`; `list_handoffs(project_id, task_id)` oldest first.
- [ ] `touch_task_session(tx, task_id, session_id)`: `INSERT ... ON CONFLICT (task_id, session_id) DO UPDATE SET last_touched_at = NOW()` (never touches `first_touched_at`); `list_task_sessions(task_id)`; `list_tasks_for_session(session_id) -> Vec<Task>` for `GET /sessions/{id}/tasks`.
- [ ] Scope in every `WHERE`; `cargo sqlx prepare` run and `.sqlx/` committed.

## Implementation notes
- Files: `orchestrator/src/repositories/tasks.rs` (or `repositories/tasks/{rows,dependencies,comments,handoffs,links}.rs`).
- `TEXT[]` labels bind as `&[String]`; `SMALLINT` binds as `i16`.
- `list_tasks` with `held = Some(true)` filters `lease_holder_session_id IS NOT NULL`; `label` filters `$1 = ANY(labels)`.
- All helpers except pure reads take `&mut PgConnection` and document "inside `begin_mutation`".
- Do not emit `TaskEvent`s here; callers compose with `append_task_events`.

## Edge cases
- `insert_task` when the project has no queue state (states were deleted down to human + terminal only, which `delete_state` prevents) → `Conflict("project has no queue state")` as a defensive check.
- `TaskRef::Number` for a number that does not exist → `None`, never a UUID parse error.
- `insert_dependency` for a pair that already has a different kind succeeds (different PK).
- `touch_task_session` must be idempotent within one transaction.

## Testing
- Unit tests: `TaskRef` parsing (`42`, `#42` is *not* accepted here, that is the frontend's search syntax; UUID; garbage → error).
- Integration tests in `tests/repositories_tasks_rows.rs`: numbers 1, 2, 3 for three inserts and never reused after a delete; default state is `backlog`; parent rules (three rejections and the allowed case); `update_task` reports no change for identical input; dependency same-project rejection, duplicate → `Conflict`, coexisting `blocks` + `discovered_from`; comment author rule violations rejected; hand-off insert with each validation failure; `touch_task_session` preserves `first_touched_at` and advances `last_touched_at`; list filters and ordering.
- `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` must pass.

## Documentation
- None: implements the documented contract as written.

## Assumes from other epics
- The session rows used in tests come from `SessionRepository` of this epic; no other epic is required.