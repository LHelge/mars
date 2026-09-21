---
id: rtfys
title: Copy the bash tool and process reaping into curiosity, without roles or worktrees
status: open
priority: P2
created: "2026-09-21T12:18:46.046864Z"
updated: "2026-09-21T20:26:14.866541859Z"
tags:
  - curiosity
  - tools
depends_on:
  - de4rw
parent: w9nsq
---

## Summary
The shell tool is the one with real couplings: `tools/bash.rs` reaches into `role` (per-role command policy) and `worktree` (where it runs), and `procs.rs` reaps what a session's shell left running. Source: `../midgaard/src/tools/bash.rs`, `src/procs.rs` at `0684a87`.

## Acceptance criteria
- [ ] `bash` runs in the agent's working directory with the process environment it inherited — in a Mars container that includes the injected secrets, which is intended (`ARCHITECTURE.md`, "Secrets", "Injection").
- [ ] The role policy is removed, not replaced: no allow/deny list (ADR 0012: the container is the permission boundary). Timeouts, output truncation and cancellation stay.
- [ ] `procs.rs` comes with its Linux/macOS gating (the `CLOCK_BOOTTIME` fix of midgaard `7e3pw`). Reaping at the end of a session stays; add the second trigger Mars needs: on `SIGINT`/`SIGTERM` the children of a running command are terminated before the agent exits, so a container stop does not wait on an orphan. (Signal *handling* is the ACP epic's task; here the reaper is callable from it.)
- [ ] As PID 1 the agent is the reaper of last resort: a test documents that zombies of finished commands do not accumulate over many tool calls.
- [ ] The tests that needed a worktree use a `tempfile` directory.

## Edge cases
- A command that backgrounds a server and returns: allowed; reaped at session end, reported by the existing `ProcessesReaped` event.
- Output that is not UTF-8; a command killed by timeout: tool error text, never a panic.

## Testing
- The copied tests pass on Linux and macOS. Quality chain passes.