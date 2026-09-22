---
id: v6p2w
title: "Scheduler job: fire each due profile schedule exactly once, as a task-less ephemeral launch through the capacity check"
status: done
priority: P1
created: "2026-09-21T20:25:23.636566263Z"
updated: "2026-09-22T09:06:56.367453329Z"
tags:
  - orchestrator
  - scheduler
  - cron
  - sessions
depends_on:
  - "6f9uu"
  - rgrvp
  - duzjs
parent: tup8z
attempts: 1
---

## Summary
The scheduler, as a named job of `CronService` with a one-minute period. Implements `ARCHITECTURE.md`, "Scheduled agents" and its row in "Background jobs".

## Behaviour
Each run takes `now` (the job already receives it, which is what makes this testable) and, for every scheduled ephemeral profile of a `ready` project: the tick is *due* if the expression has an occurrence in `(max(last_scheduled_at, process start), now]`. Process start is the floor so that ticks missed while the orchestrator was down are skipped, never caught up. For a due tick: claim it by advancing `last_scheduled_at` in one statement with the old value in the `WHERE` (so it fires exactly once even across a restart inside the same minute), then consult the capacity check (`duzjs`); allowed → launch through the extracted creation path (`rgrvp`) with the orchestrator as actor, no task, `schedule_prompt` as the message and a title that names the schedule run; refused (paused, any cap) or credential missing → the tick is spent, logged at `info` with the reason, and not queued. Several occurrences inside one window (a long-running previous tick of the job) fire once.

## Acceptance criteria
- [ ] New `JobName::Scheduler`, one-minute period, no config variable unless the ADR asked for one; never overlaps itself; `JobReport` counts fired and skipped-by-reason.
- [ ] `last_scheduled_at` is advanced before the launch, not after: a crash between the two loses one run rather than doubling it. Say so in the rustdoc and in `ARCHITECTURE.md`.
- [ ] A launch failure is logged and does not roll `last_scheduled_at` back; the next tick is the retry.
- [ ] The first run after start fires nothing that came due before the process started.
- [ ] The session is an ordinary ephemeral session: `launch_source = 'schedule'`, `created_by` NULL, `task_id` NULL, default branch as base, idle reaper and cost accounting as for any other.
- [ ] `ARCHITECTURE.md`, "Background jobs" gains the row.

## Testing
Integration tests with `TestApp` and the mock engine driving `run_once(JobName::Scheduler, now)` with chosen instants: a due tick fires once and a second run in the same minute fires nothing; a tick before process start is skipped; a refused tick (each of pause, profile cap) is spent and the following tick fires; two occurrences in one window fire once; clearing the schedule stops it. The process-start floor must be injectable for these tests.