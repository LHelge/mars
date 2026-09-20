---
id: "6s3j3"
title: "Implement useTaskStream: SSE open-before-load, explicit reconnect with a refreshed token and query invalidation on events"
status: done
priority: P1
created: "2026-09-16T20:41:37.676114039Z"
updated: "2026-09-20T10:28:18.775431761Z"
tags:
  - frontend
  - tracker
  - realtime
depends_on:
  - w9tuv
parent: gn4y2
attempts: 1
---

## Summary
Implement `frontend/src/tasks/useTaskStream.ts`, the hook that binds the task store to a project, opens the task SSE stream with `?token=` and `?after=lastSeq`, waits for the stream's `open` event before the first REST load, feeds every `TaskEvent` to `taskStore.noteEvent`, invalidates open task-detail queries, and on error closes the stream, refreshes the access token and reopens explicitly with the last cursor. The browser's automatic `EventSource` retry is never used.

## Documents
- `SPEC.md` "SSE: task stream" (URL, `?after=`, `event: task`, `id: <seq>`, keepalive, explicit reconnect, refresh on open/reopen)
- `SPEC.md` "Authentication" (stream rules paragraph: disable automatic retry, refresh token, reopen with fresh token and last cursor; 401 from refresh clears auth and routes to login; transient failures keep backoff; password change installs the new pair before reopening streams)
- `SPEC.md` "Frontend", "Task board" (first paragraph: `useTaskStream(projectId)` behaviour) and "Board refresh ordering" (open task details and related queries are invalidated on task events)
- ADR 0022

## Acceptance criteria
- [ ] `useTaskStream(projectId: string)` on mount / `projectId` change calls `useTaskStore.getState().bindProject(projectId)` and opens `new EventSource(taskStreamUrl(projectId, accessToken, lastSeq))`; no REST load happens before `onopen`.
- [ ] `onopen`: `setStream("live")`, reset the backoff, then `refresh()` (this is the initial load on first open and the reconnect refresh afterwards).
- [ ] `addEventListener("task", e)`: `JSON.parse(e.data)` as `TaskEvent`; `noteEvent(ev)`; when it returns `true` and `ev.task_id` is set, `queryClient.invalidateQueries({ queryKey: taskKeys.all(projectId) })` (covers every open detail query under the prefix) and, for `commented`/`state_changed`/`claimed`/`released`, also `["sessions"]`-prefixed task lists if the session epic defines them (best effort, no hard dependency). Malformed JSON is logged with `console.warn` and ignored.
- [ ] `onerror`: close the `EventSource`, `setStream("reconnecting")`, `bumpView()`, then `await refreshAccessToken()` from `services/auth`; on success reopen with the new token and the current `lastSeq` after a backoff of 1 s doubling to a 30 s cap; if refresh fails with 401 the auth service has already cleared local auth and routed to login, so the hook stops; other refresh failures use the same backoff.
- [ ] Subscribes to the auth service's token-change notification: when a new access token is installed while the stream is open (self-service password change), close and reopen with the new token and `lastSeq`.
- [ ] Unmount or `projectId` change closes the stream, clears pending timers and calls `bumpView()`; a response from the old view can never update the store.
- [ ] Vitest tests in `frontend/src/tasks/useTaskStream.test.ts` with a fake `EventSource` on `globalThis`: (a) no `listTasks`/`listTaskStates` call before `open`, one refresh after; (b) a `task` message reaches `noteEvent` and invalidates `taskKeys.all`; (c) `error` closes the source, calls the token refresh, and reopens with `after=` equal to the last `seq` received; (d) unmount closes and no further refresh occurs.
- [ ] `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:unit` pass.

## Implementation notes
- Files: `frontend/src/tasks/useTaskStream.ts`, `frontend/src/tasks/useTaskStream.test.ts`.
- Keep a `useRef` for the `EventSource`, backoff timer and a `closed` flag; wrap the open logic in a `connect()` closure re-used by reconnects.
- The token comes from `services/auth` (`getAccessToken()`); the refresh helper is the same one `apiClient` uses on 401 (`refreshAccessToken()`), so a 401 there performs the documented clear-and-route.
- SSE `id:` is the seq; since the browser's auto-reconnect is disabled the `Last-Event-ID` header is never relied on; `?after=` is passed on every open.
- Use `renderHook` from `@testing-library/react` (add as dev dependency if the store task did not) or a minimal custom harness.

## Edge cases
- A keepalive comment (`: keepalive`) never fires a message event; nothing to handle.
- Events replayed after reconnect (`seq <= lastSeq`) are rejected by `noteEvent`; no extra refresh.
- `onerror` can fire while the connection is still `CONNECTING` (e.g. 401 on open because the token expired): the same path applies, refresh then reopen.
- Rapid `projectId` changes: each `connect()` checks a captured view generation before touching the store.
- Do not open a stream when there is no access token (logged out); the route guard prevents this, but guard anyway.

## Testing
- Vitest tests above; `npm run test:unit`.
- Manual: expire the token (short `JWT` lifetime in a dev build or wait) and confirm the stream reopens and the board refreshes; kill the orchestrator and confirm backoff, then recovery with a refresh.
- Full chain: `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:unit`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- Frontend foundation epic: `services/auth` exposing `getAccessToken()`, `refreshAccessToken(): Promise<boolean | string>` (shared with `apiClient`'s 401 path) and a `subscribe(listener)` for token changes; `queryClient` export.
- Real-time delivery epic: `GET /api/projects/{pid}/tasks/stream?token=&after=` with `event: task`, `id: <seq>` framing.