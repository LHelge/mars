---
id: qabvt
title: "Dispatcher: launch ephemeral sessions automatically when a served task state has claimable work, bounded per profile"
type: epic
status: done
priority: P1
created: "2026-09-21T20:07:03.177266837Z"
updated: "2026-09-21T23:37:21.030708021Z"
tags:
  - orchestrator
  - frontend
  - dispatcher
  - tracker
  - sessions
  - roadmap
---

## Scope
The first v2 automation feature (`README.md`, "Roadmap after v1"; planned in `xd9e4`). Nothing in v1 launches a session by itself. `ARCHITECTURE.md`, "Task tracker" → "After v1: dispatcher and scheduled agents" sketches the design: a job, woken by `task_events` and run on a timer as a fallback, finds auto-launch profiles whose served states contain a claimable task and launches an ephemeral session for the highest-priority such task through the same path as a user launch. The reaper, the attempt limit and escalation to the human state already cover an unattended agent that fails; this epic adds only the launching.

## Decisions (settled with the user while planning, 2026-09-21; task 1 writes them into an ADR and `ARCHITECTURE.md`)
Shared with the scheduled-agents epic `tup8z` — an *unattended launch* is one by the dispatcher or the scheduler:
- **Three caps.** An unattended launch happens only while the live (`creating` or `running`) sessions are below every one of: `agent_profiles.max_concurrent` for the profile, `projects.max_concurrent_sessions` for the project, and the instance-wide config variable `AUTOMATION_MAX_SESSIONS`. *All* live sessions count, whoever launched them. The caps hold automation back only: a launch by hand is never refused by them.
- **Pause.** `projects.automation_paused`, a toggle on the project page: no unattended launch of any kind in the project while set. No restart needed. There is no instance-wide switch.
- **Eligibility.** `auto_launch` (and a schedule) only on `ephemeral` profiles. Both are refused at save unless the backend's agent credential resolves without a user, that is at `global` or `project` scope (ADR 0036; `sessions.created_by` is NULL for an unattended launch, so the `user` scope never resolves). The credential can be deleted afterwards, so the jobs also skip such a profile and log it at `info`, claiming nothing.
- **Attribution.** `sessions.launch_source` (`user`, `dispatcher`, `schedule`) says who launched a session; `created_by` is NULL for an unattended launch, but NULL alone proves nothing, because a deleted user leaves the same NULL. Tracker actor `System`.

Dispatcher only:
- Honours served states (a user launch does not). Never dispatches a task in the `human` state, a held task, a blocked task or a terminal one.
- Task order is exactly that of `tracker::leases::ready_summaries`, so MCP `ready` and the dispatcher agree.
- Two auto-launch profiles serving one state: the older profile (`created_at`) wins.
- **No circuit breaker.** A launch that fails in `creating` releases the task as in v1, and the attempt limit with escalation to the human state is the only back-off. Rejected: a per-profile breaker; not counting `creating` failures as attempts.

## Acceptance criteria
- [ ] A task moved into a state served by an `auto_launch` profile gets an ephemeral session without anyone clicking, and never beyond any of the three caps, and never while the project is paused.
- [ ] A dispatcher launch is the user launch path with no user: same claim transaction, same generated task message, same failure release.
- [ ] The rules are in `ARCHITECTURE.md` (the sketch becomes a real "Dispatcher" section), the columns in `docs/data-model.md`, the fields in `SPEC.md`, the config variable in `README.md` and `.env.example`.
- [ ] `README.md`, "Roadmap after v1" no longer lists the dispatcher.

## Coordination
The scheduled-agents epic `tup8z` depends on this epic's launch-path extraction, its schema task and its capacity check. The agent seam epic `fgbm3` works in `agent/` and the session owner; the extraction here works in `routes/sessions.rs` and `session/` — check overlap before running both at once.