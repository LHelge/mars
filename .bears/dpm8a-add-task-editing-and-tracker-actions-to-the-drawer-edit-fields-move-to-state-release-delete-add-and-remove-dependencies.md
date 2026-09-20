---
id: dpm8a
title: "Add task editing and tracker actions to the drawer: edit fields, move to state, release, delete, add and remove dependencies"
status: in_progress
priority: P1
created: "2026-09-16T20:43:46.251854665Z"
updated: "2026-09-20T10:51:50.686284872Z"
tags:
  - frontend
  - tracker
depends_on:
  - k4esd
parent: gn4y2
attempts: 1
---

## Summary
Make the drawer mutable: inline editing of title, description, priority, labels, assignee and parent through `PUT`, a `Move to` state action that hands the task off, `Release` for held tasks, `Delete`, and dependency add/remove by kind. Every action goes through `services/tasks`, surfaces the documented 400/409 errors, invalidates the detail query and calls `taskStore.invalidate()` so the board refreshes even before the SSE event lands.

## Documents
- `SPEC.md` "Tasks" (`PUT` body; state change semantics: different state hands off and clears the lease, current state is a no-op preserving lease/attempts; `parent_id` one-level rules with 400; `POST .../dependencies {depends_on, kind?}` 409 on cycle; `DELETE .../dependencies/{dep}?kind=`; `POST .../release` 409 if nobody holds; `DELETE` → 204; `priority` 0–3; labels pattern)
- `SPEC.md` "Frontend", "Task board" (actions: move to a state, release) and "Board refresh ordering" (a successful local mutation invalidates the view)
- `SPEC.md` "Code hand-offs and review" (a planning-only state move needs no hand-off and preserves the existing one)
- `ARCHITECTURE.md` "Task tracker", "The lease is the worker" (users can move, release and edit anything; a user move hands off like an agent's) and "Parents"
- `docs/data-model.md` `tasks` (assignee semantics), `task_dependencies` (kinds coexist)

## Acceptance criteria
- [ ] Edit mode (pencil action in the header) exposes `title` (1–200), `description` (textarea), `priority` (0–3 select), `labels` (validated chips), `assignee_user_id` (select from `GET /users` when the current user is admin, otherwise the current user or `unassigned`; `null` clears), `parent_id` (select from snapshot tasks without a parent, excluding the task itself and any task that has children; `null` clears). Save sends only changed fields via `updateTask`; 400 (e.g. nesting violation) is shown inline; success invalidates `taskKeys.detail(pid, number)` and calls `useTaskStore.getState().invalidate()`.
- [ ] `Move to` control: select of the project's states (from the store) with the current state marked; choosing a different state and confirming sends `updateTask(pid, number, { state })`. When the task is held, the confirmation text says `Moving hands the task off: the lease held by session <short id> is cleared and attempts reset`. Selecting the current state is disabled (no-op per the contract). No `handoff` is sent from this control (planning-only move).
- [ ] `Release` button enabled only when `lease_holder_session_id` is set; calls `releaseTask` (200 → `Task`); a 409 (`nobody holds it`) refetches and shows the message.
- [ ] `Delete` asks for confirmation (`Delete #<n> "<title>"? Dependants are unblocked and its hand-off refs removed.`), calls `deleteTask` (204), then navigates to the board and calls `invalidate()`.
- [ ] Dependencies: an `Add dependency` form with a task picker (search the board snapshot by `#number` or title using `filterTasks` from the search task, excluding the task itself) and a kind select (`blocks` default, `discovered_from`, `related`); submit `addDependency` → 200 `Task`; 409 (cycle) and 400 shown inline. Each listed dependency has a `Remove` action calling `removeDependency(pid, number, dep_task_id, kind)`; only that kind is removed.
- [ ] Every successful mutation: `queryClient.invalidateQueries({ queryKey: taskKeys.detail(pid, number) })` and `useTaskStore.getState().invalidate()`; the SSE event arriving later is deduped/coalesced by the store.
- [ ] While any mutation is pending its control is disabled and the others remain usable; concurrent conflicting edits surface as the API's 409/400 and refetch.
- [ ] `cd frontend && npm run lint && npx tsc -b && npm run build` pass.

## Implementation notes
- Files: `frontend/src/tasks/TaskEditForm.tsx`, `frontend/src/tasks/MoveToState.tsx`, `frontend/src/tasks/DependencyEditor.tsx`, edit `frontend/src/tasks/TaskDetail.tsx`, `frontend/src/tasks/DependencyList.tsx`.
- Use `useMutation` from TanStack Query for each action with `onSuccess` doing both invalidations; expose a small `useTaskMutations(pid, number)` hook returning the mutations so the hand-off and launch tasks reuse it.
- Error bodies are `{ status, error }`; show `error` verbatim in an `Alert` near the control that failed.
- Assignee resolution: non-admins cannot list users (`GET /users` is admin), so offer `me` (from `useAuth()`) and `unassigned`; admins get the full list via `services/users.listUsers`.
- Use the `/frontend-design` skill for the action bar layout: quiet secondary actions, destructive delete visually separated.

## Edge cases
- Moving to the `human` state from the UI is an ordinary user move: no `escalated` email is implied by the UI; do not special-case.
- Moving a closed task (terminal) to a non-terminal state reopens it; the parent is never reopened automatically; no client logic needed but the confirmation copy should not promise otherwise.
- Adding a `blocks` dependency on a terminal task is allowed and does not block; the indicator updates on refresh.
- Removing a `blocks` edge when a `discovered_from` edge exists for the same pair leaves the latter; the list shows both kinds separately so the user sees what remains.
- The task may be deleted by someone else mid-edit: a 404 from `PUT` shows the not-found state.

## Testing
- Vitest: `diffTaskInput(original, edited): UpdateTaskInput` (only changed fields, `null` for cleared assignee/parent) and the parent-candidate filter (excludes self, tasks with a parent, tasks with children).
- Manual: move a held task and watch the holder link disappear on the event; add a cycle (`A blocks B`, `B blocks A`) and see the 409.
- `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:unit`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- Frontend foundation epic: `useAuth()` (current user, admin flag), `services/users.ts` `listUsers` (admin) and `getUser`.
- Task tracker epic: the endpoints and statuses cited above.