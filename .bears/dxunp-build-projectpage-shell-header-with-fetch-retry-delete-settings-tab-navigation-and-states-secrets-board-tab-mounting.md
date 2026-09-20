---
id: dxunp
title: "Build ProjectPage shell: header with fetch/retry/delete/settings, tab navigation and states/secrets/board tab mounting"
status: done
priority: P1
created: "2026-09-16T20:42:34.326842377Z"
updated: "2026-09-20T07:46:35.538882400Z"
tags:
  - frontend
  - projects
depends_on:
  - "2txez"
parent: cgdc2
attempts: 1
---

## Summary
Deliver the `/projects/:id` route shell: the project header (name, remote, default branch, status, last fetch) with the project-level actions (`Fetch now`, `Retry clone`, `Delete`, a small settings form for name, default branch and `max_attempts`), and the tab strip (sessions, board, profiles, shared directories, states, secrets). The sessions, profiles and shared-directories tabs are filled by later tasks in this epic; the board and states tabs mount components delivered by the task board epic; the secrets tab mounts the project-scoped secrets manager from the foundation epic.

## Documents
- `SPEC.md` "User-facing features", Projects paragraph (a ready project shows its default branch, last fetch time, profiles, sessions, shared directories, task states and task board; deleting removes sessions, tasks, secrets, shared directories, CLI state directory and mirror).
- `SPEC.md` "Projects" table: `GET /projects/{id}`; `PUT /projects/{id}` `{name?, default_branch?, max_attempts?}`; `DELETE /projects/{id}` → 204 (409 while any session is `running` or `creating`); `POST /projects/{id}/retry-clone` (only from `error`); `POST /projects/{id}/fetch` → `Project` (runs a mirror fetch now); `max_attempts` 1–20 default 3.
- `SPEC.md` "Frontend" routes: `/projects/:id` (ProjectPage, with tabs for sessions, board, profiles, shared directories, states and secrets), `/projects/:id/tasks/:number` (the board with that task's drawer open); "Task board" paragraph (`TaskStatesEditor` on the project page).
- `SPEC.md` "Secrets" (`GET /secrets?scope=project&scope_id=<pid>`).
- `ARCHITECTURE.md` "Git model", Project clone paragraph (fetch refreshes upstream-tracking refs only and updates `last_fetched_at`).

## Acceptance criteria
- [ ] `frontend/src/pages/ProjectPage.tsx` is routed at `/projects/:id` and also at `/projects/:id/tasks/:number` (the latter forces the board tab; the drawer itself is the board epic's).
- [ ] Tab selection is the `?tab=` search parameter with values `sessions` (default), `board`, `profiles`, `shared-dirs`, `states`, `secrets`; the `/tasks/:number` route implies `board`. Unknown values fall back to `sessions`.
- [ ] Header shows name, `remote_url` (monospace), `default_branch` (or `discovering…`), the status pill from the ProjectsPage task, `status_message` when `error`, `last_fetched_at` relative time and `max_attempts`.
- [ ] `Fetch now` calls `fetchProject(id)`, disables while pending, updates the `["projects", id]` cache with the returned `Project`, surfaces errors (409 when not `ready`) in an `Alert`.
- [ ] `Retry clone` appears only when `status === "error"` and calls `retryClone`.
- [ ] `Delete` asks for confirmation naming what is removed (sessions, tasks, secrets, shared directories, mirror), calls `deleteProject`, navigates to `/projects` on 204, and shows the 409 message `refused while a session is running or being created` (server text) inline without navigating.
- [ ] Settings form (collapsed by default) edits `name`, `default_branch` (select from `listBranches` heads plus free text) and `max_attempts` (1–20) through `updateProject`, with 400/409 errors surfaced.
- [ ] Tabs `sessions`, `profiles`, `shared-dirs` render placeholders (`EmptyState` "coming in this epic") until their tasks land; `board` renders `TaskBoard` and `states` renders `TaskStatesEditor` from `frontend/src/tasks/` when those exports exist, otherwise an `EmptyState` naming the task board epic; `secrets` renders the foundation's project-scoped secrets manager with `scope="project"`, `scope_id=id`.
- [ ] While the project is `cloning`, only the header and settings are active; the other tabs show a "clone in progress" `LoadingState` and the page polls `["projects", id]` every 3 s.
- [ ] 404 from `getProject` renders the shared not-found state.
- [ ] `cd frontend && npm run lint && npx tsc -b && npm run build` pass.

## Implementation notes
- Files: `frontend/src/pages/ProjectPage.tsx`, `frontend/src/pages/project/ProjectHeader.tsx`, `frontend/src/pages/project/ProjectSettingsForm.tsx`, `frontend/src/pages/project/ProjectTabs.tsx` (exports `ProjectTab` type and the tab list so later tasks register their panels), `frontend/src/pages/project/index.ts`, route entries in `App.tsx`.
- Provide a `useProject(id)` hook in `frontend/src/pages/project/useProject.ts` (TanStack Query, key `["projects", id]`) reused by every tab task.
- Tab panels receive `{ project: Project }` as props so they never re-fetch the project.
- The board tab is out of scope here; do not implement any task UI. The `states` tab likewise only mounts `TaskStatesEditor`.
- Use `SectionHeader` for tab headings; keep the header a single dense row per the `/frontend-design` skill.

## Edge cases
- Route param `id` that is not a UUID: treat as not found without calling the API.
- `default_branch` change while sessions exist is allowed by the API; no client-side refusal.
- Fetch failures (upstream unreachable) return an error status from the endpoint; show the server message, keep the previous `last_fetched_at`.
- Navigating away mid-mutation must not throw (guard `isMounted` via TanStack's mutation lifecycle).

## Testing
- No unit tests (thin composition). Playwright scenarios live in the End-to-end tests epic.
- Command: `cd frontend && npm run lint && npx tsc -b && npm run build`.

## Documentation
- `SPEC.md` "Frontend" routes line: add the sentence that tabs are selected with `?tab=` and that `/projects/:id/tasks/:number` opens the board tab, in the same commit (the parameter name is new contract the E2E epic will rely on).

## Assumes from other epics
- Frontend foundation epic: `PageLayout`, `ProtectedRoute`, `Alert`, `LoadingState`, `EmptyState`, `SectionHeader`, `FormField`, `SubmitButton`, `useFormSubmit`, a reusable project-scoped secrets manager component exported from the secrets page module, and a shared not-found state.
- Frontend task board epic: `TaskBoard` and `TaskStatesEditor` exports in `frontend/src/tasks/`; until they exist the tab shows the placeholder.
- Projects epic (backend): the endpoints above with the documented statuses.