---
id: rapkz
title: "engine_contract's stop assertion flaked on Docker CI: exit 137 instead of the TERM handler's 143"
status: in_progress
priority: P3
created: "2026-09-21T21:20:58.427499212Z"
updated: "2026-09-26T17:16:41.116876968Z"
tags:
  - orchestrator
  - test
  - engine
  - flaky
attempts: 1
---

Problem: Engine workflow run 35655044347 (commit 7c5df61, 2026-09-21), job `engine (docker)`: `tests/engine.rs` › `engine_contract` panicked at tests/common/engine_contract.rs:288, "a stop ends the container on its TERM handler's code": left `ExitStatus { code: 137 }`, right `ExitStatus { code: 143 }`. The container was SIGKILLed — the stop's grace period elapsed before its TERM trap had run (or before the trap was installed) on a slow runner. `engine (podman)` passed in the same run, a re-run of the Docker job passed, the eleven Engine runs before it were green, and the pushed commits change nothing under orchestrator/src/engine, the engine tests or images/.

Acceptance: the contract test cannot race the container's start-up: wait until the container has signalled that its TERM trap is installed (a line on its log stream, or a file the test execs for) before calling stop, and give the stop a grace period that a loaded CI runner survives; the normalised semantics being asserted (ARCHITECTURE.md, "Engine adapter") do not change, and both halves of the contract suite (MockEngine, BollardEngine) still pass it. Same family as 4rwdc (exec_pty_echo_and_resize flaked once on rootless Podman).

References: orchestrator/tests/common/engine_contract.rs:288; orchestrator/tests/engine.rs; CLAUDE.md, "Testing expectations", engine tests. Discovered from: the midway push of epic 579dz.