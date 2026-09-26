---
id: s257z
title: "Let the E2E suite run repeatedly against one stack: reset the login throttle through a test-only route"
status: in_progress
priority: P3
created: "2026-09-20T22:59:28.290265782Z"
updated: "2026-09-26T17:16:40.935903603Z"
tags:
  - orchestrator
  - frontend
  - tests
attempts: 1
---

## Summary
Discovered by xn26a (E2E audit, epic 6s8j7). The orchestrator's login throttle blocks a client address after 10 failed logins in 15 minutes (`orchestrator/src/routes/throttle.rs`, `LOGIN_FAILURE_LIMIT`). The Playwright suite makes three deliberate failed logins per run (wrong password, deleted user, replaced password after a reset) and a fourth on every rerun, because the probe that decides whether the seeded `admin`/`changeme` account is still fresh is itself a failed login. Three consecutive full runs against one stack therefore reach 3 + 4 + 4 = 11 failures, and the third run fails with `Too many failed attempts. Try again in 15 minutes.` Today the rule is "at most two full runs per `npm run test:e2e:up`" (`frontend/tests/README.md`, `README.md` "End-to-end tests").

## Documents
- `SPEC.md` "Test-only routes", "Authentication" (login throttling); `frontend/tests/README.md`; `README.md` "Development", "End-to-end tests".

## Acceptance criteria
- [ ] A test-only route under `integration-tests` (for example `POST /api/test/throttle/reset`) clears the login throttle; it does not exist in a release build, and `SPEC.md` "Test-only routes" documents it in the same commit.
- [ ] The suite calls it once at the start of a run (a Playwright global setup, or the `auth.spec.ts` seeded-admin probe replaced by a check that is not a failed login).
- [ ] `for i in 1 2 3; do npm run test:e2e || exit 1; done` passes against one stack; the fresh-stack rule is removed from both documents.
- [ ] Backend test for the route (happy path; absent without the feature is covered by the build); both quality chains pass.
