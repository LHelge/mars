---
id: tj596
title: "Curiosity as PID 1: SIGINT ends the turn and the process, SIGTERM exits 143, a FIFO stdin never ends it"
status: open
priority: P1
created: "2026-09-21T12:19:58.832817Z"
updated: "2026-09-21T12:19:58.832817Z"
tags:
  - curiosity
  - acp
  - process
depends_on:
  - pfs5r
parent: vj82v
---

## Summary
The session image contract makes the CLI PID 1 of its container, created without `Init` (`ARCHITECTURE.md`, "Session image"): a pid namespace's init receives a signal from outside only if it installed a handler for it. Mars stops a session with `SIGINT`, then `SIGTERM`, then kill ("Stop semantics"), and the stub documents the exit statuses the owner expects. Make `curiosity acp` behave exactly so.

## Documents
- `curiosity/README.md`: signal and exit-status table. It is what `ARCHITECTURE.md`'s "Curiosity invocation" section will cite.

## Acceptance criteria
- [ ] Handlers for `SIGINT` and `SIGTERM` are installed before the first line is read.
- [ ] `SIGINT`: the turn in progress is cancelled, the pending `session/prompt` is answered with `stopReason: cancelled`, children are reaped (foundation epic's reaper), the session log is flushed, exit **0**. With no turn in progress: flush and exit 0.
- [ ] `SIGTERM`: the same shutdown without waiting for the model call to unwind beyond a short bound; exit **143**.
- [ ] stdin is a FIFO the process itself also holds open for writing (ADR 0034), so EOF never arrives in a Mars container; the process must not depend on EOF to flush or to finish a turn. When EOF does arrive (a plain pipe), exit 0.
- [ ] The response to a cancelled prompt is on stdout **before** exit, so Mars's transcript ends with a closed turn — the owner distinguishes a stopped turn from a failed one by it.

## Implementation notes
- `tokio::signal::unix`. Reuse the cancellation path of `session/cancel`.

## Testing
- Integration test spawning the binary: signal during a slow mock turn, assert the last stdout line and the exit status; the same with stdin opened read-write on a FIFO. A test that the binary run as PID 1 (`unshare --pid --fork` where available, otherwise left to the image smoke test and marked so) reacts to `SIGINT`.