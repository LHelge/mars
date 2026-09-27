---
id: gw72u
title: "Phone walkthrough: every route fits a 360×740 viewport, and the mobile coverage is complete"
status: in_progress
priority: P1
created: "2026-09-23T15:58:57.112402674Z"
updated: "2026-09-27T17:37:10.884460624Z"
tags:
  - frontend
  - mobile
  - tests
  - docs
depends_on:
  - cdkxp
  - a48hj
  - yaxpf
  - zzrt4
  - x7agt
  - ub6y6
  - baenx
  - "235cs"
  - b9n9c
  - wk748
parent: yymt7
attempts: 1
---

## Summary

The epic's gate. Every task above added its own phone scenario; this one walks every route of `SPEC.md`, "Frontend", "Routes" on the `mobile` Playwright project, asserts none scrolls sideways, fixes whatever the earlier tasks left between them (two tasks that each looked right in their own worktree can still overlap on a phone: the folded session header and the sheet opener, the nav height and the session box's `6rem`), and closes the documentation: the "Mobile layout" paragraph reads as one piece rather than nine appended sentences, and the coverage row lists every `@mobile` scenario.

Implements `SPEC.md`, "Frontend", "Mobile layout" (the whole paragraph, as landed).

## Acceptance Criteria

- [ ] `smoke.spec.ts` › `every route fits a phone without horizontal scroll @mobile`: as an admin with one project (one session, one task, one secret, one shared directory, one extra profile with a schedule) visits `/`, `/projects`, `/projects/:id?tab=` for every tab of `PROJECT_TABS`, `/projects/:id/tasks/:number`, `/sessions/:id` (with the side-panel sheet open on each tab, including Terminal on a running stub session), `/secrets`, `/settings`, `/help`, `/admin`, and on each asserts `document.documentElement.scrollWidth <= window.innerWidth` and that the page's `main` has no descendant whose bounding box exceeds the viewport's right edge by more than 1 px (a list of offenders in the failure message).
- [ ] The same scenario also checks the auth pages unauthenticated: `/login`, `/forgot-password`, `/reset-password?token=x`, `/invite/x` (whatever the route table names).
- [ ] Every finding the walkthrough turns up is fixed in this task (small) or filed as a new Bears task linked to this one (large), and the list is in this task's closing comment.
- [ ] `SPEC.md`, "Frontend", "Mobile layout" is rewritten as one paragraph in the document's voice: the rules first (breakpoints, pointer, inputs, hit areas, tooltips), then one sentence per view (nav, tables, session view with header and sheet, composer, board and drawer, diff, terminal), then what is out of scope (PWA, offline, a key bar for the terminal). Each earlier task's sentence is merged, not appended.
- [ ] `frontend/tests/README.md`'s `Mobile layout` row lists every `@mobile` scenario, and `tests/coverage-check.mjs` passes before and after a full run of both projects.
- [ ] `README.md`, "Development", "End-to-end tests" mentions the two Playwright projects and `--project mobile` in one sentence.
- [ ] The whole frontend chain passes: `npm run lint && npx tsc -b && npm run build && npm run test:unit && npm run test:e2e:up && npm run test:e2e; npm run test:e2e:down`.

## Implementation Notes

- The overflow assertion is one helper in `tests/utils/test-helpers.ts` (`expectNoHorizontalOverflow(page)`), reused by the earlier scenarios if they wrote their own inline; make it the one spelling.
- Arranging the fixtures is the existing `user`, `api`, `repo`, `project` and `sessions` fixtures of `tests/utils/fixtures.ts`; nothing sleeps.
- A screenshot per route at 360×740 into `test-results/` on failure is cheap and worth having (`page.screenshot` in the failure path only, or `trace: retain-on-failure` already covers it — check before adding).

## Edge Cases

- The Terminal tab on the phone project opens a `/bin/bash -l` exec in the stub container: the scenario ends the session through the `sessions` tracker as every other does.
- `HelpPage` at 360 px: the link list wraps (`HelpPage.tsx:46-60`); long Markdown tables inside the help topics are inside `Markdown.tsx`'s scroller.

## Testing

- The scenario above is the test. Run it on both projects to confirm `grepInvert` keeps it off `chromium`.