---
id: dy7gr
title: Write a first SSE frame at once so a task stream with nothing to replay opens through a buffering proxy
status: done
priority: P1
created: "2026-09-20T17:51:23.324591432Z"
updated: "2026-09-20T18:56:41.677457111Z"
tags:
  - orchestrator
  - realtime
  - frontend
  - tests
parent: "6s8j7"
attempts: 1
---

## Summary
Found by the 7m22f task board E2E specs. On a project with no task events to replay, the board sits on "Loading the task board" for 15 s under `npm run dev`. `useTaskStream.connect()` makes the stream's `onopen` the board's first REST load. `orchestrator/src/sse/mod.rs` starts both timers at `Instant::now() + interval` (`keepalive: interval_at(Instant::now() + timings.sse_keepalive, …)`), so a replay-empty stream writes no body byte until the first keepalive at 15 s (`realtime_timings.sse_keepalive`, `orchestrator/src/prelude/state.rs`). Measured against the orchestrator directly: headers at 5 ms, first body byte at 15007 ms. Vite's dev proxy holds a proxied response's headers until the first body byte (headers at 15027 ms), so `EventSource.onopen` fires 15 s late. nginx is unaffected (`X-Accel-Buffering: no`), but any buffering proxy in front of the API behaves like Vite.

## Documents
- `SPEC.md` "SSE: task stream" and "Authentication" (stream rules, keepalive); `ARCHITECTURE.md` "Event delivery".
- `SPEC.md` "Frontend", "Board refresh ordering" (the stream opens before the first load).

## Acceptance criteria
- [ ] The task SSE stream writes one comment frame immediately after the response starts (for example `: ready`, or the keepalive ticking once at `Instant::now()`), before any replay, so the first body byte follows the headers at once. The re-authorization the keepalive tick performs keeps its cadence.
- [ ] `SPEC.md` "SSE: task stream" says the stream opens with a comment frame; clients ignore comments.
- [ ] An integration test opens the stream on a project with no events and reads the first frame within 1 s (far below the keepalive interval the test configures).
- [ ] `frontend/tests/tasks.spec.ts`: `FIRST_LOAD_TIMEOUT = 25_000` and its explanatory comment are removed; the empty-board and create-from-form scenarios no longer take ~18 s each.
- [ ] Check whether the session WebSocket or any other stream has the same first-byte gap and say so in the report.
- [ ] Backend and frontend quality chains pass.

## Out of scope
Changing Vite's proxy configuration as the fix: the orchestrator should not depend on the proxy in front of it.
