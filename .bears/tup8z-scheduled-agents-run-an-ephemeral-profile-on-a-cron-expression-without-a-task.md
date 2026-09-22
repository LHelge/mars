---
id: tup8z
title: "Scheduled agents: run an ephemeral profile on a cron expression, without a task"
type: epic
status: done
priority: P1
created: "2026-09-21T20:07:11.967490367Z"
updated: "2026-09-22T10:23:15.146295588Z"
tags:
  - orchestrator
  - frontend
  - scheduler
  - profiles
  - sessions
  - roadmap
---

## Scope
The second v2 automation feature (`README.md`, "Roadmap after v1"; planned in `xd9e4`), after the dispatcher `qabvt`. `ARCHITECTURE.md`, "Task tracker" → "After v1: dispatcher and scheduled agents": a profile gains a cron expression, and a job launches an ephemeral session of that profile, without a task, at each tick. The daily tech-debt scanner that files `ready` tasks is the first instance and needs only `create_task`. The agent that turns GitHub issues into `backlog` tasks is **out of scope**: it needs GitHub access, which is v3.

## Decisions (settled with the user while planning, 2026-09-21; task 1 writes them into an ADR and `ARCHITECTURE.md`)
- **Expression.** Standard 5-field cron, evaluated in **UTC always**. The UI shows the next run in the viewer's local time. Rejected: an instance time zone, a per-profile time zone.
- **Missed ticks are skipped.** Only a tick that comes due while the orchestrator is up fires; nothing is caught up after an outage. Rejected: one catch-up launch at startup.
- **Overlap is bounded by the caps.** A due tick launches unless the unattended-launch capacity check of `qabvt` says no (profile `max_concurrent`, project `max_concurrent_sessions`, instance `AUTOMATION_MAX_SESSIONS`, all live sessions counted) or the project's `automation_paused` is set; then the tick is skipped and logged at `info`, not queued. Rejected: at most one live scheduled session per profile; always launch.
- **Prompt.** A dedicated `schedule_prompt` column, required when a schedule is set: `system_prompt` says who the agent is, `schedule_prompt` says what this run does. It is the `message` of the launch, which an ephemeral launch without a task must have (`validate_launch_prompt`). Rejected: a `profile_schedules` table with several schedules per profile; a fixed generated message.
- **Exactly once per tick.** `agent_profiles.last_scheduled_at` is written in the same transaction that decides to fire, so a restart inside a tick's minute cannot fire it twice. It is a guard, not a catch-up cursor.
- **Eligibility and attribution** as in `qabvt`: `ephemeral` profiles only; refused at save unless the agent credential resolves at `global` or `project` scope, skipped by the job if it later does not; `created_by` NULL and `launch_source = 'schedule'`.

## Acceptance criteria
- [ ] A profile with a schedule launches one ephemeral session per due tick, exactly once per tick across orchestrator restarts, never beyond a cap and never while the project is paused.
- [ ] The rules are in `ARCHITECTURE.md` ("Background jobs" and a "Scheduled agents" section replacing the sketch), the columns in `docs/data-model.md`, the fields in `SPEC.md`.
- [ ] `README.md`, "Roadmap after v1" no longer lists scheduled agents as such; the GitHub-issues instance remains under GitHub access.

## Coordination
Shares the `agent_profiles` migration surface, `ProfileInput` and the profile editor with `qabvt`: its schema task runs after the dispatcher's, not beside it.