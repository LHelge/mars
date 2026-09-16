---
id: "4qqjs"
title: Define TaskEvent, TaskActor and the API Task, TaskDetail, Comment and Handoff DTOs with their repository loaders
status: open
priority: P0
created: "2026-09-16T20:39:58.382423897Z"
updated: "2026-09-16T20:51:52.106460720Z"
tags:
  - orchestrator
  - tracker
  - realtime
  - docs
depends_on:
  - pkaee
parent: "5h3y4"
---

## Summary
Define the wire types the whole tracker produces: the `TaskEvent` struct and its `TaskActor` in `events/`, and the API-facing `Task`, `TaskDetail`, `Comment` and `Handoff` DTOs (state as a name, `depends_on` with kinds, `blocks` list, current hand-off) in a new `tracker/` module, together with the read-only repository loaders that assemble them from the row types. Every route, MCP tool and event payload in later tasks reuses these; nothing here writes to the database.

## Documents
- `SPEC.md` "TaskEvent" (the `TaskEvent` interface verbatim: `seq`, `ts`, `task_id`, `actor`, `kind`, optional `task`, `comment`, `from`, `to`, `reason`, `states`; `deleted` omits `task`; `states_changed` has `task_id` null).
- `SPEC.md` "Tasks": `Task = { id, project_id, number, title, description, state, priority, blocked, labels, parent_id, assignee_user_id, lease_holder_session_id, lease_since, attempts, needs_human_reason, handoff: Handoff | null, depends_on: {task_id, kind}[], blocks: id[], created_at, updated_at, closed_at }`; `TaskDetail = Task & { comments: Comment[], handoffs: Handoff[], children: Task[], sessions: {session_id, first_touched_at, last_touched_at}[] }`, hand-offs oldest first; `Comment = { id, task_id, author_user_id, author_session_id, system, body, created_at }`; `{id}` accepts UUID or per-project number.
- `SPEC.md` "Code hand-offs and review": `Handoff` shape (read only here).
- `SPEC.md` "Task states": `TaskState = { id, project_id, name, kind, position, created_at }`.
- `docs/data-model.md` `task_events` (columns `project_id, seq, ts, task_id, kind, payload`), `tasks`, `task_dependencies`, `task_comments`, `task_handoffs`, `task_sessions`.
- `ARCHITECTURE.md` "Orchestrator internals" (module layout; `events/` holds `AgentEvent`/`TaskEvent` types).
- ADR 0022 (event identity survives deletion).

## Acceptance criteria
- [ ] `orchestrator/src/events/task_event.rs` defines `TaskActor` serialised as `{ "kind": "user", "user_id" }`, `{ "kind": "session", "session_id" }` or `{ "kind": "system" }` (`#[serde(tag = "kind", rename_all = "snake_case")]`), `TaskEventKind` with the thirteen kinds serialised in `snake_case` exactly as `SPEC.md` lists them, and `TaskEvent { seq: i64, ts: DateTime<Utc>, task_id: Option<Uuid>, actor: TaskActor, kind: TaskEventKind, task: Option<TaskDto>, comment: Option<CommentDto>, from: Option<String>, to: Option<String>, reason: Option<String>, states: Option<Vec<TaskState>> }` with `#[serde(skip_serializing_if = "Option::is_none")]` on every optional field.
- [ ] `TaskEventPayload { actor, task, comment, from, to, reason, states }` is what is stored in `task_events.payload`; `TaskEvent::from_row(TaskEventRow)` and `TaskEvent::to_row_parts()` round-trip (seq, ts, task_id, kind come from columns, the rest from the payload) with a unit test.
- [ ] `orchestrator/src/tracker/dto.rs` defines `TaskDto`, `TaskDetailDto`, `CommentDto`, `HandoffDto`, `TaskSessionLinkDto { session_id, first_touched_at, last_touched_at }` and `DependencyRef { task_id, kind }` with `snake_case` field names exactly as in `SPEC.md`; `priority` serialises as a JSON number, `state` as the state name, `handoff` as `null` when `current_handoff_id` is null.
- [ ] `TaskRepository` gains read-only loaders: `load_task_dto(&self, project_id, task_id) -> Result<Option<TaskDto>>`, `load_task_dtos(&self, project_id, tasks: &[Task]) -> Result<Vec<TaskDto>>` (batch: one query each for states, outgoing dependencies, incoming `blocks` edges and current hand-offs, no N+1), `load_task_detail(&self, project_id, TaskRef) -> Result<Option<TaskDetailDto>>` (comments by `created_at`, hand-offs oldest first, children as `TaskDto`s ordered by priority then number, session links by `first_touched_at`), and `load_task_dto_in(&self, conn: &mut PgConnection, project_id, task_id)` for use inside mutation transactions (event payloads must reflect the uncommitted state).
- [ ] The loaders also work across projects for the dashboard: `load_task_dtos_any_project(&self, tasks: &[Task])`.
- [ ] `ARCHITECTURE.md` "Orchestrator internals" module tree gains the line `├── tracker/   task-tracker domain service: mutation context, state changes, leases, graph rules, escalation` in the same commit.
- [ ] `cargo sqlx prepare` run and `.sqlx/` committed.

## Implementation notes
- Files: `orchestrator/src/events/task_event.rs` (re-export from `events/mod.rs`), `orchestrator/src/tracker/mod.rs`, `orchestrator/src/tracker/dto.rs`, `orchestrator/src/repositories/tasks/dto.rs` (or the equivalent file in the existing `repositories/tasks` layout), `ARCHITECTURE.md`.
- `TaskEventKind` constants must equal the strings the models task defined (`created`, `updated`, `state_changed`, `claimed`, `released`, `escalated`, `blocked`, `unblocked`, `commented`, `dependency_added`, `dependency_removed`, `deleted`, `states_changed`); `Display`/`as_str()` return the same string used in `task_events.kind`.
- `TaskDto::from_parts(task: &Task, state_name: &str, depends_on: Vec<DependencyRef>, blocks: Vec<Uuid>, handoff: Option<HandoffDto>)` is the single assembly function.
- `depends_on` lists every outgoing edge with its kind; `blocks` lists incoming `blocks` edges only (task ids that have a `blocks` dependency on this one).
- The reads take no project lock (ADR 0021: reads do not lock); the `_in` variant only borrows a connection so callers inside a mutation can build payloads.
- `HandoffDto` copies the `task_handoffs` row field for field; this epic never inserts one.

## Edge cases
- A task whose `current_handoff_id` points at a deleted row (FK `SET NULL`) simply has `handoff: null`.
- `load_task_detail` with a per-project number that does not exist returns `None`, never a parse error.
- Children of a task are never nested further (one level), so `children[].children` does not exist on the DTO.
- Serialise `lease_since`, `closed_at` and other nullable timestamps as `null`, not omitted, so the frontend types stay exact; only `TaskEvent`'s optional sections are omitted.

## Testing
- Unit tests: `TaskActor` and `TaskEventKind` serde round-trips including the exact JSON `{"kind":"system"}`; a `deleted` event serialises without a `task` key; a `states_changed` event serialises with `"task_id":null` and a `states` array.
- Integration tests in `orchestrator/tests/tracker_dto.rs` on the migrated test pool: seed a project with default states, three tasks, a `blocks` and a `discovered_from` edge on the same pair, a comment and a `task_sessions` row; assert `load_task_dto` returns `state = "backlog"`, both `depends_on` entries with kinds, the `blocks` reverse list, `handoff: null`; `load_task_detail` returns comments, children and sessions in the documented order; `load_task_dtos` on 50 tasks issues a bounded number of queries (assert via `sqlx` logging or a query counter, or at least that it returns all 50 in priority-then-number order).
- `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Documentation
- `ARCHITECTURE.md` "Orchestrator internals": add the `tracker/` module line (the documented layout has no home for the tracker domain service; ADR not needed, no alternative rejected).

## Assumes from other epics
- "Database schema, models, repositories and test harness": the row types `Task`, `TaskState`, `TaskDependency`, `TaskComment`, `TaskHandoff`, `TaskSession`, `TaskEventRow`, `NewTaskEvent` and the kind string constants; `TaskRepository` with `find_task(project_id, TaskRef)`, `list_dependencies`, `list_dependants`, `list_comments`, `list_handoffs`, `list_children`, `list_task_sessions`; the test pool helper.