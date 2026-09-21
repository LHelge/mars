---
id: "7mnqm"
title: "Dispatcher end to end: a task moved into a served state runs on the stub image unattended; retire the README roadmap entry"
status: done
priority: P1
created: "2026-09-21T20:24:45.322323428Z"
updated: "2026-09-21T23:37:21.030350753Z"
tags:
  - orchestrator
  - frontend
  - dispatcher
  - tests
  - docs
depends_on:
  - hf9nk
  - nsvc5
parent: qabvt
attempts: 1
---

## Summary
Proof on real containers and in the browser that the dispatcher closes the loop, and the documents' final state for the epic `qabvt`.

## Acceptance criteria
- [ ] `orchestrator/tests/session_e2e.rs` (runs only with `DOCKER_HOST`, under its own `cargo test --test engine --test session_e2e` invocation, serialised by the suite's mutex): an `auto_launch` ephemeral profile on the stub image (`MARS_STUB_IMAGE`), a task moved into its served state, a session reaches `done` with no launch call, and the lease is released by the v1 paths. If the stub transcript cannot hand the task off, assert what it can do and say so.
- [ ] Playwright scenario through the fixtures of `tests/utils/fixtures.ts`: a user enables auto-launch on an ephemeral profile (a project- or global-scope fake credential arranged first), moves a task into the served state on the board, and sees a dispatcher-labelled session appear on the task and in the session list; pausing automation on the project page stops the next one. The `sessions` tracker fixture must end sessions it did not launch itself — extend it to sweep the project's sessions. Nothing sleeps for a fixed period; the E2E stack sets a short `DISPATCHER_INTERVAL_SECS`.
- [ ] `frontend/tests/README.md` coverage table gains the rows; `SPEC.md`, "User-facing features" lists automatic dispatch, the caps and the pause if the earlier tasks have not already.
- [ ] `README.md`, "Roadmap after v1" no longer lists the dispatcher; `docs/data-model.md`, `profile_kind` note ("automatic launching of ephemeral sessions is post-v1") is corrected.
- [ ] Both full quality chains of CLAUDE.md, "Code quality" pass.