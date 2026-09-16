---
id: k97mz
title: "Build SessionPage/SessionView: metadata header with cost and tokens, stop/end/sync/retry/delete actions, title edit, Copy link, side-panel layout and task side panel"
status: open
priority: P1
created: "2026-09-16T20:47:00.020802831Z"
updated: "2026-09-16T20:47:00.020802831Z"
tags:
  - frontend
  - sessions
depends_on:
  - zum7c
  - zxxj2
  - hkrrt
parent: cgdc2
---

## Summary
Assemble the `/sessions/:id` route: `SessionPage` loads the session, mounts `useSessionSocket` once, and renders `SessionView` with the metadata header (state, kind, branch, base ref, container, CLI session id, cost and tokens, error), the session actions (stop, end, sync, retry, delete), inline title editing, the `Copy link` action, the transcript plus composer in the main column, and a right-hand side panel with tabs for `Changes`, `Terminal` and `Tasks`. This task ships the `Tasks` panel (launched-for task and touched tasks); the `Changes` and `Terminal` panels are separate tasks that register into the side panel.

## Documents
- `SPEC.md` "User-facing features", Sessions paragraph (transcript, composer, stop button, metadata: state, branch, container, CLI session id; cost and token usage; users can end a session, sync its branch into the mirror, open a terminal; a parked session looks like a running one that is waiting).
- `SPEC.md` "Sessions" table: `GET /sessions/{id}`; `PUT /sessions/{id}` `{title}`; `DELETE /sessions/{id}` → 204 (must be `done` or `failed`); `POST /sessions/{id}/stop` → 202; `POST /sessions/{id}/end` → `Session` (stop, fetch-back, `done`); `POST /sessions/{id}/retry` `{message?}` → `Session` (conversational only, from `failed`; 409 for ephemeral); `POST /sessions/{id}/sync` → `{ref, commit}`; `GET /sessions/{id}/tasks` → `Task[]`; `Session` shape with `cost_usd`, `input_tokens`, `output_tokens`, `error`, `container_id`, `cli_session_id`, `task_id`, `handoff_id`.
- `SPEC.md` "Frontend": routes `/sessions/:id`; "Copy links" (absolute URL from the current origin and `/sessions/{id}`; `Link copied` only after the clipboard write succeeds; selectable field fallback when clipboard is unavailable or denied; no tokens, search or fragments); "Changes panel" last sentence (the session header shows `cost_usd` and the token counters); "Task board" paragraph (the session view shows the task the session was launched for and the tasks it touched in a side panel); "Session state" (`status` connecting/live/reconnecting).
- `SPEC.md` "Tasks": `Task` shape (`number`, `title`, `state`, `lease_holder_session_id`, `project_id`); task links use `/projects/{project_id}/tasks/{number}`.
- `ARCHITECTURE.md` "Session lifecycle" (state table; `failed` is retry-only for conversational; ephemeral not retried), "Stop semantics" (SIGINT, SIGTERM after `STOP_GRACE_SECS`, parked; stopped vs killed), "Cost accounting" (counters accumulate from `result` events).

## Acceptance criteria
- [ ] `frontend/src/pages/SessionPage.tsx` routed at `/sessions/:id`; loads `getSession(id)` (key `["sessions", id]`), seeds `store.session` with it if the store has none, mounts `useSessionSocket(id)` once and provides its API to children through a `SessionSocketContext`.
- [ ] `frontend/src/session/SessionView.tsx` layout: header row; main column = `Transcript` above `Composer`; right side panel (collapsible, default open on wide screens) with tabs `Changes`, `Terminal`, `Tasks` defined by a small registry (`sidePanels: {id, label, component, enabled(session)}[]`) so the Changes and Terminal tasks register their panels.
- [ ] Header shows: title (click to edit inline, `PUT /sessions/{id}` on Enter/blur, Escape cancels; fallback `untitled`), project link (`/projects/{project_id}`), state pill (`creating` animated, `running` green, `parked` amber with `waiting`, `done` grey, `failed` red), `kind` badge, connection status dot (`connecting`/`live`/`reconnecting`), `branch` and `base_ref` (monospace), `container_id` shortened to 12 chars with full value on hover (`—` when null), `cli_session_id` (monospace, `—` when null), `cost_usd` formatted `$0.0000`, `input_tokens`/`output_tokens` with thousands separators, `error` text when `failed`, `last_activity_at` relative.
- [ ] Actions with state gating: `Stop` (running) → `socket.stop()`; `End` (creating/running/parked, confirm) → `endSession` → cache update; `Sync` (running/parked/done) → `syncSession` → toast `Synced <ref> at <short commit>`; `Retry` (failed and conversational; opens a small form with an optional message) → `retrySession`; `Delete` (done/failed, confirm) → `deleteSession` → navigate to `/projects/{project_id}?tab=sessions`; every 409/400 surfaces the server `error` in an `Alert` in the header area. Ephemeral sessions never show `Retry`.
- [ ] `Copy link` button in the header: `frontend/src/components/CopyLinkButton.tsx` (`{ path: string }`) builds `${window.location.origin}${path}` with `path = /sessions/${id}`, writes with `navigator.clipboard.writeText`, shows `Link copied` for 2 s only on success, and on failure or missing clipboard API reveals a read-only selectable input with the URL. Reusable by the task board epic's drawer.
- [ ] `Tasks` panel (`frontend/src/session/TasksPanel.tsx`): `Launched for` section showing `getTask(project_id, task_id)` when `session.task_id` is set (number, title, state, whether this session still holds it via `lease_holder_session_id === id`), and `Touched` section from `listSessionTasks(id)` (key `["sessions", id, "tasks"]`, refetched on every `session` socket message and every 60 s); each row links to `/projects/{project_id}/tasks/{number}`.
- [ ] The store's `session` is the display source once present; the TanStack cache is updated from socket `session` messages through `queryClient.setQueryData` so other views stay consistent.
- [ ] Not-found (404) and forbidden render the shared error states; navigating between two session ids resets the view (socket hook handles the store).
- [ ] `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:unit` pass.

## Implementation notes
- Files: `frontend/src/pages/SessionPage.tsx`, `frontend/src/session/{SessionView,SessionHeader,SessionActions,SidePanel,TasksPanel,SessionSocketContext}.tsx`, `frontend/src/session/sidePanels.ts`, `frontend/src/components/CopyLinkButton.tsx`, `frontend/src/utils/format.ts` (`formatUsd`, `formatTokens`, `shortId`), route entry in `App.tsx`.
- Follow the `/frontend-design` skill: the header is a single dense monospace-heavy strip; colour only on the state pill and connection dot.
- Keep `SessionView` free of `fetch`; every call goes through `services/sessions.ts` and `services/tasks.ts`.
- Delete is a navigation away, so run `socket` teardown (unmount) before the request resolves is fine; do not reopen on 404 afterwards.

## Edge cases
- `Sync` on a `creating` session or one whose clone is gone returns an error; show it without changing state.
- `End` while `reconnecting`: REST still works; the store learns the new state from the returned `Session`.
- `Retry` returns `parked` then relaunches when `message` is given; the view shows `parked`/`running` as socket `session` messages arrive.
- Title edit with empty text sends `null`? The API takes `{title}`; send the trimmed string, and an empty string clears it only if the backend accepts it, otherwise keep the old title (surface 400).
- The `Tasks` panel for a session without `task_id` and no touched tasks shows an `EmptyState` `No tasks`.
- `Copy link` must never include `?tab=` or the token.

## Testing
- Vitest: `CopyLinkButton.test.tsx` (success shows `Link copied`; rejected clipboard shows the fallback input with the exact URL; URL has no query string), `format.test.ts`, and a `SessionHeader` render test asserting action visibility per state (running → Stop/End/Sync, failed conversational → Retry/Delete, failed ephemeral → Delete only, done → Sync/Delete).
- Playwright scenarios for the session view are in the End-to-end tests epic.
- Command: `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:unit`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- Frontend foundation epic: `PageLayout`, `ProtectedRoute` (return-destination preservation so a copied link survives login), `Alert`, `LoadingState`, `EmptyState`, a toast/notice primitive if one exists (otherwise a local transient `Alert`), `services/tasks.ts#getTask` shape and the `Task` type.
- Session lifecycle epic (backend): the endpoints above with the documented statuses; Real-time delivery epic: `session` socket messages on every state change.