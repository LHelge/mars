---
id: cxmar
title: Background jobs
type: epic
status: open
priority: P2
created: "2026-09-16T20:14:15.701334950Z"
updated: "2026-09-16T20:15:46.819502633Z"
tags:
  - orchestrator
  - cron
depends_on:
  - xjaah
---

## Scope

`cron/`: one `CronService` with a named method per job, started after recovery, each job logging its outcome and never panicking the process.

| Job | Interval | Work |
| --- | --- | --- |
| mirror fetch | `MIRROR_FETCH_INTERVAL_SECS` (600) | `git fetch --prune` on every `ready` mirror, `last_fetched_at` |
| idle reaper | 1 min | park idle conversational sessions; stop and fail idle ephemeral ones as `stalled` |
| stuck-task reaper | 1 min | release leases held by `done`/`failed` sessions with `session_ended`/`stalled`, escalate at the attempt limit, system comment and events |
| token cleanup | 1 h | expired refresh tokens, reset tokens, unaccepted invites, orphan secrets |
| secret rotation | 1 h | re-wrap rows behind the newest key version |
| orphan cleanup | 1 h | stray `mars.session_id` containers, `/data/tmp` leftovers, `refs/handoffs/*` without rows under the project git lock |

## Documents

`ARCHITECTURE.md` "Background jobs", "Task tracker" -> "Liveness comes from the session"; `README.md` "Configuration"; `docs/data-model.md` reaper notes.

## Acceptance criteria

- [ ] Each job is a testable method invoked directly in integration tests with time control where needed (idle timeout, expiry).
- [ ] The idle reaper parks a conversational session and fails an ephemeral one with `error = stalled`; the stuck-task reaper releases and escalates with the documented reasons and comments.
- [ ] A failing job logs and is retried at its next interval without stopping the others.

## Out of scope

Dispatcher and scheduled agents (post-v1).