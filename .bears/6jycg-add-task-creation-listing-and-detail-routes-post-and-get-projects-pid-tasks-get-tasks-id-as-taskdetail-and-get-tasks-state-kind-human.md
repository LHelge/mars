---
id: "6jycg"
title: "Add task creation, listing and detail routes: POST and GET /projects/{pid}/tasks, GET /tasks/{id} as TaskDetail, and GET /tasks?state_kind=human"
status: done
priority: P1
created: "2026-09-16T20:42:53.671044036Z"
updated: "2026-09-19T11:33:00.010575446Z"
tags:
  - orchestrator
  - tracker
depends_on:
  - h8qkt
parent: "5h3y4"
attempts: 1
---

## Summary
Deliver the read side and creation of tasks over REST: the filtered project list, the cross-project dashboard list by state kind, the `TaskDetail` view, and `POST` creation with initial state, parent and `blocks` dependencies, all validated under the project lock and emitting `created` (plus `dependency_added` and any `blocked` flips) in one transaction. The domain function `tracker::tasks::create_task` is shared with the MCP `create_task` tool (provenance is a later task).

## Documents
- `SPEC.md` "Tasks": `GET /tasks?state_kind=human` → `Task[]` across all projects; `GET /projects/{pid}/tasks?state=&label=&priority=&parent=&held=` → `Task[]` ordered by priority then number; `POST /projects/{pid}/tasks {title, description?, state?, priority?, labels?, parent_id?, depends_on?: id[]}` → 201 `Task`, 400 for an unknown state; `GET /projects/{pid}/tasks/{id}` → `TaskDetail`; `{id}` accepts UUID or per-project number; `priority` 0–3 default 2; one-level parent rules → 400; parent must be a different task in the same project; `number` from the project's counter, never reused; `depends_on` creates `blocks` edges, 409 on cycle.
- `SPEC.md` "TaskEvent" (`created`, `dependency_added`, `blocked`), "Frontend" → "Dashboard" (`GET /tasks?state_kind=human`).
- `docs/data-model.md` `tasks` (default state = lowest-position queue state; `created_by_user_id`), `task_dependencies`.
- `ARCHITECTURE.md` "Task tracker" → "Parents" (validate both ends on creation under the project lock), "Blocked is stored".
- ADRs 0021, 0023.

## Acceptance criteria
- [ ] `orchestrator/src/tracker/tasks.rs` exposes `pub async fn create_task(m: &mut TrackerMutation, input: CreateTaskInput) -> Result<TaskDto>` with `CreateTaskInput { title, description: Option<String>, state: Option<String>, priority: Option<i16>, labels: Vec<String>, parent: Option<Uuid>, depends_on: Vec<Uuid>, created_by: CreatedBy::User(Uuid) | CreatedBy::Session(Uuid) }`; it validates through the `Task` model, resolves `state` by name (`resolve_state`, 400 message listing valid names) or the default queue state, inserts the row with the allocated number, inserts each `depends_on` as a `blocks` edge after `check_no_cycle` (409) and the same-project check (400), recomputes `blocked` for the new task and its parent, emits `created` with the full task, then one `dependency_added` per edge (payload: the task after all edges), then any `blocked` flips; `touch_actor` for session creators.
- [ ] Event payload for `created` carries the task including its `depends_on` list (load the DTO after the edges are inserted); `dependency_added` events follow it in `depends_on` order.
- [ ] `orchestrator/src/routes/tasks.rs` exports `routes() -> Router<AppState>` with `POST /projects/{pid}/tasks` → 201 `Task`; `GET /projects/{pid}/tasks` with query `state` (name; unknown name → 400 with the valid-names message), `label`, `priority` (0–3, else 400), `parent` (UUID or number; unknown → 404), `held` (`true|false`) → 200 `Task[]`; `GET /projects/{pid}/tasks/{id}` → 200 `TaskDetail`, 404 unknown; `GET /tasks?state_kind=` → 200 `Task[]` across projects, 400 for a kind other than `queue|human|terminal`, `state_kind` required (400 when missing).
- [ ] `TaskRef::parse` is used for every `{id}` and `parent` path/query value (all-digits → number, else UUID, else 404).
- [ ] Unknown `parent_id` in the body → 404? No: 400 `parent must be a top-level task of the same project` (the repository check), consistent with the one-level rules; unknown `depends_on` id → 400 `dependency must reference tasks of the same project`.
- [ ] `created_by_user_id` is the caller; `assignee_user_id` is not settable on create (only through `PUT`).
- [ ] Router mounted; `cargo sqlx prepare` run for the list filters.

## Implementation notes
- Files: `orchestrator/src/tracker/tasks.rs`, `orchestrator/src/routes/tasks.rs`, `orchestrator/src/routes/mod.rs`.
- Request DTO private to the route: `CreateTaskRequest { title: String, description: Option<String>, state: Option<String>, priority: Option<i16>, labels: Option<Vec<String>>, parent_id: Option<Uuid>, depends_on: Option<Vec<String>> }` (`depends_on` entries accept UUID or number strings, resolved through `TaskRef` inside the mutation so numbers are looked up under the lock).
- List reads are lock-free: `TaskRepository::list_tasks` + `load_task_dtos`; the dashboard list uses `list_tasks_by_state_kind` + `load_task_dtos_any_project`.
- Dependency insertion order: cycle check → `insert_dependency` → next edge; a failure anywhere rolls back the whole creation (no task, no edges, no events).
- A task created directly in a terminal state gets `closed_at = NOW()`; created in the human state gets no `escalated` event (creation is `created` only).

## Edge cases
- `depends_on` containing the new task's own number cannot happen (number is allocated inside); a duplicate id in `depends_on` → `Conflict("dependency already exists")` from the PK, surfaced as 409; deduplicate beforehand to be lenient (document which).
- `labels` normalisation (trim, dedupe, pattern) is the model's; invalid → 400 with the model message.
- `parent_id` naming a task that has a parent → 400 `a task with a parent cannot receive children`.
- Creating a child of a terminal parent is allowed; the parent becomes `blocked` (harmless) and is never reopened.
- `GET /projects/{pid}/tasks?state=done` includes closed tasks; the board shows all columns.
- Large projects: the list endpoint has no pagination in v1 (documented in `SPEC.md` as `Task[]`); keep the batch loaders bounded to four queries.

## Testing
- Integration tests in `orchestrator/tests/tasks_api.rs` through `TestApp`: create with defaults → 201, `number = 1`, `state = "backlog"`, `priority = 2`, `blocked = false`, one `created` event with actor `user`; second task `number = 2`; explicit `state: "ready"`, unknown state → 400 with the valid-names message; `priority: 4` → 400; bad label → 400; `depends_on` on an open task → `blocked = true`, events `created`, `dependency_added`, `blocked` in that order; `depends_on` forming a cycle → 409 and no task row; `parent_id` of a task with a parent → 400; `parent_id` from another project → 400; list filters (`state`, `label`, `priority`, `parent` by number, `held=false`) and ordering by priority then number; `GET .../tasks/2` and `GET .../tasks/<uuid>` both return the detail with `comments`, `handoffs`, `children`, `sessions`; `GET /tasks?state_kind=human` returns tasks from two projects and nothing from queue states; missing `state_kind` → 400; unauthenticated → 401; unknown project → 404.
- `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Database schema, models, repositories and test harness": `insert_task` with number allocation and parent checks, `insert_dependency`, `list_tasks` with `TaskFilter`, `list_tasks_by_state_kind`, `TaskRef`.
- "Authentication, users, invites and email": `CurrentUser` extractor, `TestApp` login helpers.