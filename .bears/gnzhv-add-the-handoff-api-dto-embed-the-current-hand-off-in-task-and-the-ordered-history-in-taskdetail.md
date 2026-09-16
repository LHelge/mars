---
id: gnzhv
title: Add the Handoff API DTO, embed the current hand-off in Task and the ordered history in TaskDetail
status: open
priority: P2
created: "2026-09-16T20:40:29.082079219Z"
updated: "2026-09-16T20:40:29.082079219Z"
tags:
  - orchestrator
  - tracker
parent: xjaah
---

## Summary
Provide the `Handoff` response shape from `SPEC.md` and make sure every `Task` payload (REST responses, `TaskEvent.task`, MCP `claim`/`update` outputs) carries `handoff: Handoff | null` resolved from `tasks.current_handoff_id`, and that `TaskDetail.handoffs` lists the task's full history oldest first. The tracker epic assembles the `Task` and `TaskDetail` DTOs; this task owns the hand-off part of them and fills any gap it left, without duplicating the assembly.

## Documents
- `SPEC.md` "Code hand-offs and review" (`Handoff = { id, task_id, source_session_id, source_branch, commit, comment_id, review_status, reviewed_by_user_id, reviewed_by_session_id, reviewed_at, created_by_user_id, created_by_session_id, created_at }`; actor and source-session ids may become null after deletion while branch, commit and review timestamp remain; every state event carries the full task including its current hand-off).
- `SPEC.md` "Tasks" (`Task.handoff: Handoff | null`; `TaskDetail = Task & { comments, handoffs: Handoff[], children, sessions }`, hand-offs ordered oldest first).
- `docs/data-model.md` `task_handoffs` (columns; the current record is selected through `tasks.current_handoff_id`, not timestamps; index `task_handoffs_task_idx (task_id, created_at)`).

## Acceptance criteria
- [ ] `models::task_handoff::HandoffDto` (or `Handoff` in the routes' DTO module, following the tracker epic's naming) serialises exactly the thirteen fields above in `snake_case`, `review_status` as `unreviewed` / `approved` / `changes_requested`, timestamps RFC 3339, nullable ids as JSON `null`.
- [ ] `From<TaskHandoff> for HandoffDto` exists and is the only conversion path.
- [ ] The tracker epic's `Task` DTO builder resolves `handoff` from `current_handoff_id` with `TaskRepository::find_handoff(project_id, id)` (single query for one task; a batched `find_handoffs_by_ids(project_id, &[Uuid])` for list endpoints so `GET /projects/{pid}/tasks` does not issue one query per task).
- [ ] `TaskDetail.handoffs` comes from `TaskRepository::list_handoffs(project_id, task_id)` ordered by `created_at ASC, id ASC` (ties broken deterministically).
- [ ] `GET /projects/{pid}/tasks/{id}` for a task with three hand-offs returns them oldest first and `handoff` equals the last published (which is not necessarily the newest by timestamp once forwards exist: it is the one `current_handoff_id` names).
- [ ] `.sqlx/` refreshed if a new query is added.

## Implementation notes
- Files: `orchestrator/src/models/task_handoff.rs` (DTO), the tracker epic's DTO assembly module (likely `orchestrator/src/routes/tasks.rs` or `orchestrator/src/tracker/dto.rs`), `orchestrator/src/repositories/tasks.rs` (`find_handoffs_by_ids`).
- If the tracker epic already embedded `handoff` and `handoffs`, this task reduces to verifying the field set against `SPEC.md`, adding the batched lookup and the tests below; do not create a second DTO.
- The DTO is also what `TaskEvent.task` embeds, so the event payload assembly must use the same builder.

## Edge cases
- `current_handoff_id` pointing at a row of another task cannot happen (repository check on insert), but the builder must not panic if `find_handoff` returns `None`: log `warn!(task_id = %id)` and emit `handoff: null`.
- Source session deleted: `source_session_id: null`, `source_branch` and `commit` present.
- Reviewer user deleted after approval: `review_status: "approved"`, `reviewed_by_user_id: null`, `reviewed_at` present.

## Testing
- Integration test in `orchestrator/tests/tasks_api.rs` (or the tracker epic's file): seed a task with hand-off rows through `TaskRepository::insert_handoff` inside `begin_mutation`, set `current_handoff_id`, then assert `GET .../tasks/{id}` has `handoff.id` equal to the current pointer and `handoffs` in insertion order; `GET .../tasks` embeds `handoff` on the same task and `null` on a task without one; delete the source session row and re-read to assert the null/retained fields.
- `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Task tracker: states, tasks, leases, dependencies and events": the `Task` / `TaskDetail` DTO builders and `GET` task routes.
- "Database schema, models, repositories and test harness": `TaskHandoff` row type, `find_handoff`, `list_handoffs`, `insert_handoff`, `set_task_state_fields`.