---
id: "4q3t2"
title: "Engine test container sweep removes a concurrent run's containers: scope cleanup_stale_test_containers to dead runs"
status: open
priority: P3
created: "2026-09-26T17:34:03.165407834Z"
updated: "2026-09-26T17:34:03.165407834Z"
tags:
  - orchestrator
  - tests
  - engine
  - flaky
---

## Summary
Found by aej86. `cleanup_stale_test_containers` in `orchestrator/tests/common/engine.rs` (around line 123) force-removes every `mars.test` container whose run id differs from its own process's. Two `tests/engine.rs` (or `session_e2e.rs`) runs on one engine socket at the same time — two worktrees, or the coordinator and a task-implementer — therefore remove each other's containers mid-scenario. It showed as `Conflict("container ... in wrong state \"stopping\"")` at `tests/engine.rs:828` while a sibling ran the engine suite. CI is unaffected (one engine per job); local parallel work is not, and it may explain some of the "flaky on rootless Podman" reports (4rwdc).

## Documents
- `CLAUDE.md`, "Testing expectations", engine tests; the header of `tests/common/engine.rs`.

## Acceptance criteria
- [ ] The sweep removes only containers of runs that are no longer alive: for example it labels each container with the run's pid and start time and skips a label whose process still exists, or only removes containers older than a bound no live run reaches.
- [ ] Two concurrent `cargo test --features integration-tests --test engine` runs on one Podman socket both pass.
- [ ] A crashed run's leftovers are still removed by the next run.

Discovered from: aej86.