---
id: cws3a
title: "Implement claims: atomic claim for a profile's served states, claim-for-launch over all non-terminal states, the ready list, and the user release route"
status: done
priority: P1
created: "2026-09-16T20:44:45.993609259Z"
updated: "2026-09-19T12:17:50.414038835Z"
tags:
  - orchestrator
  - tracker
  - sessions
depends_on:
  - "6jycg"
parent: "5h3y4"
attempts: 1
---

## Summary
Implement the lease acquisition side of the tracker: the atomic claim statement scoped to a profile's served states (used by the MCP `claim` tool), `claim_for_launch` over every non-terminal state (used by `POST /projects/{pid}/sessions` with `task_id`), the claimable-task read behind the MCP `ready` tool, and the user's `POST /tasks/{id}/release` which clears a lease without escalating. Claims increment `attempts`, emit `claimed`, and link the session to the task in the same transaction; exactly one of two concurrent claims wins.

## Documents
- `docs/data-model.md` `tasks` (the claim statement verbatim: `UPDATE tasks SET lease_holder_session_id = $2, lease_since = NOW(), attempts = attempts + 1, updated_at = NOW() WHERE id = $1 AND project_id = $3 AND state_id = ANY($4) AND NOT blocked AND lease_holder_session_id IS NULL RETURNING *`; zero rows = claim lost; `$4` = the profile's served states, or every non-terminal state for a UI launch), `tasks_claimable_idx`, `task_sessions` (upsert on claim and on launch-for-task), "A **release** clears the lease and keeps the state ... A release by a user never escalates."
- `ARCHITECTURE.md` "Task tracker" → "The lease is the worker" (claim succeeds for exactly one session; users can release anything), "Launching a session for a task" (launch ignores served states; task must be unheld, unblocked, non-terminal; the human state qualifies), "Attempts and escalation" (a claim increments `attempts`).
- `SPEC.md` "Tasks": `POST /projects/{pid}/tasks/{id}/release` → `Task` (clears the lease, keeps the state; 409 if nobody holds it); "TaskEvent" (`claimed`; `released` with `reason: "user"`); "MCP tool contracts" → `ready` (`TaskSummary = { id, number, title, state, priority, labels, description_excerpt, attempts, depends_on_count }`, served states only, not blocked, unheld, ordered by priority then number, empty for a profile serving nothing), `claim` (`conflict` "task is not claimable" on zero rows; "task is not in a state this profile serves" when outside served states; returned task includes its hand-off); "Sessions" (`POST .../sessions` 409 if `task_id` names a task that is held, blocked or in a terminal state).
- ADRs 0009 (mechanism), 0016, 0021, 0030.

## Acceptance criteria
- [ ] `TaskRepository::claim(conn, project_id, task_id, session_id, state_ids: &[Uuid]) -> Result<Option<Task>>` executes the statement above verbatim (`Option::None` for zero rows), plus `list_claimable(project_id, state_ids, limit) -> Result<Vec<TaskSummaryRow>>`: one scoped `query_as!` using the claimable index, joining `task_states` for the state name and counting outgoing `task_dependencies` of every kind with a lateral aggregate, ordered by `priority, number` and limited by `$3`. This task owns the repository query; `y2nd8` reuses it through `ready_summaries`.
- [ ] `tracker::leases::claim_for_profile(m, task: &Task, session_id, served_state_ids: &[Uuid]) -> Result<TaskDto>`: if `task.state_id` is not in `served_state_ids` → `Error::Conflict("task is not in a state this profile serves")`; else run `claim`; `None` → `Error::Conflict("task is not claimable")`; on success emit `claimed` (actor `session`) with the task after the claim and `touch(task.id, session_id)`.
- [ ] `tracker::leases::claim_for_launch(m, task: &Task, session_id) -> Result<TaskDto>`: `state_ids` = every state of the project whose kind is not `terminal` (queue and human); `None` → `Error::Conflict("task is not claimable")`; emits `claimed` with the mutation's actor (the launching user) and `touch(task.id, session_id)`. The sessions epic calls this inside its session-insert transaction, which must be a `TrackerMutation` (project row locked before the session row is inserted).
- [ ] `tracker::leases::ready_summaries(pool, project_id, served_state_ids, limit) -> Result<Vec<TaskSummary>>` is a lock-free read returning `TaskSummary` DTOs with `description_excerpt` = trim the description, replace each newline sequence (CRLF, LF or CR) with one space, then take the first 200 Unicode scalar values and trim trailing whitespace, without an ellipsis and `depends_on_count` = number of outgoing edges of any kind; no events, no touches (ADR 0030).
- [ ] `tracker::leases::release_by_user(m, task: &Task) -> Result<TaskDto>`: no holder → `Error::Conflict("task is not held")`; else clear `lease_holder_session_id`/`lease_since` (keep `attempts`, state, `closed_at`), emit `released` with `reason: "user"` and the task; no comment, no escalation, never moves the task.
- [ ] `POST /projects/{pid}/tasks/{id}/release` → 200 `Task`, 409 `task is not held`, 404 unknown task.
- [ ] `cargo sqlx prepare` run and `.sqlx/` committed.

## Implementation notes
- Files: `orchestrator/src/tracker/leases.rs`, `orchestrator/src/tracker/dto.rs` (`TaskSummary`), `orchestrator/src/repositories/tasks/` (claim, claimable list, counts), `orchestrator/src/routes/tasks.rs` (release route).
- The claim never checks the state kind in Rust: `state_ids` carries the policy (served states or all non-terminal), the SQL does the rest, and the project lock held by the `TrackerMutation` serialises competing claims within a project; the `WHERE ... lease_holder_session_id IS NULL` clause is still what makes the statement safe if a caller ever bypasses the lock.
- `served_state_ids` come from `TaskRepository::list_profile_states(profile_id)` (schema epic); the MCP epic passes them from its `SessionContext.profile`.
- A user release keeps `attempts` so that a subsequent agent release can still escalate correctly (only a state change resets it).

## Edge cases
- Claim of a task in the human state by a profile: never served (only queue states can be served), so it is `task is not in a state this profile serves`; by launch: allowed.
- Claim of a blocked task: zero rows → `task is not claimable` even when the state matches.
- Claim of a terminal task by launch: zero rows → 409.
- `ready` defaults to 20; an explicit limit must be an integer from 1 through 100 inclusive. Reject out-of-range or non-integer values with `invalid_argument` at the MCP boundary; never clamp. `ready_summaries` validates the same range and returns a validation error for invalid internal callers. Test default, 1, 100, 0, 101 and fractional input (fractional input at the MCP boundary).
- `release_by_user` on a task held by a session that is already `done` (reaper not yet run): allowed; the reaper later finds nothing to release.
- A session claiming a task it already holds: zero rows (holder not null) → `task is not claimable`; the MCP epic may special-case this message if desired, but the tracker returns the conflict.

## Testing
- Integration tests in `orchestrator/tests/tracker_leases.rs` (direct mutation use with sessions inserted through `SessionRepository`) and `tasks_api.rs` (release route): `claim_for_profile` on a `ready` task with served `[ready]` → holder set, `lease_since` set, `attempts = 1`, one `claimed` event with actor `session`, one `task_sessions` row; same task again from another session → `task is not claimable`; task in `backlog` with served `[ready]` → `task is not in a state this profile serves`; blocked task → not claimable; **two claims spawned concurrently on one task from two sessions → exactly one `Ok` and one `Conflict`** (the epic's acceptance criterion); `claim_for_launch` on a `needs_human` task → success, on a `done` task → conflict, on a held task → conflict; `ready_summaries` returns only unheld, unblocked tasks in served states ordered by priority then number, with `depends_on_count` and a truncated excerpt, and adds no `task_events` or `task_sessions` rows; `POST .../release` on a held task → 200 with `lease_holder_session_id: null`, `attempts` unchanged, state unchanged, one `released` event with `reason: "user"`; on an unheld task → 409 `task is not held`.
- `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Database schema, models, repositories and test harness": `SessionRepository` insert for test sessions, `list_profile_states`, `set_task_state_fields`.
- "Session lifecycle": calls `claim_for_launch` inside its session-insert transaction (`md2zq`); until then only tests exercise it.
- "MCP server and agent tools": calls `claim_for_profile` and `ready_summaries`.