---
id: "2sjtz"
title: Implement agent and reaper releases with escalation at max_attempts, needs_human semantics, and release_leases_for_session
status: open
priority: P1
created: "2026-09-16T20:45:32.563509460Z"
updated: "2026-09-16T20:45:32.563509460Z"
tags:
  - orchestrator
  - tracker
  - sessions
  - cron
depends_on:
  - cuw5s
  - cws3a
  - kctj2
parent: "5h3y4"
---

## Summary
Implement the lease-ending paths that can escalate: an agent giving a task back (`release` tool), the orchestrator releasing every lease of a dead session (session end/failure hook now, stuck-task reaper later), and the `needs_human` hand-off including the already-in-human-state case. When `attempts` has reached the project's `max_attempts`, a non-user release moves the task to the human state with a system comment, `needs_human_reason` and an `escalated` event instead of leaving it in its queue. Escalations are recorded on the mutation so the caller can send the email after commit.

## Documents
- `ARCHITECTURE.md` "Task tracker" → "Attempts and escalation" (`attempts` counts claims since the last state change; a release by the agent or the reaper at `max_attempts` (default 3) goes to the human state with the reason recorded and an `escalated` event; three failing implementers produce one item in `needs_human` with three comments), "Liveness comes from the session, not from tool calls" (release with reason `session_ended` or `stalled`, system comment; ending a session from the UI releases its leases immediately; the reaper is the backstop), "Notification" (every move into the human state by `needs_human` or the reaper sends one email).
- `SPEC.md` "Tasks" ("A user `release` keeps the state and never escalates; agent and reaper releases escalate to the `human` state once `attempts` reaches the project's `max_attempts`"), "MCP tool contracts" → `release` (caller must hold the lease; reason recorded as a comment; at `max_attempts` moves to the human state with `needs_human_reason` set), `needs_human` (moves to the human state, sets `needs_human_reason`, releases the lease, resets `attempts`; requires holding the lease or the task being unheld; already in the human state: record the reason, explicitly release any held lease, preserve `attempts`, emit `commented`, `updated` and when applicable `released`, no new `escalated` event or email), "TaskEvent" (`released` reasons `given_back | session_ended | stalled | user`; `escalated` with `from`/`to` and free-text `reason`).
- `docs/data-model.md` `tasks` ("A **release** clears the lease and keeps the state. When the release comes from an agent or from the reaper and `attempts` has reached `projects.max_attempts`, the same transaction moves the task to the project's human state instead, sets `needs_human_reason`, and writes a system comment."), `task_comments` (`system = true`, no author), `projects.max_attempts`.
- ADRs 0016, 0021, 0030.

## Acceptance criteria
- [ ] `tracker::leases::release_by_agent(m, task: &Task, session_id, reason: &str) -> Result<TaskDto>`: holder must equal `session_id` (else `Error::Conflict("task is not held by this session")`); `add_comment(Session(session_id), reason)` → `commented`; then if `task.attempts >= project.max_attempts`: escalate (below) else clear the lease keeping state and `attempts`, emit `released` with `reason: "given_back"`; `touch(task.id, session_id)`.
- [ ] `tracker::leases::release_leases_for_session(pool, session_id, reason: ReleaseReason::SessionEnded | Stalled) -> Result<Vec<Escalation>>`: loads the session's project, opens one `TrackerMutation` with actor `system`, and for every task with `lease_holder_session_id = session_id` (re-read under the lock) writes a system comment `Lease released by the orchestrator: holder session <session_id> <ended|stalled>.` → `commented`, then escalates if `attempts >= max_attempts` else clears the lease with `released` `reason: "session_ended" | "stalled"`; no `touch` (actor is system; the link already exists from the claim); returns the escalations after commit for the caller to email. Zero held tasks → no mutation events, no rows.
- [ ] Escalation (private helper `escalate(m, task, reason_text)`): `reason_text` = `format!("attempt limit reached ({attempts}/{max_attempts}): {last reason}")` where the last reason is the agent's reason or `session <id> ended` / `session <id> stalled`; write a system comment `Escalated to <human state name> after <attempts> attempts. <reason_text>` → `commented`; call `change_state(task, human_state, StateEventKind::Escalated { reason: reason_text.clone() }, needs_human_reason: Some(reason_text))` which clears the lease, resets `attempts`, emits `escalated` with `from`/`to`; `m.record_escalation(...)` with the assignee from the task row.
- [ ] `tracker::leases::needs_human(m, task: &Task, session_id, reason: &str) -> Result<TaskDto>`: holder must be `session_id` or the task unheld (else `Conflict("task is held by another session")`); `add_comment(Session(session_id), reason)` → `commented`; if the task is not in the human state: `change_state(..., Escalated { reason }, needs_human_reason: Some(reason))` → `escalated`, `record_escalation`; if already in the human state: `UPDATE tasks SET needs_human_reason = $reason, updated_at = NOW()` → `updated`, then if held by `session_id` clear the lease → `released` with `reason: "given_back"`, `attempts` preserved, no `escalated`, no `record_escalation`; `touch(task.id, session_id)` in both cases.
- [ ] The human state is resolved as the project's single `kind = 'human'` state; if none exists (impossible after the deletion refusals) return `Error::Internal` with a logged error.
- [ ] Every function above emits nothing and rolls back on any validation failure (rejections write no history).

## Implementation notes
- Files: `orchestrator/src/tracker/leases.rs` (extend), `orchestrator/src/tracker/escalation.rs` (`Escalation` struct from the mutation task; add the text helpers here).
- `attempts >= max_attempts` uses the value read under the project lock (`Task` re-read via `find_task_for_update` after `TrackerMutation::begin`), not the caller's stale copy.
- `release_leases_for_session` is the function the sessions epic installs in its `on_session_ended`/`on_session_failed` hooks and the Background jobs epic calls from the stuck-task reaper (which decides `Stalled` from `sessions.error == "stalled"`). It must be callable when the session row is already `done`/`failed` and must not touch the session row.
- Lock order for `release_leases_for_session`: only the project row (via `begin`) and task rows; never the session row (session-state writers lock that first in other paths, and the project lock must precede session locks per ADR 0021; this function needs no session lock at all).
- Comment texts above are the contract (tests assert them); keep them in `const`/`format!` helpers in `escalation.rs`.

## Edge cases
- `release_by_agent` at `attempts == max_attempts` on a task already in the human state (a user launched an agent on an escalated task, it failed): escalate path with the human state as target equals the current state → `change_state` is a no-op; handle explicitly: clear the lease, keep `attempts`, set `needs_human_reason`, emit `updated` and `released` (`given_back`), no `escalated`, no email (mirror the `needs_human` already-in-human rule).
- `max_attempts` changed by a user between claims: the comparison uses the current project value.
- A session holding tasks in a project whose human state was renamed: resolution is by kind, not name.
- `needs_human` on an unheld task by a session that never claimed it: allowed by the spec ("or the task being unheld"); the `touch` links it.
- `release_leases_for_session` when the session holds a task the reaper already released: the locked re-read finds nothing; no events.

## Testing
- Integration tests in `orchestrator/tests/tracker_escalation.rs` on the test pool with sessions from `SessionRepository`: with `max_attempts = 3`, three claim+`release_by_agent` cycles → after the third the task is in `needs_human`, `needs_human_reason` starts with `attempt limit reached (3/3):`, three session comments plus one system comment beginning `Escalated to needs_human after 3 attempts.`, events `commented`, `released` (`given_back`) twice, then `commented`, `commented`, `escalated` with `from: "ready", to: "needs_human"`, lease null, `attempts = 0`, one `Escalation` returned; `release_by_agent` by a non-holder → conflict, no rows; `release_leases_for_session` with two held tasks (one at the limit) → one `released` `session_ended` and one `escalated`, both with a system comment and actor `system`, and the function returns exactly one escalation; `Stalled` variant → `reason: "stalled"`; `needs_human` from `ready` → `commented` + `escalated`, lease cleared, `attempts = 0`; `needs_human` on a task already in `needs_human` and held → `commented`, `updated`, `released` (`given_back`), `attempts` preserved, no `escalated`, no escalation returned; `needs_human` on an unheld task → `commented` + `escalated`.
- `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Documentation
- `ARCHITECTURE.md` "Task tracker" → "Attempts and escalation": add one sentence recording that a release at the limit for a task already in the human state clears the lease and records the reason without a new `escalated` event or email (the document covers this only for `needs_human`).

## Assumes from other epics
- "Database schema, models, repositories and test harness": `list_by_lease_holder`, `find_task_for_update`, `SessionRepository` for test sessions.
- "Session lifecycle": installs `release_leases_for_session` in its hooks (a later task of this epic wires it).
- "Background jobs": the stuck-task reaper iterates `done`/`failed` sessions and calls `release_leases_for_session`.