---
id: svgjq
title: "Dispatcher job: on a timer, launch an ephemeral session for the best claimable task of each auto-launch profile"
status: done
priority: P1
created: "2026-09-21T20:24:08.300481180Z"
updated: "2026-09-21T22:24:12.207971796Z"
tags:
  - orchestrator
  - dispatcher
  - cron
  - tracker
  - sessions
depends_on:
  - rgrvp
  - duzjs
parent: qabvt
attempts: 1
---

## Summary
The dispatcher itself, as a named job of `CronService` (`orchestrator/src/cron/`, `JobName`, `scheduler::spawn_job`), on a timer only; the `task_events` wake-up is the next task. Implements `ARCHITECTURE.md`, "Dispatcher" and the row it adds to "Background jobs".

## Behaviour
Each run, for every `ready` project that is not `automation_paused`, for every `auto_launch` ephemeral profile in `created_at` order: while the capacity check (`duzjs`) allows and `tracker::leases::ready_summaries` over the profile's served states returns a task, launch through the extracted creation path (`rgrvp`) with the orchestrator as actor and served states enforced. The claim inside that path is what decides: a 409 `task is not claimable` (someone else got it first) is a `debug` line and the next candidate, never an error. A profile whose agent credential no longer resolves at `global`/`project` scope is skipped with one `info` line per run and claims nothing.

## Acceptance criteria
- [ ] New `JobName::Dispatcher` with a period from config (`DISPATCHER_INTERVAL_SECS`, documented default, in `README.md` and `.env.example`); runs once at start after recovery like every job; a tick never overlaps its previous run; outcome in a `JobReport` (launched, skipped-by-reason counts), no-work outcomes at `debug`.
- [ ] Lock discipline: the job holds no lock across launches; each launch takes git lock then project lock exactly as a user launch does (ADR 0021). Candidate reads are lock-free pool reads.
- [ ] A launched session has `launch_source = 'dispatcher'`, `created_by` NULL, `task_id` set, the generated task message as its prompt, and starts from the task's current hand-off when it has one.
- [ ] Never dispatched: a task in the `human` state, a held, blocked or terminal task, a task in a state the profile does not serve, anything for a `conversational` profile even if a row somehow has `auto_launch`.
- [ ] A launch that fails in `creating` releases the task exactly as in v1; the job adds no back-off of its own.
- [ ] `ARCHITECTURE.md`, "Background jobs" table gains the row; the "Dispatcher" section loses any not-yet-implemented marker.

## Testing
Integration tests with `TestApp` and the mock engine, driving `CronService::run_once(JobName::Dispatcher, now)`: a ready task gets a session and a `System` `claimed` event; priority order follows `ready_summaries`; caps and pause are respected across two runs; two profiles serving one state — the older wins; an unserved state, a blocked task and a human-state task are left alone; a missing credential skips without claiming; a lost claim moves on to the next task. Tracker preconditions are arranged through `tests/common/tracker.rs`.