---
id: "9udyu"
title: Code-split the frontend by route so the session view's markdown, virtualizer and xterm load on demand
status: open
priority: P3
created: "2026-09-20T08:35:21.475715524Z"
updated: "2026-09-20T08:35:21.475715524Z"
tags:
  - frontend
  - perf
depends_on:
  - dyr6h
  - frhcc
  - zcj6p
parent: cgdc2
---

## Summary
Found while verifying k97mz: `npm run build` now emits one 608 kB JS chunk (178 kB gzip) and Vite's "chunks larger than 500 kB" warning, because `SessionPage` brought `react-markdown` and `@tanstack/react-virtual` into the entry bundle; `TerminalView` adds `@xterm/xterm`. The login page and dashboard pay for all of it.

## Documents
- `SPEC.md` "Frontend" (stack and routes); `README.md` "Development" (the nginx image builds with `npm run build`).

## Acceptance criteria
- [ ] Route components that are not needed for first paint are loaded with `React.lazy` + `Suspense` (at least `SessionPage`, `ProjectPage`, `AdminPage`, `SecretsPage`), with `LoadingState` as the fallback inside `PageLayout`; `TerminalView` (xterm and its CSS) is lazy within the session view so opening a session does not load it until the Terminal tab is opened.
- [ ] `npm run build` prints no chunk-size warning without raising `build.chunkSizeWarningLimit`; the entry chunk is reported in the task's closing note.
- [ ] Named exports are kept (CLAUDE.md): lazy imports map the named export (`import("./pages/SessionPage").then(m => ({ default: m.SessionPage }))`).
- [ ] The Playwright smoke test and unit tests still pass; `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:unit && npm run test:e2e`.

## Documentation
- none unless a build option is added to `README.md`'s development notes.
