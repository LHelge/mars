---
id: w7y2t
title: "E2E: automatic merge and the round limit in a browser, with coverage rows"
status: open
priority: P2
created: "2026-09-22T20:20:47.557644753Z"
updated: "2026-09-22T20:20:47.557644753Z"
tags:
  - frontend
  - e2e
depends_on:
  - "6w2qf"
  - vssu2
  - nq3dc
parent: tykeu
---

Playwright scenarios for the two new `SPEC.md`, "User-facing features" paragraphs, "Automatic merge" and "Round limit", arranged through `tests/utils/fixtures.ts` and `test-helpers.ts` (`frontend/tests/README.md`).

## What to do
- `automerge.spec.ts` (or extend `handoffs.spec.ts`): a new project's `merge` column carries the `auto-merge` chip; approving a hand-off into `merge` merges it without any session — the card reaches `done` with the `Merged <commit>` comment and the integration head moves; a conflicting hand-off comes back to `ready` with the paths and `main` is unchanged; turning auto-merge off in the states editor leaves the next approved task in `merge`.
- Round limit: with `max_rounds` set to 1 in the project settings, a session-actor `changes_requested` forward sends the task to `needs_human` with the reason. If driving an agent send-back through the stub image is impractical, assert the auto-merge conflict path at the limit instead, which is system-actor.
- Add the two rows to the coverage table in `frontend/tests/README.md`, and extend the "Task board" row of the "Frontend" table for the editor controls and card label; run `node tests/coverage-check.mjs`.
- No fixed sleeps: wait on the card or the SSE-driven refresh.

## Done when
Frontend quality chain passes, including `test:e2e`.