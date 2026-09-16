---
id: bgynw
title: "Implement the stuck-task reaper: release leases held by done or failed sessions with session_ended or stalled, escalate at max_attempts, system comments and events"
status: open
priority: P1
created: "2026-09-16T20:44:06.827099157Z"
updated: "2026-09-16T20:44:06.827099157Z"
tags:
  - orchestrator
  - cron
  - tracker
depends_on:
  - yb2ny
parent: cxmar
---

## Summary
Fill in `CronService::stuck_task_reaper`: every minute find every task whose lease holder session is `done` or `failed`, and release those leases through the tracker's project-locked release primitive with reason `stalled` (holder failed with `error = stalled`) or `session_ended` (any other dead holder). A release that finds `attempts` at the project's `max_attempts` escalates the task to the human state instead, with `needs_human_reason`, an `escalated` event, a system comment and the escalation email. This is the backstop behind the immediate release performed by the `on_session_ended` hook.

## Documents
- `ARCHITECTURE.md` "Background jobs" (stuck-task reaper row: `1 min`, "Release tasks held by `done` or `failed` sessions, escalating those at the attempt limit; write the system comment and emit `TaskEvent`s")
- `ARCHITECTURE.md` "Task tracker" -> "Liveness comes from the session, not from tool calls" (alive = `creating`, `running`, `parked`; dead = `done`, `failed`; reasons `session_ended` / `stalled`; system comment; "Ending a session from the UI releases its leases immediately; the reaper is the backstop"), "Attempts and escalation" (`attempts` at `max_attempts` -> human state, reason recorded, `escalated` event), "Notification" (every move into the human state by the reaper emails the assignee or all admins honouring `notify_email`), "One mutation at a time per project" (project row lock first; email outside the transaction; helpers share the transaction)
- `SPEC.md` "TaskEvent" (`released` with `reason: "session_ended" | "stalled"`; `escalated` with `from`/`to` and free-text `reason`; `commented`; actor `{ kind: "system" }`), "Tasks" ("agent and reaper releases escalate to the `human` state once `attempts` reaches the project's `max_attempts`")
- `docs/data-model.md` `tasks` (`lease_holder_session_id`, `lease_since`, `attempts`, `needs_human_reason`, `tasks_lease_holder_idx` "for the stuck-task reaper"; release paragraph), `task_comments` (`system = TRUE`, both author columns NULL), `task_events`, "Tracker mutation transactions" (background jobs use the project-locked transaction), `projects.max_attempts`, `sessions.state`, `sessions.error`
- ADRs 0016, 0021, 0028, 0030

## Acceptance criteria
- [ ] `TaskRepository::list_dead_lease_holders() -> Result<Vec<DeadHolder { project_id: Uuid, session_id: Uuid, session_state: SessionState, session_error: Option<String>, held: i64 }>>` runs `SELECT t.project_id, s.id, s.state, s.error, COUNT(*) FROM tasks t JOIN sessions s ON s.id = t.lease_holder_session_id WHERE s.state IN ('done', 'failed') GROUP BY t.project_id, s.id, s.state, s.error ORDER BY t.project_id, s.id` (uses `tasks_lease_holder_idx`), outside any transaction.
- [ ] `cron/stuck_tasks.rs`: `impl CronService { pub async fn stuck_task_reaper(&self, now: DateTime<Utc>) -> Result<JobReport> }` maps each holder to `ReleaseReason::Stalled` when `session_state == Failed && session_error.as_deref() == Some("stalled")`, else `ReleaseReason::SessionEnded`, and calls the tracker primitive `tracker::release_leases_for_session(&self.state, session_id, reason).await -> Result<ReleaseOutcome { released: Vec<Uuid>, escalated: Vec<Uuid> }>` once per dead session. `items` counts released plus escalated tasks; a primitive error counts `failures += 1`, is logged `error!(session_id = %sid, project_id = %pid, error = %e, "lease release failed")`, and the loop continues.
- [ ] The primitive (owned by the tracker epic; extend it here only if it does not yet cover these points) does, in one transaction per session: lock the project row, re-read the tasks still held by that session, and for each: if `attempts >= projects.max_attempts` -> set `state_id` to the project's `human` state, clear the lease, reset `attempts` to 0, set `needs_human_reason = "<attempts> attempts exhausted; last holder session <sid> <ended|stalled>"`, insert a system comment `Escalated to <human state name>: <attempts> attempts exhausted; last holder session <sid> <ended|stalled>.`, emit `escalated { from, to, reason: <needs_human_reason>, actor: system }` and `commented`; otherwise -> clear the lease (`lease_holder_session_id = NULL, lease_since = NULL`), keep state and `attempts`, insert a system comment `Lease released: holder session <sid> ended.` or `Lease released: holder session <sid> stalled (no output within the profile's idle timeout).`, emit `released { reason: "session_ended" | "stalled", actor: system }` and `commented`. All `task_events` rows get successive `MAX(seq)+1` under the project lock and one `pg_notify('task_events', '<project_id>:<seq>')` inside the transaction. No `task_sessions` upsert for a system release (ADR 0030: the session did not act).
- [ ] After commit, for every escalated task the primitive sends the escalation email through `EmailClient` (assignee if set, otherwise every admin, skipping `notify_email = false`); the reaper sends nothing itself and never sends duplicates for a task escalated in an earlier tick.
- [ ] The job is idempotent: a second run against the same database does nothing and emits no events.
- [ ] `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes; `.sqlx/` refreshed.

## Implementation notes
- Files: `orchestrator/src/cron/stuck_tasks.rs`, `orchestrator/src/cron/mod.rs`, `orchestrator/src/repositories/task.rs` (the dead-holder query), the tracker service module that owns `release_leases_for_session` (extend, do not duplicate), `orchestrator/.sqlx/`.
- Lock order: only the database project row (via the primitive); the reaper acquires no git lock and no session row lock. One transaction per dead session, never one transaction for the whole sweep, so a large backlog cannot hold a project lock for long.
- Reason strings are the `TaskEvent.reason` values from `SPEC.md` verbatim: `session_ended`, `stalled`.
- The human state and `max_attempts` are read inside the locked transaction, not from the listing.
- Wording of the comments above is the contract; if the tracker epic already fixed different wording for the hook path, use that wording for both paths and update the documentation line below accordingly (one wording for hook and reaper).

## Edge cases
- Holder session deleted between listing and release: `ON DELETE SET NULL` already cleared the lease; the primitive finds no held tasks and returns empty outcomes (`skipped += 1`).
- Holder relaunched (`failed -> parked` by a retry) between listing and release: the primitive re-reads the session state under the project lock and releases nothing if the holder is alive again.
- Project without a human state cannot exist (`task_states_one_human_idx` plus deletion refusal); treat "human state missing" as an internal error for that project and continue.
- Task in a terminal state with a stale lease (should not happen: hand-offs clear leases): release it anyway without escalation.
- Many tasks held by one session: all handled in that session's single transaction, events in one batch, one notify.
- Email failure: logged by the primitive, never rolls back the committed escalation.

## Testing
- Integration test `orchestrator/tests/cron_stuck_tasks.rs` via `TestApp` (project with default states, `max_attempts = 3`, admin and an assignee user, mock email): insert sessions in `done`, `failed` with `error = "stalled"`, `parked`, `running`; claim one task per session through the tracker's claim (or direct SQL setting `lease_holder_session_id`, `lease_since`, `attempts`); run `app.cron().stuck_task_reaper(Utc::now())`: the done-held task has no lease, a `released` event with `reason = "session_ended"` and actor `system`, a `system = true` comment with the exact text; the stalled-held task likewise with `reason = "stalled"`; parked- and running-held tasks untouched; a task with `attempts = 3` held by a done session moves to `needs_human` with `attempts = 0`, `needs_human_reason` set, an `escalated` event with `from = "ready"`, `to = "needs_human"`, the system comment, and exactly one captured email to the assignee (a second escalated task without assignee emails every admin whose `notify_email` is true and skips the opted-out one); `LISTEN task_events` on a second connection sees one notification per affected project; second run returns `JobReport::default()` and adds no events; report counts match (`items = 4`).
- Command: `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- `ARCHITECTURE.md` "Task tracker" -> "Liveness comes from the session": append the exact system-comment wording for `session_ended`, `stalled` and escalation so hook, reaper and UI agree (only if the tracker epic has not already recorded it).

## Assumes from other epics
- "Task tracker: states, tasks, leases, dependencies and events": `release_leases_for_session(state, session_id, reason)` with project-locked release, escalation at `max_attempts`, system comments, `released`/`escalated`/`commented` events and the escalation email; `TaskRepository`, `Task` model, task-event append with notify.
- "Authentication, users, invites and email": `EmailClient` with the escalation message template and the mock capturing messages.
- "Session lifecycle: launcher, owner, recovery and sessions API": `SessionRepository::insert`/`transition` for seeding dead sessions in tests.