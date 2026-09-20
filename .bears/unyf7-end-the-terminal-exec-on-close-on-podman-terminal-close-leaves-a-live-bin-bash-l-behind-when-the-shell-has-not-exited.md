---
id: unyf7
title: "End the terminal exec on close on Podman: Terminal::close leaves a live /bin/bash -l behind when the shell has not exited"
status: done
priority: P2
created: "2026-09-20T00:20:46.260032178Z"
updated: "2026-09-20T10:51:25.343028527Z"
tags:
  - orchestrator
  - engine
  - realtime
depends_on:
  - "6f23c"
attempts: 1
---

## Summary
Discovered by 6f23c (epic h8kw9, Real-time delivery). `BollardExec::close` (`orchestrator/src/engine/streams.rs`) half-closes the exec attach's input and drains the output. On Docker that is the shell's EOF; on rootless Podman (6.1.2, compat API) nothing is passed on, so an idle `/bin/bash -l` stays at its prompt, the drain runs out its 5 s timeout, and `close` answers `-1`. `inspect_exec` afterwards shows `running = true`. A session whose terminal is opened and closed repeatedly accumulates idle shells in the container until it stops, and every `terminal_close` on Podman takes the full 5 s and reports `exit_code: -1`.

## Documents
- `ARCHITECTURE.md` "Engine adapter", the `exec + resize (TTY on)` row, records the difference as observed; this task replaces that note with the normalised behaviour.
- `SPEC.md` "WebSocket: session stream" (terminal paragraph; `terminal_closed { exit_code }`).
- `CLAUDE.md` "Testing expectations": a changed normalisation changes the engine contract suite and both halves pass it.

## Acceptance criteria
- [ ] After `ExecSession::close` on a live PTY exec, the exec's process is gone on both engines (for example: write EOT / `exit` before the half-close, or kill the exec's pid through a second exec, whichever is verified on both engines), and `close` returns well inside its bound.
- [ ] `terminal_adapter_resize_and_close_while_alive` in `orchestrator/tests/engine.rs` asserts the exec is no longer running instead of recording either outcome.
- [ ] The `exec + resize` Notes column in `ARCHITECTURE.md` describes the normalised behaviour; the accumulation caveat is removed.
- [ ] Backend quality chain passes with `DOCKER_HOST` on Podman; the Docker half is confirmed by the Engine CI workflow.