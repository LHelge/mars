---
id: w9tuv
title: Implement taskStore with the ADR 0022 board refresh ordering
status: done
priority: P0
created: "2026-09-16T20:40:40.074332858Z"
updated: "2026-09-20T10:20:18.803361739Z"
tags:
  - frontend
  - tracker
  - tests
depends_on:
  - zazd5
parent: gn4y2
attempts: 1
---

## Summary
Build `frontend/src/tasks/taskStore.ts`, the Zustand store that holds the authoritative REST snapshot of a project's task states and tasks and implements the board refresh ordering rules of ADR 0022: at most one refresh in flight, view and event generations, coalesced follow-up, stale responses discarded, failures keeping the previous snapshot.

## Documents
- `SPEC.md` "Frontend", "Board refresh ordering" (the full paragraph is the contract)
- `SPEC.md` "Frontend", "Task board" (payloads are never applied to cards; columns in `position` order)
- `SPEC.md` "TaskEvent" (dedupe by `seq`; events are refresh signals; `states_changed` refreshes both)
- `SPEC.md` "SSE: task stream" (refresh on open/reopen and on each new event)
- `ARCHITECTURE.md` "Frontend architecture" (subscribe before loading, repeat a load dirtied by incoming changes, refresh on reconnect)
- ADR 0022
- `CLAUDE.md` "Code quality" (frontend chain) and "Frontend conventions" (Zustand for board state, never raw event arrays)

## Acceptance criteria
- [ ] `frontend/src/tasks/taskStore.ts` exports `useTaskStore` (Zustand `create`) with state `{ projectId: string | null; states: TaskState[]; tasks: Task[]; lastSeq: number; loaded: boolean; loading: boolean; dirty: boolean; error: string | null; stream: "connecting" | "live" | "reconnecting"; viewGeneration: number; eventGeneration: number; query: string }` and actions `bindProject(projectId)`, `setStream(status)`, `noteEvent(event: TaskEvent): boolean`, `invalidate()`, `bumpView()`, `refresh(): Promise<void>`, `setQuery(query)`, `reset()`.
- [ ] `bindProject(pid)`: when `pid` differs from the current one, increments `viewGeneration`, clears `states`, `tasks`, `query`, sets `lastSeq = 0`, `loaded = false`, `dirty = false`, `error = null`, `stream = "connecting"`. Same `pid` is a no-op.
- [ ] `noteEvent(ev)`: returns `false` and changes nothing when `ev.seq <= lastSeq` (duplicate or replayed); otherwise sets `lastSeq = ev.seq`, increments `eventGeneration`, sets `dirty = true`, schedules `refresh()` and returns `true`. The event payload (`task`, `states`, `comment`) is never written into the snapshot.
- [ ] `invalidate()` (called after every successful local mutation) increments `eventGeneration`, sets `dirty = true` and schedules `refresh()`.
- [ ] `bumpView()` increments `viewGeneration` (reconnect, unmount).
- [ ] `refresh()`: if `loading` is already true, returns immediately (the in-flight refresh sees `dirty`/generation change and follows up). Otherwise sets `loading = true`, `dirty = false`, captures `v = viewGeneration` and `e = eventGeneration`, reads **both** `listTaskStates(pid)` and `listTasks(pid)` concurrently, then: if `v !== viewGeneration` discard both and stop (no follow-up: the new view loads itself); else if `e !== eventGeneration` discard both, set `loading = false` and run exactly one follow-up `refresh()`; else install both arrays in one `set`, `loaded = true`, `error = null`, `loading = false`, then if `dirty` run one follow-up. `states` are stored sorted by `position`.
- [ ] Read failure (either read rejects): snapshot, `loaded` and `lastSeq` unchanged, `dirty` set back to `true`, `error` set to the message, `loading = false`; no empty board is ever installed. Reads go through `queryClient.fetchQuery` with `staleTime: 0` so TanStack's retry/backoff applies before the failure is reported.
- [ ] Unit tests in `frontend/src/tasks/taskStore.test.ts` cover: (a) an event noted while a load is in flight causes the in-flight responses to be discarded and exactly one coalesced follow-up refresh (two service calls total per endpoint, not three); (b) a failed load keeps the previous snapshot and `error` is set; (c) a duplicate `seq` returns `false` and triggers no refresh; (d) `bindProject` to another project makes the older responses ineligible (the first project's arrays are never installed); (e) `refresh()` while loading does not start a second concurrent read; (f) a successful load after `invalidate()` during the load performs one follow-up.
- [ ] `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:unit` pass.

## Implementation notes
- Files: `frontend/src/tasks/taskStore.ts`, `frontend/src/tasks/taskStore.test.ts`.
- Inject the services for testability: `createTaskStore(deps: { listTasks, listTaskStates })` returning the Zustand store, with `useTaskStore = createTaskStore({ listTasks, listTaskStates })` as the app singleton. Tests build their own store with deferred promises (`let resolve; new Promise(r => resolve = r)`) to control ordering.
- Selectors to export: `selectColumns(state)` → `{ state: TaskState; tasks: Task[] }[]` in `position` order, tasks in the API's order (priority, then number); `selectTaskByNumber(number)`; `selectTaskById(id)` (used by cards for parent badges and dependency links).
- The follow-up refresh must be scheduled as a microtask/`void refresh()` after the state update, not awaited inside the same call, to keep `refresh()` re-entrant-safe.
- `query` lives here so the drawer route change does not lose it; the search task defines the filtering.

## Edge cases
- An event arriving before the first load (stream open, load not yet started) simply raises `eventGeneration`; the first load captures the new generation.
- `noteEvent` on `states_changed` (`task_id` null) is handled identically: it is a refresh signal.
- `reset()` (called on logout / refresh 401 by the foundation's auth clearing) returns to the initial state and bumps `viewGeneration` so in-flight reads are discarded.
- A follow-up refresh that itself fails leaves `dirty = true`; the next event or the Retry action retries.

## Testing
- Vitest tests listed above, run with `npm run test:unit`.
- Full chain: `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:unit`.

## Documentation
- none: implements the documented contract as written (Vitest is delivered by the frontend scaffold task).

## Assumes from other epics
- Task `zazd5` in this epic (types, services, query keys).
- Frontend foundation epic: a shared `queryClient` instance exported from `frontend/src/services/queryClient.ts` (or wherever `main.tsx` creates it); if it is only created inline in `main.tsx`, export it from a module in this task.
- Repository scaffolding epic: the Frontend CI workflow file to extend.