---
id: ue26r
title: realtime_acceptance listener_loss_is_recovered_by_streams still closes with Away under heavy machine load
status: open
priority: P2
created: "2026-09-20T06:28:33.779112807Z"
updated: "2026-09-20T06:28:33.779112807Z"
tags:
  - orchestrator
  - tests
  - ws
  - flaky
---

## Summary
`tests/realtime_acceptance.rs::listener_loss_is_recovered_by_streams` failed once on 2026-09-20 during the round-2 verification of epic qgj33 (MCP), while a subagent was compiling the orchestrator in parallel on the same machine: `the socket ended before sequence 2: Close(Some(CloseFrame { code: Away, reason: "" }))` (panic in `tests/common/app.rs`, `collect_ws_events`). Five isolated reruns on the same commit (`13ad916`) passed. No MCP code is on that path.

## Documents
- `SPEC.md` "WebSocket: session stream" (close after two missed pongs).
- `src/ws/mod.rs` `on_ping` (the pong budget), fixed twice already in 092fd4c and 8780401 (task 7sdvv).

## What to find out
The `Away` close is `on_ping` reaching `MAX_MISSED_PONGS`. Under a starved runtime the axum-test client may not answer two pings within their periods, or the server may still count a pong it has not read. Decide whether the remaining window is in the server (a real bug: a slow server must not close a client that answers) or in the test (ping period too short for a loaded CI machine; the test config could lengthen it).

## Acceptance criteria
- [ ] The cause is identified and written into the commit message.
- [ ] The test passes 50 consecutive runs while `stress-ng --cpu $(nproc)` (or a parallel `cargo build`) loads the machine.

Discovered from qgj33 (round 2 verification).