---
id: s4nmf
title: "Add TaskRepository core: project-locked mutation primitive, task-event append with notify, task states and profile served states"
status: done
priority: P1
created: "2026-09-16T20:32:02.151930431Z"
updated: "2026-09-17T07:59:27.381629277Z"
tags:
  - orchestrator
  - core
  - tracker
  - realtime
depends_on:
  - vnwqh
  - "2xrbu"
  - tu78t
parent: p5tsd
assignee: claude-opus-subagent
attempts: 1
---

## Summary
Create `TaskRepository<'a>` with the tracker mutation transaction primitive and the state-configuration tables: `begin_mutation` (one `READ COMMITTED` transaction that locks the project row), `append_task_events` (`MAX(seq)+1` per project plus `pg_notify('task_events', '<project_id>:<seq>')` in the same transaction), task-state CRUD with the documented deletion refusals and the default state set, and the `profile_states` link with its same-project and queue-kind checks. Task rows, dependencies, comments, hand-offs and session links follow in the next task; domain logic (claims, blocked recomputation, cycles, escalation) is the tracker epic's.

## Documents
- `docs/data-model.md` "Tracker mutation transactions", `task_states` (constraints, deletion refusals, default set), `profile_states`, `task_events` (append rule, `pg_notify`, validation that the event's task belongs to the project while it exists), "Notifications (LISTEN/NOTIFY channels)".
- `ARCHITECTURE.md` "Task tracker" ("One mutation at a time per project"; a project retains at least one queue state, exactly one human state, at least one terminal state), "Event delivery"; ADR 0021, ADR 0028.
- `SPEC.md` "Task states" (positions: missing appends, explicit shifts states at and after it; rename by id; `kind` immutable; 409 cases), "TaskEvent" (`states_changed` with `task_id` null).

## Acceptance criteria
- [ ] `TaskRepository::begin_mutation(&self, project_id) -> Result<Transaction<'_, Postgres>>` begins a transaction on the pool and calls `ProjectRepository::lock_project` inside it; `NotFound` when the project does not exist. Every other helper takes `&mut PgConnection` and documents that it must run inside such a transaction.
- [ ] `append_task_events(tx, project_id, events: &[NewTaskEvent]) -> Result<Vec<i64>>`: for each event, when `task_id` is `Some`, verify `EXISTS (SELECT 1 FROM tasks WHERE id = $task AND project_id = $project)` **unless** the event kind is `deleted` (the row is already gone); insert with `INSERT INTO task_events (project_id, seq, ts, task_id, kind, payload) SELECT $1, COALESCE(MAX(seq), 0) + 1, $2, $3, $4, $5 FROM task_events WHERE project_id = $1 RETURNING seq`; after the batch, one `SELECT pg_notify('task_events', $1)` with `<project_id>:<highest seq>`. Empty batch → no-op. `task_events_pkey` violation → `Error::Internal("task event sequence collision")` with `tracing::error!(project_id = %id)`.
- [ ] `list_task_events_after(project_id, after: i64, limit) -> Vec<TaskEventRow>` and `max_task_event_seq(project_id) -> i64` for the SSE replay path.
- [ ] Task states: `insert_default_states(tx, project_id) -> Vec<TaskState>` inserting `DEFAULT_TASK_STATES`; `insert_state(tx, project_id, NewTaskState { name, kind, position: Option<i32> })` (missing position = max + 1; explicit position shifts `position >= $p` up by one first; unique name → `Conflict("state name already taken")`; second human state → `Conflict("project already has a human state")`); `list_states(project_id)` ordered by `position`; `find_state_by_name(project_id, name)`; `find_state(project_id, id)`; `rename_state(tx, project_id, id, name)`; `move_state(tx, project_id, id, position)` re-packing positions to `0..n`; `default_state(tx, project_id)` = the queue state with the lowest position.
- [ ] `delete_state(tx, project_id, id) -> Result<()>` refuses with `Conflict` and the messages `cannot delete the human state`, `cannot delete the last queue state`, `cannot delete the last terminal state`, `state is in use by tasks` (checked with `EXISTS` under the project lock; the `RESTRICT` FK is the backstop), then deletes and re-packs positions.
- [ ] Profile served states: `set_profile_states(tx, project_id, profile_id, state_ids: &[Uuid])` replaces the link rows after verifying every state belongs to `project_id` and has `kind = 'queue'` and the profile belongs to `project_id` (`BadRequest("served states must be queue states of this project")`); `list_profile_states(profile_id) -> Vec<TaskState>`; `list_profile_state_ids_for_project(project_id) -> HashMap<Uuid, Vec<Uuid>>` for the profile list DTO.
- [ ] None of these helpers appends the `states_changed` event or the `updated` events themselves; callers compose helper + `append_task_events` in the same transaction (documented in the module doc).
- [ ] `cargo sqlx prepare` run and `.sqlx/` committed.

## Implementation notes
- Files: `orchestrator/src/repositories/tasks.rs` (may be a directory `repositories/tasks/{mod,states,events}.rs` if it grows), `src/repositories/mod.rs`.
- Locking order is fixed by ADR 0021: git lock (outside) → project row (`begin_mutation`) → session rows → task rows. Put that sentence in the module doc comment.
- `READ COMMITTED` is the Postgres default; do not change the isolation level.
- The notify payload uses bound parameters (`pg_notify($1, $2)`), never string-formatted SQL.

## Edge cases
- `insert_state` with `position` beyond the end appends; negative positions → `BadRequest`.
- `delete_state` on an unknown id → `NotFound`.
- `set_profile_states` with an empty slice is valid (profile serves nothing).
- Concurrent `begin_mutation` on the same project must serialize; on different projects must not.

## Testing
- Integration tests in `tests/repositories_tasks_core.rs` on `common::db::test_pool()` (seed user/project/profile via repositories or SQL): default states have the documented names, kinds and positions; `insert_state` shifting and appending; rename and move re-pack positions; each `delete_state` refusal; second human state → `Conflict`; `set_profile_states` rejects a non-queue state and a state of another project; `append_task_events` sequences 1..n, refuses a task from another project, allows `deleted` for a missing task; `PgListener` on `task_events` receives exactly one `<project_id>:<max seq>` after commit and nothing after rollback; two `begin_mutation` calls on one project serialize (second waits until first commits) while a third on another project proceeds.
- `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` must pass.

## Documentation
- None: implements the documented contract as written.

## Assumes from other epics
- none inside this epic beyond its dependencies; the tracker epic adds the `TaskEvent` payload assembly, the `states_changed` emission and all domain rules.