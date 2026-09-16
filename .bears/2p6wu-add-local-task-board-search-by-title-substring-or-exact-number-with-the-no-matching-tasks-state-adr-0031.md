---
id: "2p6wu"
title: "Add local task-board search by title substring or exact #number with the No matching tasks state (ADR 0031)"
status: open
priority: P1
created: "2026-09-16T20:42:41.327883489Z"
updated: "2026-09-16T20:42:41.327883489Z"
tags:
  - frontend
  - tracker
depends_on:
  - "347vr"
parent: gn4y2
---

## Summary
Add the labelled search field above the board and the pure filtering function that derives visible cards from the complete task snapshot without touching the store's arrays, issuing requests or reconnecting SSE. Digit-only queries (optionally prefixed with `#`) match the exact per-project number; anything else is a case-insensitive title substring. The query survives refreshes, resets on project change and never closes an open drawer.

## Documents
- `SPEC.md` "Frontend", "Task-board search" (both paragraphs: placeholder `Search title or #number`, clear action, trimming, exact number vs substring, `No matching tasks`, distinguish from loading/error/empty, keep through refreshes, reset on project change, drawer and direct URL unaffected)
- `SPEC.md` "User-facing features", "Task board" (search sentence)
- `ARCHITECTURE.md` "Frontend architecture" (search derives from the complete snapshot; state local to the project view; reapplied after refreshes)
- ADR 0031

## Acceptance criteria
- [ ] `frontend/src/tasks/search.ts` exports `normalizeQuery(raw: string): string` (trim) and `filterTasks(tasks: Task[], query: string): Task[]`: empty query returns the same array reference; `/^#?\d+$/` matches `task.number === Number(digits)` only; otherwise `task.title.toLowerCase().includes(query.toLowerCase())`. Order of the input is preserved. Descriptions and comments are never inspected.
- [ ] `TaskBoard` renders `TaskSearch` above the columns: an `<input type="search">` with `<label>` text `Search`, placeholder `Search title or #number`, `aria-label="Search tasks"`, bound to `useTaskStore(s => s.query)` / `setQuery`, and a clear button (`Clear search`, shown only when non-empty) that resets the query and refocuses the field.
- [ ] Visible cards come from `selectVisibleColumns` = `selectColumns` with `filterTasks` applied per column; the store's `tasks`/`states` arrays are unchanged; column order and card order are retained; columns stay rendered when they have no matches.
- [ ] When `loaded`, the query is non-empty and every column is empty after filtering, the board shows `No matching tasks` with the clear action; this state is not shown while `!loaded`, when `error` is set without a snapshot, or for an empty project with an empty query (those keep the loading / error / `EmptyState` from the board task).
- [ ] Typing does not call `listTasks`, `listTaskStates`, `refresh()` or touch the `EventSource` (assert via the injected service mocks in the store test harness).
- [ ] The query persists across `noteEvent`-triggered refreshes and is cleared by `bindProject` to another project; navigating between `/projects/:id/tasks/:number` and the board of the same project keeps it.
- [ ] Vitest tests in `frontend/src/tasks/search.test.ts`: `42` and `#42` match task 42 only (not 142 or 420); ` #42 ` with whitespace matches; `login` matches `Fix Login Redirect`; `LOGIN` matches the same; empty and whitespace-only queries return all; `#` alone is a title search for `#`; a terminal-state task is matched; input order preserved.
- [ ] `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:unit` pass.

## Implementation notes
- Files: `frontend/src/tasks/search.ts`, `frontend/src/tasks/search.test.ts`, `frontend/src/tasks/TaskSearch.tsx`, edit `frontend/src/tasks/TaskBoard.tsx` and `frontend/src/tasks/taskStore.ts` (add `selectVisibleColumns`).
- `Number(digits)` on very long digit strings can overflow precision; compare as strings after stripping leading zeros? No: `SPEC.md` says exact number, so compare `String(task.number) === digits.replace(/^0+(?=\d)/, "")` to be safe for both `042` and huge inputs.
- The `No matching tasks` message is rendered inside the board area, below the (still visible, empty) columns or as an overlay row; keep columns to satisfy "preserve columns when there are no matches".

## Edge cases
- A task whose title is renamed by a live refresh to no longer match disappears; one created that matches appears; a deletion removes it. All fall out of re-deriving from the snapshot.
- Opening a task through its direct URL while the search hides its card still opens the drawer (drawer state is route-driven, not derived from visible cards).
- Search input must not steal keyboard focus on refresh re-renders: keep it uncontrolled-by-refresh (the store's `query` only changes on user input or project change).

## Testing
- Vitest suites above plus a store-harness test that `setQuery` triggers no service calls.
- `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:unit`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- none beyond this epic's board and store tasks.