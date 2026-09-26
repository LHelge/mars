---
id: aej86
title: "Run the real-session-image terminal scenario in Engine CI: build the stub image before the engine tests and pass MARS_STUB_IMAGE to them"
status: done
priority: P3
created: "2026-09-20T00:20:51.796665843Z"
updated: "2026-09-26T18:59:43.845994962Z"
tags:
  - infra
  - ci
  - engine
  - tests
depends_on:
  - "6f23c"
attempts: 1
---

## Summary
Follow-up of 6f23c (epic h8kw9, Real-time delivery). `terminal_real_session_image_bash_as_agent` in `orchestrator/tests/engine.rs` runs `/bin/bash -l` as `agent` on a real session image only when `MARS_STUB_IMAGE` is set, and prints a skip line otherwise. The Engine CI workflow builds the stub image after its engine-test step, so the scenario skips on both matrix legs today.

## Documents
- `README.md` "Development" (`MARS_STUB_IMAGE`, the CI workflows).
- The header of `orchestrator/tests/engine.rs` (the note on the terminal scenarios and this follow-up).

## Acceptance criteria
- [ ] The Engine workflow builds the stub image before the engine tests on both the Podman and Docker legs and exports `MARS_STUB_IMAGE` to the engine-test step.
- [ ] The scenario's output in CI shows it ran (no skip line) on both legs.
- [ ] `actionlint` is clean; `README.md` "Development" and the test-file header are updated to say the scenario runs in CI.