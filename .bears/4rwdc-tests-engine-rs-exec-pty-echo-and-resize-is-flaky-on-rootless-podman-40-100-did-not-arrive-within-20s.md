---
id: "4rwdc"
title: "tests/engine.rs exec_pty_echo_and_resize is flaky on rootless Podman: \"40 100\" did not arrive within 20s"
status: done
priority: P3
created: "2026-09-21T17:35:14.148056438Z"
updated: "2026-09-26T18:59:43.893653055Z"
tags:
  - orchestrator
  - engine
  - tests
  - flaky
attempts: 1
---

## Summary
Seen once on 2026-09-21 while verifying kb48s (a change that touches no engine code): `cargo test --features integration-tests --test engine --test session_e2e` on rootless Podman failed `exec_pty_echo_and_resize` at `tests/engine.rs:131` with `"40 100" did not arrive within 20s`; the other 13 engine tests passed, and an immediate rerun of the whole binary passed 14/14. The machine had just finished a full nextest run and a stub image build.

## Documents
- `ARCHITECTURE.md`, "Engine adapter" (exec, PTY resize); `CLAUDE.md`, "Testing expectations", engine tests.

## Acceptance criteria
- [ ] Reproduce (loop the test, under load) and decide whether the resize is lost by the adapter (a resize issued before the exec's PTY exists) or only read too early by the test.
- [ ] Fix at the level the cause lives at; nothing sleeps for a fixed period.