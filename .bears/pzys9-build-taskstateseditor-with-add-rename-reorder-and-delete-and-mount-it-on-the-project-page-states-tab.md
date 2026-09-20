---
id: pzys9
title: Build TaskStatesEditor with add, rename, reorder and delete and mount it on the project page states tab
status: done
priority: P2
created: "2026-09-16T20:41:06.670901964Z"
updated: "2026-09-20T10:20:20.010834040Z"
tags:
  - frontend
  - tracker
depends_on:
  - zazd5
parent: gn4y2
attempts: 1
---

## Summary
Deliver `frontend/src/tasks/TaskStatesEditor.tsx`, the project-page tab that lists a project's task states in `position` order and lets a user add, rename, reorder and delete them through `services/taskStates`, surfacing the API's deletion refusals as disabled actions with a reason rather than as errors after the fact. It is mounted on the `ProjectPage` "states" tab delivered by the Frontend project and session views epic.

## Documents
- `SPEC.md` "Task states" (endpoints, statuses, `TaskState` shape, name rule `[a-z0-9][a-z0-9_-]*` 1–32, kind immutable, `position` semantics, `states_changed` event)
- `SPEC.md` "Frontend", "Task board" (last sentence: `TaskStatesEditor` lists, adds, renames, reorders and removes states, deletion rules surfaced as disabled actions)
- `SPEC.md` "User-facing features", "Task board" (users edit the state list itself)
- `docs/data-model.md` `task_states` (default set, one human state, refusals)
- `ARCHITECTURE.md` "Task tracker", "State is a queue" (at least one queue, exactly one human, at least one terminal)

## Acceptance criteria
- [ ] `TaskStatesEditor({ projectId })` loads states with `useQuery(taskStateKeys.list(pid), () => listTaskStates(pid))` and tasks with `useQuery(taskKeys.all(pid), () => listTasks(pid))`, shows `LoadingState` / `Alert` on error, and lists states in `position` order with name, a kind badge (`queue` / `human` / `terminal`) and the count of tasks currently in the state.
- [ ] Add form: name input (validated client-side against `^[a-z0-9][a-z0-9_-]{0,31}$`, error text `Name must be 1–32 characters of a-z, 0-9, _ or -`), kind select (`queue` default; `human` option disabled with reason `This project already has a human state` when one exists), optional position; submits `createTaskState` (201). 400 and 409 responses are shown through `useFormSubmit`'s error state verbatim (`error` field of the body).
- [ ] Rename: inline edit of the name submitting `updateTaskState(pid, oldName, { name })`; 409 (`name taken`) shown inline; kind is never sent (400 otherwise).
- [ ] Reorder: move-up / move-down buttons submit `updateTaskState(pid, name, { position: newPosition })`; first row's up and last row's down are disabled.
- [ ] Delete is a disabled button with a tooltip/`title` reason when: `kind === "human"` (`The human state cannot be deleted`), it is the only `queue` state (`The last queue state cannot be deleted`), it is the only `terminal` state (`The last terminal state cannot be deleted`), or any task is in it (`N task(s) are in this state`). Otherwise it confirms and calls `deleteTaskState` (204). A 409 that still arrives (race) is shown as an `Alert` and both queries refetched.
- [ ] After every successful mutation both queries are invalidated (`taskStateKeys.list(pid)`, `taskKeys.all(pid)`) and, if the board store is bound to this project (`useTaskStore.getState().projectId === pid`), `invalidate()` is called on it; the `states_changed` SSE event will refresh the board as well.
- [ ] Mounted in `ProjectPage`'s states tab (named export used from `pages/ProjectPage.tsx`).
- [ ] `cd frontend && npm run lint && npx tsc -b && npm run build` pass.

## Implementation notes
- Files: `frontend/src/tasks/TaskStatesEditor.tsx`, edit `frontend/src/pages/ProjectPage.tsx` (tab mount), `frontend/src/tasks/index.ts`.
- Use `FormField`, `SubmitButton`, `Alert`, `LoadingState`, `EmptyState`, `SectionHeader` from `components/` and `useFormSubmit()` for the add form.
- Task counts per state: group the task list by `task.state` (state name). Because the list is not paginated (`SPEC.md` "Non-goals"), the full list is available.
- Invoke the `/frontend-design` skill before shaping the tab: dense operator-console table, kind shown as quiet colour, disabled actions with visible reasons.

## Edge cases
- Renaming a state renames it everywhere (tasks reference states by id), so after a rename the board columns and card `state` names change on refresh; no client-side remap needed.
- Position edits: the API shifts states at and after the new position, so after a move refetch rather than reordering locally.
- Creating a state with an explicit position equal to an existing one is valid (shifts the rest).
- Deleting a state the user just emptied: counts come from the last task query; invalidate on task events is not available on this tab unless the board is mounted, so refetch tasks on window focus (`refetchOnWindowFocus: true`) and before enabling delete after an edit.

## Testing
- Unit test (Vitest, if present from the store task) for the pure `deletionReason(state, states, taskCounts): string | null` helper covering the four refusal cases and the allowed case.
- Manual: add, rename, reorder, delete against a running orchestrator; confirm 409 surfaces when racing.
- `cd frontend && npm run lint && npx tsc -b && npm run build` (plus `npm run test:unit` when present).

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- Frontend project and session views epic: `pages/ProjectPage.tsx` with a states tab slot that renders this component.
- Frontend foundation epic: shared components and `useFormSubmit()`.
- Task tracker epic: the task-states endpoints and refusal statuses as documented.