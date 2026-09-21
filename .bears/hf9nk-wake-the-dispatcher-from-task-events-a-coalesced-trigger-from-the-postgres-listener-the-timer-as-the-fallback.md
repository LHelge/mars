---
id: hf9nk
title: "Wake the dispatcher from task_events: a coalesced trigger from the Postgres listener, the timer as the fallback"
status: open
priority: P1
created: "2026-09-21T20:24:21.055470308Z"
updated: "2026-09-21T20:24:21.055470308Z"
tags:
  - orchestrator
  - dispatcher
  - realtime
  - cron
depends_on:
  - svgjq
parent: qabvt
---

## Summary
`ARCHITECTURE.md`, "Dispatcher": the job is "woken by `task_events` and run on a timer as a fallback". With the timer alone a task waits up to a period for its agent. Subscribe the dispatcher to the task-event notifications the one `LISTEN` connection already forwards (`orchestrator/src/events/listener.rs`, `fanout.rs`; `docs/data-model.md`, "Notifications (LISTEN/NOTIFY channels)") and run the job shortly after one arrives. A session ending also frees a slot: decide from the fan-out what signal exists for that (a session state notification if there is one; otherwise the release the stuck-task reaper or `end` writes is already a task event) and say so in the document.

## Acceptance criteria
- [ ] No second `LISTEN` connection; the dispatcher is one more subscriber of the existing fan-out.
- [ ] Wake-ups are coalesced: a burst of events (a planner filing twenty tasks) causes one run after a short debounce, and events arriving during a run cause exactly one more run afterwards. Still never two runs at once, including against the timer's own tick.
- [ ] Only the projects named by the notifications are scanned on a wake-up run if the job's shape makes that cheap; a full scan is acceptable and the timer run is always full.
- [ ] A lagging or reconnecting listener loses nothing that matters: the timer run is the resync. State that in the rustdoc.
- [ ] Shutdown stops the waker with the rest of `CronService`.
- [ ] `ARCHITECTURE.md`, "Dispatcher" and "Event delivery" describe the subscriber.

## Testing
Integration test: with a long timer period, moving a task into a served state launches a session without `run_once` being called; a burst produces launches up to the cap and no overlapping runs. No fixed sleeps — poll for the session row with a deadline.