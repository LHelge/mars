---
id: htyzj
title: "Link tasks and sessions: GET /sessions/{id}/tasks and installing release_leases_for_session in the session end and failure hooks"
status: done
priority: P1
created: "2026-09-16T20:46:59.309167546Z"
updated: "2026-09-19T13:27:07.135018810Z"
tags:
  - orchestrator
  - tracker
  - sessions
depends_on:
  - "2sjtz"
  - f3bjb
parent: "5h3y4"
attempts: 1
---

## Summary
Close the loop between sessions and the tracker: expose the tasks a session touched at `GET /api/sessions/{id}/tasks`, and install the tracker's `release_leases_for_session` (followed by escalation email) as the implementation of the session lifecycle's `on_session_ended` and `on_session_failed` hooks, so that ending a session from the UI, an ephemeral session finishing, a launch failing in `creating` and recovery marking a session `failed` all release its leases immediately. The stuck-task reaper (Background jobs epic) remains the backstop and calls the same function.

## Documents
- `SPEC.md` "Sessions": `GET /sessions/{id}/tasks` → `Task[]` touched by this session; "Frontend" → "Task board" (the session view shows the task the session was launched for and the tasks it touched in a side panel).
- `ARCHITECTURE.md` "Task tracker" → "Liveness comes from the session, not from tool calls" (dead means `done` or `failed`; ending a session from the UI releases its leases immediately; the reaper is the backstop), "Launching a session for a task" (a launch that fails in `creating` releases the task when the session becomes `failed`), "Session lifecycle" state table (`done`/`failed`: held tasks are released).
- `docs/data-model.md` `task_sessions` (`task_sessions_session_idx`), `tasks_lease_holder_idx`.
- ADRs 0016, 0021.

## Acceptance criteria
- [ ] `GET /api/sessions/{id}/tasks` → 200 `Task[]` (full `Task` DTOs) for every `task_sessions` row of the session ordered by `last_touched_at DESC`, loaded through `load_task_dtos` with the session's project; 404 for an unknown session; 401 unauthenticated. Lives in `orchestrator/src/routes/sessions.rs` if that module exists, otherwise in `routes/tasks.rs` with a comment; exactly one implementation across epics (see the sessions epic's `md2zq`, which lists the same route: `htyzj` owns the route and `md2zq` depends on and reuses it).
- [ ] `tracker::hooks::on_session_dead(state: &AppState, session_id, reason: ReleaseReason)` calls `release_leases_for_session(pool, session_id, reason)` and then `escalation::notify(state, escalations)`; errors are logged with `session_id = %id` and swallowed (a session transition must not fail because the tracker did).
- [ ] At startup (`main.rs` / `AppState` construction) the sessions epic's `on_session_ended` and `on_session_failed` hooks are set to call `on_session_dead` with `SessionEnded`; the `failed` hook passes `Stalled` when `sessions.error == "stalled"`, `SessionEnded` otherwise.
- [ ] The hook runs after the session-state transaction has committed (never inside it: the project lock must precede session locks, so the tracker mutation is a separate, later transaction).
- [ ] `cargo sqlx prepare` run if queries were added.

## Implementation notes
- Files: `orchestrator/src/tracker/hooks.rs`, `orchestrator/src/routes/sessions.rs` or `routes/tasks.rs`, `orchestrator/src/main.rs` (hook installation), `orchestrator/src/prelude/state.rs` if the hook fields live there.
- `TaskRepository::list_tasks_for_session(session_id)` exists from the schema epic; add ordering by `last_touched_at DESC` if it lacks it.
- If the sessions epic has not yet defined the hook fields, define them here as `pub type SessionHook = Arc<dyn Fn(Uuid, Option<String>) -> BoxFuture<'static, ()> + Send + Sync>` on `AppState` with no-op defaults, and note it in the commit so the sessions epic reuses them.
- Idempotency: calling the hook twice for one session releases nothing the second time (the locked re-read finds no held tasks).

## Edge cases
- Session deleted (row gone) before the hook runs: `release_leases_for_session` finds no project → log at `debug` and return; the FK already nulled `lease_holder_session_id`... but `lease_since` must also be null: add a defensive `UPDATE tasks SET lease_since = NULL WHERE lease_holder_session_id IS NULL AND lease_since IS NOT NULL` in the same function? No: the `CHECK ((lease_holder_session_id IS NULL) = (lease_since IS NULL))` makes a `SET NULL` cascade fail on a held task, so the sessions epic's `delete` only runs on `done`/`failed` sessions whose leases were released; document this coupling in the hook's doc comment.
- A session that ends while a `TrackerMutation` for the same project is open: the hook waits for the project lock; fine.
- `GET /sessions/{id}/tasks` for a session whose touched task was deleted: the `task_sessions` row cascaded; the task simply is not listed.

## Testing
- Integration tests in `orchestrator/tests/session_tasks_api.rs` through `TestApp`: a session (inserted through the repository or launched with the mock engine) claims two tasks and comments on a third → `GET /sessions/{id}/tasks` returns three tasks ordered by `last_touched_at DESC`; unknown session → 404; calling the installed `on_session_ended` hook (or ending the session through the sessions API when available) releases both held tasks with `released` events `reason: "session_ended"`, system comments, and leaves the commented-only task untouched; a task at `max_attempts` escalates and the mock email client captured one message; calling the hook again produces no new events; the `failed` hook with `error = "stalled"` yields `reason: "stalled"`.
- `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Session lifecycle": the `on_session_ended`/`on_session_failed` hook fields on `AppState` and the session transitions that invoke them (`bppkk`, `md2zq`, recovery); until they exist, this task defines no-op hook fields as described.
- "Authentication, users, invites and email": `CurrentUser`, `TestApp` helpers.