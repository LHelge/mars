---
id: yymt7
title: "Mobile-friendly frontend: the console works on a phone"
type: epic
status: done
priority: P1
created: "2026-09-23T15:54:35.312767689Z"
updated: "2026-09-27T18:30:33.618174592Z"
tags:
  - frontend
  - mobile
  - docs
  - tests
---

## Scope

Mars is used from a phone to keep an eye on things: watch a session run, answer it in the composer, look at the board, open a task, approve or send back a hand-off, glance at the dashboard. Today the session view is unusable below about 1024 px (the open side panel takes the whole row, the header can squeeze the transcript to nothing), every input zooms iOS on focus, the composer cannot take a newline on a touch keyboard, three tables push the page sideways, several facts live only in `title` tooltips a phone cannot show, and the touch targets are 14–24 px. `SPEC.md`, "Non-goals" still says "Mobile layouts beyond 'does not break'".

This epic makes those flows work on a 360–430 px wide touch screen without a second layout tree: one responsive layout, adapted at Tailwind's `sm` (640) and `lg` (1024) breakpoints for width and at `pointer-coarse:` for touch. It does not build a PWA, a native shell, offline support or a phone-first redesign of the transcript. The terminal stays desktop-first (no modifier-key bar); the epic only stops it breaking the layout.

Audit of the code as of 2026-09-23 (file:line references are in each task): responsive work already exists in the tables (columns hidden at `sm`/`md`/`lg`), the forms (stack below `sm`), the nav (help label and username hidden below `sm`) and the help page (`lg` sidebar). What has none: `SessionView`, `SessionHeader`, `SidePanel` (a one-shot `matchMedia` read at 1024 px), `Composer`, `TaskBoard`, `TaskCard`, `ProjectTabs`, `DiffView`.

## Order

Four rounds. The foundation task (cdkxp) lands first: it writes the rule the other tasks implement (`SPEC.md`, "Frontend", "Mobile layout"), the `useMediaQuery` hook, the page-shell basics and the `mobile` Playwright project every other task adds its scenario to. The touch-controls task (a48hj) is second on its own, because it touches the shared style constants and one line in most of the files the other tasks reshape, and a merge is cheaper than seven rebases over it. Then seven tasks run in parallel; the session header (zzrt4) follows the side-panel sheet (yaxpf) because it reuses the sheet frame and the same header row. The walkthrough (gw72u) closes the epic and is its gate.

## Acceptance Criteria

- [ ] `SPEC.md`, "Frontend" has a "Mobile layout" sub-heading stating the breakpoints, the pointer rule, the input size rule, the tooltip rule and what each view does on a phone; the "Mobile layouts beyond 'does not break'" non-goal is gone; `ARCHITECTURE.md`, "Frontend architecture" has the one-layout paragraph; an ADR records the rejected alternatives.
- [ ] On a 360×740 viewport: no route scrolls sideways, the session view shows the transcript and the composer with the side panel as a sheet over them, a newline can be typed in the composer, the board's columns snap one at a time, the task drawer keeps a draft behind a confirmation, and every text input is 16 px on a coarse pointer.
- [ ] Every interactive control has a 44 px hit area on a coarse pointer without changing the desktop density.
- [ ] No fact is only in a `title` tooltip.
- [ ] Playwright runs a `mobile` project (Pixel-class device) over the scenarios tagged `@mobile`, `frontend/tests/README.md` carries the "Mobile layout" coverage row, and `npm run test:e2e` runs both projects.
- [ ] `npm run lint && npx tsc -b && npm run build && npm run test:unit && npm run test:e2e` pass; the first-paint byte budget of `scripts/check-entry-chunk.mjs` still holds.