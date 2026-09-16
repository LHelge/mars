---
id: "6s8j7"
title: End-to-end tests with Playwright
type: epic
status: open
priority: P2
created: "2026-09-16T20:14:55.298493493Z"
updated: "2026-09-16T20:16:02.986200849Z"
tags:
  - frontend
  - orchestrator
  - infra
  - tests
depends_on:
  - cgdc2
  - gn4y2
  - qgj33
---

## Scope

The Playwright suite and its CI workflow.

- Harness: `workers: 1`, a real orchestrator built with `--features integration-tests`, Postgres, the stub session image on a real engine, fresh users per test through `POST /api/test/users`, helpers in `tests/utils/test-helpers.ts`.
- Scenarios: login and forced password change; invite acceptance via the logged link; project creation from a local bare repository reaching `ready`; launching a conversational session on the stub image, watching the replayed transcript, sending a message, stopping and resuming; creating tasks, moving them across columns, live updates from a second browser context; launching a session for a task and seeing the claim on the card; a revision hand-off, review approval and task merge; secrets manager round trip; admin user management.
- E2E CI workflow triggered on `orchestrator/**` or `frontend/**`.

## Documents

`CLAUDE.md` "Testing expectations" (Frontend E2E); `README.md` "CI"; `ARCHITECTURE.md` "Session image" (stub); `SPEC.md` "Test-only routes".

## Acceptance criteria

- [ ] `npm run test:e2e` passes locally against the documented setup and in CI.
- [ ] Each user-facing feature paragraph in `SPEC.md` has at least one scenario.

## Out of scope

Engine tests and backend integration tests (their own epics).