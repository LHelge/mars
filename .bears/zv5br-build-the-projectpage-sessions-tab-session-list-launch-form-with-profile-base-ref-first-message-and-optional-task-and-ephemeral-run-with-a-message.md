---
id: zv5br
title: "Build the ProjectPage sessions tab: session list, launch form with profile, base ref, first message and optional task, and ephemeral run-with-a-message"
status: done
priority: P1
created: "2026-09-16T20:44:23.157112957Z"
updated: "2026-09-20T07:58:57.083198516Z"
tags:
  - frontend
  - sessions
  - projects
depends_on:
  - dxunp
parent: cgdc2
attempts: 1
---

## Summary
Fill the `sessions` tab of `ProjectPage`: the list of the project's sessions with state, kind, profile, branch, cost and activity, deletion of finished sessions, and the launch form that creates a session from a profile with a base ref chosen from the project's branches, an optional first message and an optional task. Selecting an ephemeral profile turns the form into "run with a message". A successful launch navigates to the new session view.

## Documents
- `SPEC.md` "User-facing features", Sessions paragraph (launch from a profile and a base ref, default the task's current hand-off commit when present otherwise the project's default branch, optional first message, optional task; title defaults; sessions keep running when nobody watches) and Agent profiles paragraph (an ephemeral profile runs one prompt and ends; launched from the project page with a message).
- `SPEC.md` "Sessions" table: `GET /projects/{pid}/sessions?state=` → `Session[]`; `POST /projects/{pid}/sessions` `{profile_id, base_ref?, title?, message?, task_id?}` → 201 `Session` (`state: creating`; 400 if `base_ref` does not resolve; 400 for an ephemeral profile with neither `task_id` nor `message`; 409 if the project is not `ready`; 409 if `task_id` names a task that is held, blocked or terminal); `DELETE /sessions/{id}` → 204 (must be `done` or `failed`); `Session` shape and the title default rule (task title, else first line of `message` truncated to 80 characters, else null).
- `SPEC.md` "Projects": `GET /projects/{id}/branches` → `Branch[]` (`kind: head | upstream | session`); the default session base is the integration head named by `default_branch`; callers may choose an upstream-tracking ref, tag, session ref or commit id.
- `SPEC.md` "Tasks": `{id}` accepts a task's UUID or per-project number (`GET /projects/{pid}/tasks/{id}` → `TaskDetail`).
- `ARCHITECTURE.md` "Session lifecycle" table (state meanings) and "Launch sequence".

## Acceptance criteria
- [ ] `frontend/src/pages/project/SessionsTab.tsx` lists `listProjectSessions(pid)` (key `["projects", pid, "sessions"]`, `refetchInterval` 10 s while any session is `creating`/`running`, otherwise 60 s) sorted by `created_at` desc; columns: title (link to `/sessions/{id}`, fallback `untitled`), state pill (`creating` animated, `running` green, `parked` amber, `done` grey, `failed` red with `error` on hover/tooltip), `kind`, profile name (joined from `listProfiles`), `branch` (monospace), `cost_usd` (4 decimals), `last_activity_at` relative.
- [ ] A state filter (all / running / parked / done / failed) applies `?state=` server-side.
- [ ] `Delete` on `done`/`failed` rows (confirm) → `deleteSession(id)` → invalidate; 409 shown inline.
- [ ] Launch form (`frontend/src/pages/project/LaunchSessionForm.tsx`): profile select (default profile preselected; each option shows kind and served states), base ref combobox fed by `listBranches(pid)` grouped `Integration heads` / `Upstream` / `Session branches` with free-text entry for tags or commit ids, preselected to `project.default_branch`; optional title; message textarea; optional task field accepting `#12`, `12` or a UUID.
- [ ] When a task number is entered the form resolves it with `getTask(pid, number)` (debounced), shows the task title, and if the task has a current `handoff` shows `Base: hand-off commit <short sha> (from session …)` and clears the explicit base ref so the server applies the hand-off default; the user may still override, in which case the override is disclosed ("overrides the hand-off base").
- [ ] Ephemeral profile selected: the heading changes to `Run with a message`, the message is required unless a task is given, and the help text says the session ends after one result with no composer. Conversational: message optional (label `First message (optional)`).
- [ ] Submit via `createSession(pid, input)` using `useFormSubmit()`; omit empty optional fields; on 201 invalidate the sessions list and navigate to `/sessions/{id}`; 400/409 messages from the server shown in an `Alert` (base ref unresolved, ephemeral without input, project not ready, task not launchable).
- [ ] The form is disabled with an explanatory note while `project.status !== "ready"`.
- [ ] `cd frontend && npm run lint && npx tsc -b && npm run build` pass.

## Implementation notes
- Files: `frontend/src/pages/project/SessionsTab.tsx`, `frontend/src/pages/project/LaunchSessionForm.tsx`, `frontend/src/pages/project/SessionStatePill.tsx` (shared with `SessionView` header and the dashboard if the foundation did not create one; check `components/` first), `frontend/src/pages/project/BaseRefSelect.tsx`, `frontend/src/utils/format.ts#formatUsd`, `#shortSha`.
- Register the panel in `ProjectTabs` for `tab=sessions`.
- Base ref value sent to the API is the branch `name` exactly as returned (`main`, `origin/main`, session id for session refs per the git contract) or the free text.
- Task resolution uses `services/tasks.ts#getTask`; do not implement any task mutation here.
- Session branches (ahead/behind, merge/rebase/push) are a separate task (`GitActionsPanel`) mounted below the list by that task; leave a slot.

## Edge cases
- `task_id` must be sent as the task UUID even when the user typed a number: resolve first, then submit; if resolution fails, block submit with `Task not found`.
- Whitespace-only message is treated as absent.
- Title longer than 80 characters is allowed by the API (only the default is truncated); no client cap beyond a soft counter.
- A profile list that is empty (cannot happen: `default` always exists) still renders without crashing.
- Double submit prevented by `useFormSubmit`'s pending state.

## Testing
- Vitest: `parseTaskRef("#12" | "12" | "<uuid>")` unit test in `frontend/src/utils/taskRef.test.ts`; no component tests required. Playwright launch scenarios belong to the End-to-end tests epic.
- Command: `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:unit`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- Frontend foundation epic: shared UI components and `useFormSubmit`.
- Session lifecycle epic (backend): the endpoints and statuses above.
- Frontend task board epic: `services/tasks.ts` mutations; this task only reads `getTask`.