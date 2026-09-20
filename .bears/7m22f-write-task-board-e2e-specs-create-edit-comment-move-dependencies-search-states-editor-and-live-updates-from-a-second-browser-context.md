---
id: "7m22f"
title: "Write task board E2E specs: create, edit, comment, move, dependencies, search, states editor and live updates from a second browser context"
status: in_progress
priority: P1
created: "2026-09-16T20:44:41.033096060Z"
updated: "2026-09-20T17:26:54.244438674Z"
tags:
  - frontend
  - tracker
  - tests
depends_on:
  - ku8up
parent: "6s8j7"
attempts: 1
---

## Summary
Cover the "Task board" feature paragraph without sessions: creating tasks that land in `backlog`, editing, commenting, moving cards across the project's columns from the drawer, `blocks` dependencies and the blocked indicator, parent/child closure, board search by title and exact `#number`, the task-states editor with its refusals, deep links to a task drawer, `Copy link`, and live updates: a second browser context sees a task created, moved and a state renamed by the first without reloading, through SSE-triggered refreshes.

## Documents
- `SPEC.md` "User-facing features", "Task board" paragraph.
- `SPEC.md` "Tasks" table and paragraphs (`POST /projects/{pid}/tasks` → 201 in `backlog` by default; `PUT` with `state` hands off, terminal state sets `closed_at`; `POST .../dependencies` `{depends_on, kind?}` → 409 on cycle; `DELETE .../dependencies/{dep}?kind=`; `POST .../comments`; `priority` 0–3 default 2; parent nesting one level, 400 otherwise; a non-terminal child blocks its parent and the last closing child closes the parent via `system`; `{id}` accepts UUID or number).
- `SPEC.md` "Task states" table (`POST` → 201, 409 duplicate or second `human`; `PUT /{name}` rename/reorder, 400 if `kind` given; `DELETE` → 409 while any task is in the state, for the `human` state, the last `queue` and the last `terminal`; every change emits `states_changed`).
- `SPEC.md` "TaskEvent" (kinds; `states_changed` refreshes columns and cards), "SSE: task stream".
- `SPEC.md` "Frontend": "Task board", "Task-board search" (placeholder `Search title or #number`, `#42`/`42` match 42 only, case-insensitive title substring, `No matching tasks` with clear action, query survives refreshes, resets on project change), "Board refresh ordering" (ADR 0022), "Copy links" (`/projects/{project_id}/tasks/{number}`, `Link copied`).
- `docs/data-model.md` `task_states` default set: `backlog`, `ready`, `review`, `merge`, `needs_human`, `done`, `cancelled`.

## Acceptance criteria
- [ ] `frontend/tests/tasks.spec.ts`; each test: fresh user, bare repo, ready project (via helpers), page at `/projects/:id` board tab.
- [ ] `columns show the default states in order` and an empty project shows the board's empty state, not `No matching tasks`.
- [ ] `create a task from the board form`: title `Write docs`, priority 1, labels `docs`; the card appears in `backlog` with `#1`, the priority marker and the label; the drawer opens on click and shows description, empty comments, no dependencies.
- [ ] `edit and comment`: change title and description in the drawer; add a comment; both persist after reload; system comments (none yet) are absent.
- [ ] `move across columns`: drawer "move to" `ready` → card in `ready`; then `done` → card in `done`, `closed_at` shown; then `backlog` → reopened (no `closed_at`).
- [ ] `dependencies block and unblock`: tasks A and B; add `blocks` on B depending on A; B's card shows the blocked indicator and the dependency count; drawer lists A under `blocks`; moving A to `done` clears B's blocked indicator live; adding A depends on B now (would not cycle since A→B removed? create the cycle explicitly: A depends on B while B depends on A) → the 409 message is shown and no edge is added.
- [ ] `parent closes when its last child closes`: parent P with children C1, C2 (`parent_id`); P shows the blocked indicator and a children list; move C1 and C2 to `done`; P moves to `done` by itself (system `state_changed`) and its drawer shows a system-styled comment or history line if the frontend renders one.
- [ ] `search`: tasks `Fix Login Redirect` (#1), `login page copy` (#2), `Other` (#3) plus tasks up to #14 and #142 are not needed—create #1..#3 then assert: `login` shows #1 and #2 only, `#2` and `2` show #2 only, `xyz` shows `No matching tasks` with columns still rendered, clear restores all; the query survives a task creation from the API (refresh) and is reset when navigating to another project.
- [ ] `deep link and copy link`: open `/projects/:id/tasks/2` directly → the board with #2's drawer open; `Copy link` writes `http://localhost:5173/projects/<id>/tasks/2` (grant `clipboard-read`/`clipboard-write` permissions on the context and read `navigator.clipboard.readText()`) and shows `Link copied`.
- [ ] `states editor`: add `qa` (kind `queue`) at position after `review` → new column; rename `qa` to `verify` → column header and any task in it follow; delete refused (409 text) for `needs_human`, for a state with a task in it, for the last `terminal` after deleting `cancelled`; delete `verify` when empty succeeds.
- [ ] `live updates in a second browser context`: user U1 (context A) and U2 (context B, `newLoggedInPage`) both on the board; A creates a task → B shows the card within 5 s without reload; A moves it to `ready` → B's column updates; A renames `ready` to `todo` → B's column header changes; A deletes the task → B's card disappears. Assert B performed no full navigation (`page.url()` unchanged, no `load` event) by installing a `page.on("load")` counter.
- [ ] `release from the drawer is disabled when nobody holds the task` (the held case lives in the task-session spec).

## Implementation notes
- Files: `frontend/tests/tasks.spec.ts`.
- Cards and columns should be found by accessible names (`getByRole("region", { name: "backlog" })` or `getByTestId("column-backlog")`); the board epic is asked, through the coverage task's test-id list, to expose `data-testid="task-card-<number>"` and `data-testid="column-<state>"` if role queries prove ambiguous.
- Use `page.context().grantPermissions(["clipboard-read", "clipboard-write"])` for the copy-link test; Chromium only, which is the configured project.
- Parent/child closure and the 409 cycle text come from the backend; assert on substrings that `SPEC.md` implies (`cycle`, `last`, `human`) rather than exact sentences, and tighten once the tracker epic fixes messages.

## Edge cases
- Board refresh coalescing means a burst of events yields one refresh; the second-context assertions must wait with `expect.poll`/`toBeVisible` timeouts of 5–10 s, never assert immediacy.
- Search by `#2` must not match `#12` when it exists; create 12 tasks in the search test through the API to prove it (cheap: 12 POSTs).
- `states_changed` after rename: the query string of an open search must persist (assert the input value after the rename refresh).

## Testing
- The spec file; run twice against one stack.
- Command: `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:e2e`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Frontend task board, task detail and hand-off controls": `TaskBoard`, `TaskCard`, `TaskDetail` drawer with move/comment/dependency/delete actions, search field, `TaskStatesEditor`, `Copy link`, `useTaskStream` refresh path.
- "Task tracker" and "Real-time delivery": task routes, `states_changed`, SSE stream with replay.