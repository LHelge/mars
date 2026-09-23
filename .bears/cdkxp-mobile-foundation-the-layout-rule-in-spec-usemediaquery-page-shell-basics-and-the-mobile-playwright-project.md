---
id: cdkxp
title: "Mobile foundation: the layout rule in SPEC, useMediaQuery, page-shell basics and the mobile Playwright project"
status: open
priority: P1
created: "2026-09-23T15:55:11.397650604Z"
updated: "2026-09-23T15:55:11.397650604Z"
tags:
  - frontend
  - mobile
  - docs
  - tests
parent: yymt7
---

## Summary

Everything else in epic yymt7 implements a rule this task writes. It adds the "Mobile layout" sub-heading to `SPEC.md`, "Frontend", records the decision in an ADR and a paragraph in `ARCHITECTURE.md`, ships the one reactive width hook the views will share, sets the page-shell basics in `index.html` and `index.css`, and gives Playwright a `mobile` project so every following task can add its own phone scenario. No view changes here beyond `SidePanel` adopting the hook, which is the smallest proof that the hook works.

Implements `SPEC.md`, "Frontend", "Mobile layout" (new), and deletes the non-goal "Mobile layouts beyond 'does not break'" (`SPEC.md`, "Non-goals").

## Acceptance Criteria

- [ ] `SPEC.md`, "Frontend" has a **Mobile layout.** paragraph, placed after **Code splitting.**, that states: (1) one layout for every width, adapted at Tailwind's `sm` (640 px) and `lg` (1024 px) breakpoints, never a separate mobile route tree; (2) touch is `pointer-coarse:`, never inferred from width — hit areas of at least 44 px and text controls of at least 16 px on a coarse pointer, unchanged density on a fine one; (3) a `title` tooltip is never the only place a fact lives; (4) the terminal is desktop-first and only kept from breaking the layout; (5) one sentence per view naming what a phone gets, filled in by the tasks below and written here as the epic's outline (side panel as a sheet below `lg`, folded session header, board columns that snap one at a time, icon-only nav below `sm`, unified diff below `sm`).
- [ ] The line `- Mobile layouts beyond "does not break".` is removed from `SPEC.md`, "Non-goals".
- [ ] `ARCHITECTURE.md`, "Frontend architecture" gains a **One layout, adapted at the edges.** paragraph: breakpoints in class names, `useMediaQuery` for the one case where JavaScript needs the width (what is mounted), `pointer-coarse:` for touch, and why (ADR).
- [ ] `docs/decisions/0049-one-responsive-layout-no-mobile-routes.md` records the alternatives rejected: a separate mobile route tree or app shell, a PWA with its own shell, and a user-agent check instead of media queries.
- [ ] `src/hooks/useMediaQuery.ts` exports `useMediaQuery(query: string): boolean`, built on `useSyncExternalStore` over `window.matchMedia(query)` with a `change` subscription, `false` where `matchMedia` is missing (tests), exported from `src/hooks/index.ts`; a Vitest test beside it covers the subscription and the missing-`matchMedia` case.
- [ ] `SidePanel.tsx` replaces its one-shot `wideScreen()` initial state with the hook: the panel is open beside the transcript at `lg` and collapsed below, and follows a resize across the breakpoint (`SPEC.md`, "Session side panel" already says 1024 px; make it say `lg` and that it follows a resize). This is the only view change in this task; the sheet itself is task "Session side panel as a sheet".
- [ ] `index.html`: `viewport-fit=cover` on the viewport meta, one `theme-color` meta per colour scheme (`media="(prefers-color-scheme: dark)"` / `light`) using the `--color-console-bg` values of `index.css`.
- [ ] `index.css`: `-webkit-text-size-adjust: 100%` and `text-size-adjust: 100%` on `html`; `overscroll-behavior-y: none` on `body` (no pull-to-refresh over a live transcript); `-webkit-tap-highlight-color: transparent`; `padding: env(safe-area-inset-*)` on `#root` or `body` so a notch does not cover the header on a phone in landscape.
- [ ] `frontend/playwright.config.ts` has a second project `{ name: "mobile", use: { ...devices["Pixel 7"] }, grep: /@mobile/ }` and the `chromium` project gets `grepInvert: /@mobile/`, so a scenario tagged `@mobile` in its title runs on the phone project only; `tests/utils/browser.ts` or `test-helpers.ts` gains an `isMobile` fixture or helper (`testInfo.project.name === "mobile"`) for the scenarios that branch.
- [ ] `frontend/tests/README.md`: the "Running it" section explains the two projects and the `@mobile` tag (`npx playwright test --project mobile`), and the `SPEC.md`, "Frontend" coverage table gains a `Mobile layout` row pointing at the first `@mobile` scenario this task adds: `smoke.spec.ts` › `the console loads on a phone @mobile` (login, dashboard, no horizontal overflow: `document.documentElement.scrollWidth <= window.innerWidth`).
- [ ] `tests/coverage-check.mjs` still passes before and after the run with the tag in the title.

## Implementation Notes

- Read `src/session/SidePanel.tsx:28-46` (the `WIDE_PX` read) and `src/session/TerminalView.tsx:52,142` (the colour-scheme `matchMedia`, which the hook may also replace if it is a one-line change, otherwise leave it).
- `useSyncExternalStore` subscribe/get-snapshot: create the `MediaQueryList` once per query with `useMemo`; the server snapshot is `false`.
- The Playwright `grep` on a project: `projects: [{ name: "chromium", use: {...}, grepInvert: /@mobile/ }, { name: "mobile", use: {...devices["Pixel 7"]}, grep: /@mobile/ }]`. `workers: 1` and the shared `webServer` stay as they are. The stack (`npm run test:e2e:up`) is unchanged.
- `tests/git.spec.ts:200` relies on the side panel being open at the desktop viewport; it keeps working because `chromium` is still Desktop Chrome.
- Coverage table format: `| Mobile layout | \`smoke.spec.ts\` › \`the console loads on a phone @mobile\` |`. The check reads the title verbatim, tag included.

## Edge Cases

- `useMediaQuery` under Vitest/jsdom: `window.matchMedia` is undefined, the hook returns `false`, `SidePanel` starts collapsed; any unit test that expected the panel open must set a `matchMedia` stub (grep `SidePanel.test`).
- `viewport-fit=cover` without safe-area padding puts content under the notch; both go together.

## Testing

- Vitest for the hook; the existing `SidePanel` unit tests updated for the hook.
- `npm run lint && npx tsc -b && npm run build && npm run test:unit`, then `npm run test:e2e:up && npm run test:e2e; npm run test:e2e:down` with both projects.
- Invoke the `/frontend-design` skill before touching the page shell.