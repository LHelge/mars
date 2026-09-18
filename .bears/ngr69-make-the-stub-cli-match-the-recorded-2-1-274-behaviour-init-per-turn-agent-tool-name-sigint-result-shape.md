---
id: ngr69
title: "Make the stub CLI match the recorded 2.1.274 behaviour: init per turn, Agent tool name, SIGINT result shape"
status: open
priority: P2
created: "2026-09-18T08:02:08.320358728Z"
updated: "2026-09-18T08:02:08.320358728Z"
tags:
  - images
  - tests
  - docs
depends_on:
  - h3e43
parent: "8vnwy"
---

## Summary
The credentialed verification (task h3e43, `images/claude/VERIFY.md`) showed three places where `images/stub/claude` differs from the real CLI it stands in for. Session-owner and Playwright tests run against the stub, so the differences hide real behaviour.

## Documents
- `images/claude/VERIFY.md` "Observed on 2.1.274"; `ARCHITECTURE.md` "Session image" (stub list), "Stop semantics"

## Acceptance criteria
- [ ] The real CLI writes a `system`/`init` line at the start of every turn; the stub drops the fixture's init lines and writes one per process. Decide with the translator task whether the stub should emit its own init per turn, and implement it.
- [ ] On SIGINT mid-turn the real CLI writes a `user` line `[Request interrupted by user]` and a `result` with `subtype: "error_during_execution"`, `is_error: true`, `terminal_reason: "aborted_streaming"`, then exits 0; the stub writes `subtype: "success"`, `is_error: false`. The stub follows the real shape; `images/smoke-test.sh` and `ARCHITECTURE.md` "Session image" are updated in the same commit.
- [ ] The stub's `init.tools` lists `Task`; 2.1.274 lists and uses `Agent`. List both or follow the pin, and keep the synthesised reply unchanged.
- [ ] Stub unit tests cover each change; the smoke test passes on Podman and Docker.