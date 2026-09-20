---
id: ee72w
title: Re-check the holder session's state under the project lock in release_leases_for_session
status: done
priority: P3
created: "2026-09-20T05:52:24.395731316Z"
updated: "2026-09-20T07:00:34.238073656Z"
tags:
  - orchestrator
  - tracker
  - cron
parent: cxmar
---

## Summary
Found while implementing the stuck-task reaper (bgynw). `tracker::leases::release_leases_for_session` re-reads the task rows under the project lock but not the holder session's state. The reaper lists dead holders outside any transaction, so a holder retried (`failed -> parked`) between the listing and the release has its leases released a moment after it became alive again. The task is merely re-claimable, so the impact is small, but the bgynw task text ("the primitive re-reads the session state under the project lock and releases nothing if the holder is alive again") describes a guard the code does not have.

## Documents
- `ARCHITECTURE.md` "Task tracker" -> "Liveness comes from the session, not from tool calls"
- `docs/data-model.md` "Tracker mutation transactions"

## Acceptance criteria
- [ ] Decide whether a dead session can become alive again at all (which transitions leave `failed`/`done`); if none can, record that in the primitive's doc comment and close this task without code.
- [ ] Otherwise the primitive reads `sessions.state` inside the locked transaction when called by the reaper and releases nothing for a holder that is `creating`, `running` or `parked`; an integration test in `tests/cron_stuck_tasks.rs` covers it.

## Notes
Touches `src/tracker/leases.rs`; coordinate with epic kg8cx (tracker mutation envelope).