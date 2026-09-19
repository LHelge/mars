---
id: gg4ch
title: "Keep a session's CLI alive across an orchestrator restart on Podman: stdin EOF on attach disconnect ends the run"
status: open
priority: P1
created: "2026-09-19T02:48:11.192398643Z"
updated: "2026-09-19T02:48:11.192398643Z"
tags:
  - orchestrator
  - sessions
  - engine
  - architecture
parent: s52qg
---

## Summary
Found by jt93h (`orchestrator/tests/session_e2e.rs`, restart scenario). On rootless Podman an orchestrator restart, or a graceful `SIGTERM` shutdown, ends every running conversational session's CLI: when the orchestrator's attach connection closes, Podman's compat attach passes the close to the container as stdin EOF even with `StdinOnce: false` (Docker keeps stdin open), and the CLI, stub and pinned version alike, exits 0 on stdin EOF. Measured: the container exits within about 100 ms of the owners being dropped, before `session::recover` lists containers, so recovery adopts an *exited* container and the session comes back `parked` with reason `CLI exited`. Reproduced outside Mars with plain `podman attach` (kill the attach client, the container exits 0).

Nothing is lost (transcript, offsets, checkout and `--resume` survive, and the next message relaunches the session), so this is a liveness regression, not a correctness one. But ADR 0004 makes rootless Podman the target engine, and `ARCHITECTURE.md` "Restart procedure" promises that running sessions are re-adopted ("a restart with a hundred parked sessions and two running ones costs two reattaches"), which today only holds on Docker: on Podman a restart interrupts whatever turn each running agent was in.

## Documents
- `ARCHITECTURE.md` "Restart procedure" (the paragraph "What the CLI makes of the restart" records the limitation and names this task), "Engine adapter" (attach row), "Session container specification" (stdin flags), "Session image".
- ADR 0004, ADR 0010; a new ADR for the mechanism chosen.

## Scope
Hold the CLI's stdin open independently of the orchestrator's attach connection so that a restart on Podman leaves the process running and recovery reattaches to a live conversation. Candidates, to be decided in the ADR: a FIFO on the session volume that the entrypoint opens read-write and the owner writes to (no attach at all); a small holder process in the session image that owns stdin and relays from a socket or FIFO; or an upstream/compat-API fix if one exists for the pinned Podman versions. The decision has to keep the single-writer rule, the uid contract and the stub image's contract.

## Acceptance criteria
- [ ] On rootless Podman, `tests/session_e2e.rs`'s restart scenario asserts the session is still `running` after `recover()`, the same container is adopted, and the next message is delivered to the *same* process (no relaunch, no second `init`).
- [ ] The same holds on Docker.
- [ ] `ARCHITECTURE.md` "Restart procedure" and the attach row describe the mechanism and no longer carry the limitation; the ADR records the rejected alternatives.

## Reference
Bears jt93h (report), `orchestrator/tests/session_e2e.rs` (the scenario that today asserts only the engine-independent half).