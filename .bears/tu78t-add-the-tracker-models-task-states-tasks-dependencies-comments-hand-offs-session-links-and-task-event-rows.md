---
id: tu78t
title: "Add the tracker models: task states, tasks, dependencies, comments, hand-offs, session links and task-event rows"
status: open
priority: P1
created: "2026-09-16T20:27:24.947302370Z"
updated: "2026-09-16T20:51:51.545088753Z"
tags:
  - orchestrator
  - core
  - tracker
depends_on:
  - sywed
parent: p5tsd
---

## Summary
Define the domain types for the task tracker under `models/` with their validation rules and the `TaskError` enum: `TaskState`, `Task`, `TaskDependency`, `TaskComment`, `TaskHandoff`, `TaskSession` and the `task_events` row type, plus the constant default state set. Models hold validation only, never SQL; the repository task builds on these types.

## Documents
- `docs/data-model.md` "Enums" (`task_state_kind`, `task_dependency_kind`), "Tasks" (all subsections: `task_states` incl. default set, `profile_states`, `tasks`, `task_dependencies`, `task_comments`, `task_handoffs`, `task_sessions`, `task_events`).
- `SPEC.md` "Task states", "Tasks", "Code hand-offs and review" (validation rules and DTO field names), "TaskEvent" (kind strings).
- `ARCHITECTURE.md` "Orchestrator internals" (`Error` enum has a `#[from]` variant per model error).

## Acceptance criteria
- [ ] `TaskStateKind { Queue, Human, Terminal }` and `TaskDependencyKind { Blocks, DiscoveredFrom, Related }` derive `sqlx::Type` with `#[sqlx(type_name = "task_state_kind", rename_all = "snake_case")]` (resp. `task_dependency_kind`) and `serde` with `rename_all = "snake_case"`.
- [ ] `TaskStateName::parse(&str)` accepts 1–32 characters matching `^[a-z0-9][a-z0-9_-]*$` and rejects everything else with `TaskError::InvalidStateName`.
- [ ] `Task` validation: title trimmed, 1–200 chars (`TaskError::InvalidTitle`); priority 0–3 (`TaskError::InvalidPriority`); every label 1–32 chars matching the state-name pattern, duplicates removed (`TaskError::InvalidLabel`); `parent_id != id` (`TaskError::SelfParent`).
- [ ] `NewTaskComment::validate`: body non-empty after trim (`TaskError::EmptyComment`); when `system` is false exactly one of `author_user_id` / `author_session_id` is set, when true neither (`TaskError::InvalidCommentAuthor`).
- [ ] `ReviewStatus { Unreviewed, Approved, ChangesRequested }` serialises as `unreviewed` / `approved` / `changes_requested` (TEXT column, not an enum type).
- [ ] `NewTaskHandoff::validate`: `commit` is a full lowercase hex object id (40 or 64 chars, `TaskError::InvalidCommit`); exactly one of `created_by_user_id` / `created_by_session_id` (`TaskError::InvalidHandoffActor`); if `review_status` is `Unreviewed` then no reviewer and no `reviewed_at`, otherwise exactly one reviewer and `reviewed_at` set (`TaskError::InvalidReview`); `source_branch` non-empty.
- [ ] `DEFAULT_TASK_STATES: [(&str, TaskStateKind, i32); 7]` equals `backlog/queue/0, ready/queue/1, review/queue/2, merge/queue/3, needs_human/human/4, done/terminal/5, cancelled/terminal/6`.
- [ ] `TaskEventRow { project_id, seq, ts, task_id: Option<Uuid>, kind: String, payload: serde_json::Value }` and `NewTaskEvent { ts, task_id, kind, payload }` exist; the kind strings from `SPEC.md` "TaskEvent" are available as constants (`created`, `updated`, `state_changed`, `claimed`, `released`, `escalated`, `blocked`, `unblocked`, `commented`, `dependency_added`, `dependency_removed`, `deleted`, `states_changed`).
- [ ] `TaskError` is added as a `#[from]` variant of `prelude::Error` mapping to HTTP 400.
- [ ] Every rule above has a unit test.

## Implementation notes
- Files: `orchestrator/src/models/task_state.rs`, `models/task.rs`, `models/task_dependency.rs`, `models/task_comment.rs`, `models/task_handoff.rs`, `models/task_session.rs`, `models/task_event.rs`, re-exported from `models/mod.rs`; `prelude/error.rs` gains `#[error(transparent)] Task(#[from] TaskError)`.
- Row structs mirror the columns exactly (`Task` has `id, project_id, number: i32, title, description, state_id, priority: i16, blocked, labels: Vec<String>, parent_id, assignee_user_id, lease_holder_session_id, lease_since, attempts: i16, needs_human_reason, current_handoff_id, created_by_user_id, created_by_session_id, created_at, updated_at, closed_at`). `TaskState { id, project_id, name, kind, position: i32, created_at }`. `TaskDependency { task_id, depends_on_task_id, kind }`. `TaskComment { id, task_id, author_user_id, author_session_id, system, body, created_at }`. `TaskHandoff` with every column from `docs/data-model.md`. `TaskSession { task_id, session_id, first_touched_at, last_touched_at }`.
- Input structs (`NewTask`, `NewTaskState`, `NewTaskComment`, `NewTaskHandoff`) carry the caller-supplied fields; ids are generated with `Uuid::new_v4()` by the caller or the constructor so the id is known before insert.
- `TaskError` is a `thiserror` enum with a human-readable message per variant; the message is what the API returns in `{status, error}`.
- Keep the state-name pattern in one function (`is_state_name(&str)`) reused for labels.
- The API-facing `Task` DTO (with `state` name, `depends_on`, `blocks`, `handoff`) is assembled by the tracker epic; do not add it here.

## Edge cases
- Priority arrives as `i16` from the database and as JSON number from the API; validate in one place (`Priority::try_from(i16)` or a validated newtype).
- Labels: reject empty strings, reject > 32 chars, reject uppercase; dedupe preserving first occurrence.
- Commit ids: reject uppercase hex and abbreviated ids.
- `TaskStateKind` is immutable after creation; the model exposes no setter, the repository refuses updates.

## Testing
- Unit tests in each model file (`#[cfg(test)] mod tests`) covering every accept/reject rule listed above, including boundary lengths (1, 32, 33 for names; 200, 201 for titles; 0, 3, 4 and -1 for priority).
- `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` must pass.

## Documentation
- None: implements the documented contract as written.

## Assumes from other epics
- "Repository scaffolding, tooling and CI": `prelude::Error` with the `IntoResponse` mapping and `models/mod.rs` exist; `serde`, `thiserror`, `uuid`, `chrono`, `sqlx` are dependencies.