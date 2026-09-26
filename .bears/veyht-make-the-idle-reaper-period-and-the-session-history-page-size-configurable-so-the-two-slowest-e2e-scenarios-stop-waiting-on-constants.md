---
id: veyht
title: Make the idle reaper period and the session history page size configurable so the two slowest E2E scenarios stop waiting on constants
status: done
priority: P3
created: "2026-09-20T22:59:36.658435117Z"
updated: "2026-09-26T18:59:43.800583633Z"
tags:
  - orchestrator
  - frontend
  - tests
  - perf
attempts: 1
---

## Summary
Discovered by xn26a (E2E audit, epic 6s8j7). Two scenarios are slow because of constants, not because of what they assert: `sessions.spec.ts` "an idle session is parked without user action" takes 33 to 46 s because the idle reaper runs on `REAPER_PERIOD`, a hard-coded 60 s in `orchestrator/src/cron/mod.rs`; `session-view.spec.ts` "older history loads on scroll-up" takes about 27 s and some sixty inputs because the history window is `PAGE_SIZE = 200` in `frontend/src/session/useSessionSocket.ts`. Together they are about a sixth of the suite's 7 minutes.

## Documents
- `README.md` "Configuration" (variable contract, `.env.example`); `ARCHITECTURE.md` "Session lifecycle" (idle parking, the reaper); `SPEC.md` "WebSocket: session stream" (history window, `?after`/older pages); `frontend/tests/README.md` (slow scenarios).

## Acceptance criteria
- [ ] The reaper period is a documented configuration variable with the current 60 s default (`Config::from_env()`, `README.md` "Configuration", `.env.example`), and `frontend/tests/e2e-stack.sh` sets it to a few seconds.
- [ ] Decide whether the history page size should be configurable at all (it is part of the client/server contract); if yes, by what mechanism (server-advertised in the `session` frame, or a build-time constant the E2E dev server overrides); if no, record the reason in `frontend/tests/README.md` and close that half.
- [ ] The two scenarios' timings before and after are recorded in `frontend/tests/README.md`; both quality chains pass.
