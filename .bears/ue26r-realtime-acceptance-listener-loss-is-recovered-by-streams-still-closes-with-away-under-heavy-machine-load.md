---
id: ue26r
title: realtime_acceptance listener_loss_is_recovered_by_streams still closes with Away under heavy machine load
status: open
priority: P1
created: "2026-09-20T06:28:33.779112807Z"
updated: "2026-09-20T07:12:10.951512842Z"
tags:
  - orchestrator
  - tests
  - ws
  - flaky
---

## Summary
`tests/realtime_acceptance.rs::listener_loss_is_recovered_by_streams` failed on 2026-09-20 in two of the three full-suite verification runs of epic qgj33 (MCP), each time while subagents were compiling the orchestrator in parallel on the same machine (load average about 6.5): `the socket ended before sequence 2: Close(Some(CloseFrame { code: Away, reason: "" }))` (panic in `tests/common/app.rs`, `collect_ws_events`). Eight isolated reruns of the binary on the same commits (`13ad916`, `870fc11`) passed, also under that load. No MCP code is on that path.

## Documents
- `SPEC.md` "WebSocket: session stream" (close after two missed pongs).
- `src/ws/mod.rs` `on_ping` (the pong budget), fixed twice already in 092fd4c and 8780401 (task 7sdvv).

## What to find out
The `Away` close is `on_ping` reaching `MAX_MISSED_PONGS`. It reproduces only inside a full `cargo test` run, not when the binary runs alone, so look at what differs: the listener backend was terminated in this scenario, so the reconnect path and the safety read run while the ping ticks. Decide whether the remaining window is in the server (a real bug: a slow server must not close a client that answers) or in the test (ping period too short for a loaded machine; the test config could lengthen it).

## Acceptance criteria
- [ ] The cause is identified and written into the commit message.
- [ ] The test passes 50 consecutive runs while `stress-ng --cpu $(nproc)` (or a parallel `cargo build`) loads the machine.

Discovered from qgj33 (round 2 and round 3 verification).