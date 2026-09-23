---
id: mrgmh
title: "Flaky: realtime_acceptance::slow_client_does_not_block_others closes with Away before the last sequence"
status: open
priority: P2
created: "2026-09-23T14:44:08.145669072Z"
updated: "2026-09-23T14:44:08.145669072Z"
tags:
  - orchestrator
  - realtime
  - tests
  - flaky
---

Observed in Release run 35874346197 (commit 0877ed1, no Rust change), job `orchestrator / Format, clippy, tests`, under nextest on a hosted runner:

```
FAIL [2.713s] mars-orchestrator::realtime_acceptance slow_client_does_not_block_others
panicked at tests/common/app.rs:1140:26:
the socket ended before sequence 200: Close(Some(CloseFrame { code: Away, reason: "" }))
```

The fast client's socket was closed with `Away` before it collected sequence 200. Either the server is closing the fast client while the slow one lags, which would be the property the test guards (SPEC.md, "WebSocket: session stream"; ARCHITECTURE.md, "Event delivery"), or the test's timing or its shutdown order is racy under a loaded runner. Find which it is, fix it, and prove it by looping the test (for example `cargo nextest run --features integration-tests -E 'test(slow_client_does_not_block_others)' --retries 0` 200 times) on a loaded machine.

Found while landing 2v86y; the other 2421 tests passed on the same run.