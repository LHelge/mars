---
id: praxe
title: Refuse a zero DISPATCHER_INTERVAL_SECS and MIRROR_FETCH_INTERVAL_SECS at startup instead of panicking in the job's first tick
status: in_progress
priority: P3
created: "2026-09-26T18:13:53.031575752Z"
updated: "2026-09-26T19:46:16.761768943Z"
tags:
  - orchestrator
  - config
attempts: 1
---

## Summary
Found by veyht. `Config::from_env()` (`orchestrator/src/prelude/config.rs`) now refuses `REAPER_INTERVAL_SECS=0`, because `tokio::time::interval(Duration::ZERO)` panics and would take the job down at its first tick. `DISPATCHER_INTERVAL_SECS` and `MIRROR_FETCH_INTERVAL_SECS` are parsed with the same `optional_parsed::<u64>` and have no such bound, so a zero there panics in the cron job rather than failing fast at startup.

## Documents
- `README.md`, "Configuration" (the variable contract); `CLAUDE.md`, "Backend conventions" (`Config::from_env()` fails fast).

## Acceptance criteria
- [ ] Every interval variable `JobName::period` reads (`DISPATCHER_INTERVAL_SECS`, `MIRROR_FETCH_INTERVAL_SECS`, `REAPER_INTERVAL_SECS`) is refused at startup when zero, naming the variable, through one shared helper rather than three copies of the check.
- [ ] A unit test per variable in `config.rs`.
- [ ] `README.md` "Configuration" states the lower bound of each.

Discovered from: veyht.