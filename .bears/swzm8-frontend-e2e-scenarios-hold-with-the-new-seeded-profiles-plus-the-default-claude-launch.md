---
id: swzm8
title: "Frontend E2E: scenarios hold with the new seeded profiles, plus the default `claude` launch"
status: open
priority: P2
created: "2026-09-25T09:17:34.610205Z"
updated: "2026-09-25T09:17:40.753251Z"
tags:
  - frontend
  - tests
  - profiles
depends_on:
  - "35xvy"
  - hwcn9
parent: xz6yq
---

The Playwright half of epic `xz6yq` (read it first). Every new project now seeds `claude` (conversational, default, serves nothing), `planner`, and `implementer` and `reviewer` as ephemeral with `auto_launch`.

- Find every scenario in `frontend/tests/` that assumed the old set: the default profile being `implementer`; a drawer launch on a `ready` or `review` task opening a conversational session with a composer; three profiles in the Profiles tab; seeded profiles never being dispatched (`dispatcher.spec.ts`, `automerge.spec.ts`, `task-sessions.spec.ts`, `handoffs.spec.ts`, `projects.spec.ts`). Arrange what a scenario needs explicitly through `tests/utils/fixtures.ts` and `test-helpers.ts` rather than relying on seeded profiles.
- **Cross-test interference.** The e2e stack runs the dispatcher (`DISPATCHER_INTERVAL_SECS=10`). If any scenario stores a `global`-scope agent credential, every project on the instance with a task in `ready` or `review` now has seeded auto-launch profiles the dispatcher will act on. Find out whether that happens and make scenarios independent of it. For example, pause automation or turn `auto_launch` off in projects that must not be dispatched, or keep credentials at project scope and remove them in teardown. Pick one approach and write it down in `frontend/tests/README.md`.
- New scenario: on a fresh project, the launch form preselects `claude`, and launching gives a conversational session with a composer. Add its row to the coverage table in `frontend/tests/README.md`. A new `data-testid` goes in `src/utils/testIds.ts`.
- Nothing sleeps for a fixed period. Chain: `npm run test:e2e:up && npm run test:e2e; npm run test:e2e:down`.