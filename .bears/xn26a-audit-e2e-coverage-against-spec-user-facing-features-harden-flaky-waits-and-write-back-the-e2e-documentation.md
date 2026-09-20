---
id: xn26a
title: Audit E2E coverage against SPEC user-facing features, harden flaky waits and write back the E2E documentation
status: open
priority: P2
created: "2026-09-16T20:48:02.931989676Z"
updated: "2026-09-20T18:50:44.488937122Z"
tags:
  - frontend
  - docs
  - tests
depends_on:
  - svv8w
  - rsk7c
  - hez8r
  - sey9x
  - "2yeth"
  - h3uux
  - qpybb
  - dy7gr
  - dthk8
  - qhyhw
  - g4s53
  - "77ue3"
  - nvjt5
parent: "6s8j7"
---

## Summary
Close the epic: prove that every paragraph of `SPEC.md` "User-facing features" has at least one Playwright scenario, run the whole suite repeatedly against the stack and in CI to shake out timing-dependent failures, consolidate the `data-testid` hooks the specs relied on, and write the E2E section of the documentation so a newcomer can run and extend the suite. No new feature scenarios are added here except gaps the audit finds.

## Documents
- Epic acceptance criteria: `npm run test:e2e` passes locally against the documented setup and in CI; each user-facing feature paragraph in `SPEC.md` has at least one scenario.
- `SPEC.md` "User-facing features" (paragraphs: Login and invites, Projects, Agent profiles, Sessions, Task board, Git operations, Secrets, Users (admin), Dashboard).
- `CLAUDE.md` rule 1 (documentation is part of the change), "Testing expectations" (Frontend E2E), "Code quality" (frontend chain).
- `README.md` "Development" (End-to-end tests subsection from the stack task), "CI".

## Acceptance criteria
- [ ] `frontend/tests/README.md` contains a coverage table: one row per `SPEC.md` "User-facing features" paragraph and per `SPEC.md` "Frontend" sub-heading (Copy links, Dashboard, Session state, Transcript rendering, Changes panel, Task board, Task-board search, Board refresh ordering, Hand-off controls, Composer), naming the spec file and test title that covers it, or "backend integration test" / "not covered in E2E: <reason>" for the documented exclusions (login throttling, SIGTERM "killed" rendering, GitHub compare link if skipped).
- [ ] Every paragraph row has at least one E2E scenario; any gap found is closed in this task with a test added to the appropriate spec file (or a new task filed and linked with `discovered_from` if larger than an hour).
- [ ] The suite passes 3 consecutive runs locally against one stack (`for i in 1 2 3; do npm run test:e2e || exit 1; done`) and the CI run on `main` shows no retried tests in the Playwright report; every `waitForTimeout` call in `frontend/tests/` is removed or justified by a comment naming the timing it waits for.
- [ ] Spec files share `tests/utils/fixtures.ts`: a Playwright `test.extend` fixture providing `user`, `api`, `repo`, `project` and an auto-cleanup of sessions (ends any `running`/`parked` session created through `launchSession`) so `afterEach` boilerplate is removed from the spec files.
- [ ] A `tests/utils/test-ids.ts` module lists every `data-testid` the suite depends on as constants, and the frontend components use the same constants (imported from a shared `src/utils/testIds.ts` re-exported to tests) so a rename fails type-checking rather than a test.
- [ ] Total wall-clock time of `npm run test:e2e` against a warm stack is recorded in `frontend/tests/README.md` together with the machine class, and CI stays under its `timeout-minutes`.
- [ ] `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:e2e` passes; the E2E workflow is green.

## Implementation notes
- Files: `frontend/tests/README.md`, `frontend/tests/utils/fixtures.ts`, `frontend/tests/utils/test-ids.ts`, `frontend/src/utils/testIds.ts`, edits to the spec files to adopt the fixtures, `README.md`, `CLAUDE.md` only if a command changed.
- Flake hunting: prefer `expect(locator).toBeVisible({ timeout })` and `expect.poll` over fixed sleeps; for container-bound waits use `waitForSessionState` with explicit budgets; make sure every spec that launches a session cleans up so the stack does not accumulate containers across runs (check `podman ps -a --filter label=mars.session_id` is empty after a run).
- Keep the coverage table hand-written but verify it with a small script (`node tests/coverage-check.mjs`) that greps spec titles listed in the table and fails on a missing one; wire it into `npm run test:e2e` as a pre-step or into the CI workflow.

## Edge cases
- Tests that were `test.skip`ped with a reason (scroll-up pagination, idle-park timeout, compare link) are listed in the coverage table as exclusions with the reason, and a follow-up task is filed if the reason is a missing product capability rather than a test constraint.
- The seeded-admin forced-change scenario depends on a fresh database; the README must say that a rerun without `test:e2e:up` skips it.

## Testing
- Three consecutive local runs and one CI run; the coverage check script.
- Command: `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:e2e`.

## Documentation
- `README.md` "Development", "End-to-end tests": link to `frontend/tests/README.md`, note the fresh-database requirement and the `E2E_*` knobs; `README.md` "CI" E2E row verified; `CLAUDE.md` "Testing expectations" Frontend E2E bullet amended only if the helper module path or the stack command changed (same commit).

## Assumes from other epics
- All frontend epics have landed their pages; every spec task in this epic is done.