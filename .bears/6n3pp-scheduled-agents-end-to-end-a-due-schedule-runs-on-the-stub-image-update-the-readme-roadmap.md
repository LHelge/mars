---
id: "6n3pp"
title: "Scheduled agents end to end: a due schedule runs on the stub image; update the README roadmap"
status: open
priority: P1
created: "2026-09-21T20:25:58.761672025Z"
updated: "2026-09-21T20:26:04.012246145Z"
tags:
  - orchestrator
  - frontend
  - scheduler
  - tests
  - docs
depends_on:
  - v6p2w
  - k73td
  - "7mnqm"
parent: tup8z
---

## Summary
Proof in the browser and on real containers that a schedule fires, and the documents' final state for the epic `tup8z`.

## The testing problem to solve first
A cron expression cannot come due faster than the next minute boundary, and the suites must not sleep for a fixed period or wait a minute. Give the integration-tests build a way to make a tick due now — a test-only route under `/api/test/` that runs the scheduler job once with a supplied `now`, beside the existing test-only routes (`orchestrator/src/routes/test.rs`) — and document it in `README.md`, "Development", "End-to-end tests" with the others. It exists only under the `integration-tests` feature.

## Acceptance criteria
- [ ] Playwright scenario through the suite's fixtures: a user gives an ephemeral stub-image profile a schedule and prompt (credential arranged at project or global scope), sees the next run on the editor, the test makes the tick due, and a schedule-labelled session appears in the project's session list and reaches `done`; with automation paused the same trigger launches nothing. The `sessions` fixture ends what the scheduler launched.
- [ ] `orchestrator/tests/session_e2e.rs`: one scenario running a scheduled launch on the stub image to `done` under `DOCKER_HOST`, serialised by the suite's mutex.
- [ ] `frontend/tests/README.md` coverage table gains the rows; `SPEC.md`, "User-facing features" describes scheduled profiles.
- [ ] `README.md`, "Roadmap after v1": scheduled agents are removed as a feature; the agent that turns GitHub issues into backlog tasks stays, listed with GitHub access. `ARCHITECTURE.md` no longer has an "After v1: dispatcher and scheduled agents" sketch if both halves have been replaced.
- [ ] Both full quality chains of CLAUDE.md, "Code quality" pass.