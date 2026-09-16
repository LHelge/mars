---
id: "347vr"
title: Build TaskBoard columns, TaskCard, the create-task form and the TasksPage route mounting the board
status: open
priority: P1
created: "2026-09-16T20:42:14.186504373Z"
updated: "2026-09-16T20:42:14.186504373Z"
tags:
  - frontend
  - tracker
depends_on:
  - "6s3j3"
parent: gn4y2
---

## Summary
Render the per-project board: `TaskBoard` with one column per task state in `position` order fed from `taskStore`, `TaskCard` showing the documented indicators, a create-task form, and `pages/TasksPage.tsx` as the route-level component that mounts `useTaskStream` and the board for `/projects/:id/tasks/:number` (drawer added by a later task) and is also what `ProjectPage`'s board tab renders. Loading, refreshing/reconnecting, error-with-retry and empty states are distinguished exactly as the refresh-ordering rules require.

## Documents
- `SPEC.md` "Frontend", "Task board" (columns in `position` order; card fields: priority, labels, holding session with link, attempts when above one, assignee, blocked and dependency indicators, parent badge)
- `SPEC.md` "Frontend", "Board refresh ordering" (keep the previous snapshot visibly refreshing/reconnecting while stale, loading state before the first successful load, error/retry state never installs an empty board)
- `SPEC.md` "Frontend", routes (`/projects/:id` tabs incl. board; `/projects/:id/tasks/:number`)
- `SPEC.md` "Tasks" (`POST /projects/{pid}/tasks` body and 400 for unknown state; `Task` fields; `priority` 0 critical to 3 low, default 2; labels pattern)
- `SPEC.md` "User-facing features", "Task board"
- `CLAUDE.md` "Frontend conventions" (operator console tone; `/frontend-design` skill before shaping UI)

## Acceptance criteria
- [ ] `frontend/src/pages/TasksPage.tsx` (named export) reads `:id` from the route, calls `useTaskStream(projectId)` and renders `<TaskBoard projectId=.../>`; `ProjectPage`'s board tab renders the same `TaskBoard` inside its own `useTaskStream` call (one stream per mounted view; the store is shared).
- [ ] `frontend/src/tasks/TaskBoard.tsx` uses `selectColumns` to render a horizontally scrolling row of columns, each headed by the state name, a kind marker (`human` and `terminal` states quietly distinguished) and the card count; cards in the API order (priority, then number).
- [ ] Board status line: `LoadingState` while `!loaded && loading`; `Alert` with a `Retry` button calling `refresh()` when `error` is set and `!loaded`; when `loaded && error` the previous snapshot stays visible with an inline error and `Retry`; a small `Refreshing` / `Reconnecting` indicator when `loading` or `stream !== "live"` while `loaded`. An empty project (`loaded && tasks.length === 0`) shows `EmptyState` with the create action, still rendering the columns.
- [ ] `frontend/src/tasks/TaskCard.tsx` shows: `#<number>` and title (link to `/projects/{pid}/tasks/{number}`), priority as `P0`–`P3` with colour only for `P0`/`P1`, labels as chips, the holding session as a link to `/sessions/{lease_holder_session_id}` when set (label `held`), `attempts` only when `> 1` (`3 attempts`), assignee username (resolved through `useQuery(["users", id], () => getUser(id))` with `staleTime: Infinity`, falling back to the first 8 characters of the id while loading), a `blocked` marker when `task.blocked`, a dependency indicator with the count of `depends_on` entries of kind `blocks` and of `blocks` (e.g. `↑2 ↓1` with `title` text explaining), a parent badge `part of #<n>` resolved from the snapshot via `selectTaskById(parent_id)`, and `needs_human_reason` as a one-line note when set.
- [ ] Create-task form (opened from a `New task` button in the board header): `title` (1–200, required), `description` (textarea, markdown), `state` select (defaults to the first `queue` state in position order), `priority` select (default 2), `labels` (comma/space separated, each validated against `^[a-z0-9][a-z0-9_-]{0,31}$`), optional parent (choose from snapshot tasks that have no `parent_id`), optional `depends_on` (multi-select from snapshot); submits `createTask` (201), then `useTaskStore.getState().invalidate()` and closes; 400 shown via `useFormSubmit`.
- [ ] Route `/projects/:id/tasks/:number` is registered in `App.tsx` pointing at `TasksPage` (the drawer task extends it); it is a `ProtectedRoute`.
- [ ] `cd frontend && npm run lint && npx tsc -b && npm run build` pass.

## Implementation notes
- Files: `frontend/src/pages/TasksPage.tsx`, `frontend/src/tasks/TaskBoard.tsx`, `frontend/src/tasks/TaskCard.tsx`, `frontend/src/tasks/CreateTaskForm.tsx`, edit `frontend/src/App.tsx`, `frontend/src/pages/ProjectPage.tsx` (board tab mount), `frontend/src/tasks/index.ts`.
- Invoke the `/frontend-design` skill before shaping the board and card: dense operator console, dark-friendly, monospace for `#number`, commit ids and labels, colour reserved for state (blocked, held, human state column, P0/P1).
- Cards are plain links; drag-and-drop is not part of v1 (moves happen in the drawer).
- `getUser(id)` is `GET /users/{id}` (JWT, any user) from the foundation epic's `services/users.ts`; add it there if missing.
- Keep the store subscription narrow: `useTaskStore(selectColumns)` with a shallow comparator so a refresh that yields identical arrays does not re-render every card; use `useShallow` from `zustand/react/shallow`.

## Edge cases
- A task whose `state` name is not among the current states (only possible mid-refresh): render it in a trailing `unknown` column rather than dropping it; it disappears on the next consistent snapshot.
- Parent badge for a parent not in the snapshot (deleted between refreshes): show `part of ?` until the next refresh.
- Label input rejects invalid entries client-side with the same message as the states editor.
- The create form's `state` select must list only current state names; an unknown state is a 400 the form surfaces.

## Testing
- Vitest: `selectColumns` ordering (position order, tasks kept in API order, unknown-state bucket) and the label parser/validator.
- Manual against a running orchestrator: create a task, see it appear on the resulting `created` event; kill the orchestrator to see the reconnecting indicator with the old snapshot still visible.
- `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:unit`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- Frontend project and session views epic: `pages/ProjectPage.tsx` with a board tab slot; how it selects tabs (query param or nested path) is its call; this task only exports `TaskBoard` and `TasksPage`.
- Frontend foundation epic: `ProtectedRoute`, `PageLayout`, shared components, `services/users.ts` (`getUser`).
- Task tracker epic: `GET/POST /projects/{pid}/tasks`, `GET /projects/{pid}/task-states`.