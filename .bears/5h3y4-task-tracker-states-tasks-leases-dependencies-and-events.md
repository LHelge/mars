---
id: "5h3y4"
title: "Task tracker: states, tasks, leases, dependencies and events"
type: epic
status: done
priority: P1
created: "2026-09-16T20:13:45.794119246Z"
updated: "2026-09-19T18:51:43.757557926Z"
tags:
  - orchestrator
  - tracker
depends_on:
  - pkaee
---

## Scope

The shared tracker as a domain service used by REST now and by MCP and background jobs later (ADRs 0016, 0021, 0022, 0023).

- Tracker mutation transaction: project row lock, validation after locking, task-number allocation, `task_events` append with `pg_notify`, `task_sessions` upsert rules (ADR 0030), all in one transaction; helpers accept the caller's transaction.
- Task states API: list, create (single `human` state), rename, reorder, delete with the documented refusals, `states_changed` events.
- Tasks API: list with filters, create, `PUT` (state as hand-off: lease clear, `attempts` reset, `closed_at`, dependant `blocked` recompute, reopen; current-state no-op rule; no-change emits nothing), delete with dependant recompute and `deleted` event retaining identity, dependencies by `(task, depends_on, kind)` with cycle check on `blocks` only, comments, user `release` (never escalates), `GET /tasks?state_kind=human`, `TaskDetail` with comments, hand-offs, children and sessions.
- Leases and claims: the atomic claim statement, claim-for-launch ignoring served states (unheld, unblocked, non-terminal), agent/reaper release with escalation at `max_attempts` writing the system comment and `escalated` event, `needs_human` semantics including the already-in-human-state case.
- Parents: one-level rules on create and re-parent, parent `blocked` by open children, automatic closure into the lowest-position terminal state with actor `system`, never reopened automatically.
- Discovery provenance resolution for session-created tasks (ADR 0023).
- Escalation email through `EmailClient` to the assignee or all admins honouring `notify_email`.
- Task and session link: `GET /sessions/{id}/tasks`, lease release on session end/failure hook used by the Session lifecycle epic.

## Documents

`ARCHITECTURE.md` "Task tracker" (all subsections except "Code hand-offs" and "Review approval"); `SPEC.md` "Task states", "Tasks", "TaskEvent", "Task board" (behavioural rules); `docs/data-model.md` "Tasks"; ADRs 0016, 0021, 0022, 0023, 0030.

## Acceptance criteria

- [ ] Every task-state and task endpoint has happy-path and error-path tests, including cycle rejection, one-level parent rules, last-queue/terminal/human deletion refusals and the current-state no-op.
- [ ] Concurrency tests: two claims on one task yield exactly one winner; concurrent reciprocal `blocks` edges cannot both succeed.
- [ ] Event tests: each documented `TaskEvent` kind is emitted with the right `actor`, `from`/`to`, `reason`; no event on rejected or no-op operations; `deleted` retains `task_id`.
- [ ] Escalation email is asserted through the mock email client for the assignee case and the all-admins case with opt-out.

## Out of scope

Hand-off publication and review (Code hand-offs epic); MCP tools (MCP epic); the stuck-task reaper schedule (Background jobs epic).