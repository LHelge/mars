---
id: qn95b
title: Run the task board, drawer and hand-off controls once against a live orchestrator before the Playwright scenarios exist
status: open
priority: P2
created: "2026-09-20T11:38:42.020811351Z"
updated: "2026-09-20T11:38:42.020811351Z"
tags:
  - frontend
  - tracker
  - tests
depends_on:
  - gn4y2
---

## Summary
Discovered while closing gn4y2 (Frontend task board, task detail and hand-off controls). Every task of that epic was verified by lint, `tsc -b`, build, Vitest (492 tests) and the Playwright smoke spec, but none of it has run against a real orchestrator: no agent had a stack up, and the machine had no `.env`, Postgres or stub image running. The types mirror `SPEC.md` and the services are thin, so the risk is in runtime behaviour the unit tests cannot see: the SSE open-before-load sequence, the reconnect with a refreshed token, the 400/409/422 bodies as the API really words them, and the drawer's refetch on task events.

## Documents
- `SPEC.md` "Frontend" (Task board, Task-board search, Board refresh ordering, Hand-off controls, Copy links), "SSE: task stream", "Code hand-offs and review", "Git" (task merge form, diff by `handoff_id`).
- `README.md` "Development", "Running locally".

## Acceptance criteria
- [ ] With Postgres, the orchestrator (`--features integration-tests`) and `npm run dev` running: create a project from a local bare repository, open the board tab, create tasks, see them arrive through the `created` event without a reload, and see a second browser context follow.
- [ ] States tab: add, rename, reorder and delete a state; the board columns follow; the four deletion refusals are disabled with their reasons.
- [ ] Drawer: open by URL before the board has loaded; comment; edit fields; move a held task and read the hand-off warning; release; add a dependency cycle and read the 409; delete.
- [ ] Hand-off: publish a revision at a synced session tip, then with a stale commit (409 shown verbatim); approve; merge the approved hand-off and read `Merged as <short> · Task state unchanged`; force a conflict and read the paths; view the diff of an older hand-off.
- [ ] Launch: "Open in session" and "Run once" on the stub image, with and without a base override; the disabled reasons for held, blocked, terminal and not-ready.
- [ ] Kill and restart the orchestrator: `Reconnecting` with the old snapshot kept, then one refresh on reopen with `after=` at the last sequence.
- [ ] Every defect found becomes its own Bears task; anything that is really a `SPEC.md` discrepancy is reported as such.

## Notes
The Playwright epic (6s8j7) automates most of these scenarios; this task exists so that defects surface before that epic starts writing assertions against behaviour nobody has watched.