---
id: pqtsh
title: "Build ProjectsPage: project list with clone progress, create-project form and retry clone"
status: done
priority: P1
created: "2026-09-16T20:42:03.886072555Z"
updated: "2026-09-20T07:46:34.460091305Z"
tags:
  - frontend
  - projects
depends_on:
  - "2txez"
parent: cgdc2
attempts: 1
---

## Summary
Deliver the `/projects` route: a dense list of every project with its clone status, a create form (name, remote URL, optional default branch, optional personal access token) that returns immediately with `status: cloning`, live progress until the project turns `ready` or `error`, and a retry action for failed clones. Every user sees every project (no per-project authorisation in v1).

## Documents
- `SPEC.md` "User-facing features", Projects paragraph (creation returns immediately with `status: cloning`; the list shows progress and turns `ready` or `error`; deleting a project deletes everything under it).
- `SPEC.md` "Projects" table: `GET /projects` → `Project[]`; `POST /projects` `{name, remote_url, default_branch?, credential?}` → 201 `Project`; `POST /projects/{id}/retry-clone` → `Project` (only from `error`); `Project` shape; `credential` stored as the orchestrator-only secret `GIT_CREDENTIAL` and never returned (`has_credential` flag).
- `docs/data-model.md` `projects`: `name` 1–100 chars UNIQUE; `remote_url` `https://` only; `default_branch` null while discovery pends; `status_message` set when `error`.
- `SPEC.md` "Frontend" routes: `/projects` (ProjectsPage); `CLAUDE.md` "Frontend conventions" (TanStack Query for server state, `useFormSubmit`, shared UI components, `/frontend-design` skill before shaping UI).

## Acceptance criteria
- [ ] `frontend/src/pages/ProjectsPage.tsx` (named export) is routed at `/projects` inside `ProtectedRoute` + `PageLayout`.
- [ ] The list is a TanStack Query on `listProjects()` (key `["projects"]`) with `refetchInterval` of 3 s while any project is `cloning`, otherwise off; rows show name (link to `/projects/{id}`), `remote_url`, `default_branch` (or `discovering…` when null), a status pill (`cloning` animated, `ready` quiet, `error` red with `status_message` beneath), `last_fetched_at` relative time, and `has_credential` as a small lock indicator.
- [ ] Empty state uses `EmptyState` with a call to action to create the first project.
- [ ] Create form (inline panel or drawer) with `FormField`s: name (required, 1–100), remote URL (required, must start with `https://`, validated client-side before submit), default branch (optional), credential (optional password field, help text: "Personal access token for a private repository; stored write-only as the project secret GIT_CREDENTIAL and never shown again"). Submits through `createProject`, uses `useFormSubmit()`, shows 400/409 errors via `Alert` with the server's `error` message, and on 201 invalidates `["projects"]` and navigates to `/projects/{id}`.
- [ ] `Retry clone` button on `error` rows calls `retryClone(id)`, invalidates the list, and surfaces a 409 (not in `error`) as an inline error.
- [ ] Sorting: `cloning` and `error` first, then by name; stable across refetches.
- [ ] `cd frontend && npm run lint && npx tsc -b && npm run build` pass.

## Implementation notes
- Files: `frontend/src/pages/ProjectsPage.tsx`, `frontend/src/pages/projects/ProjectCreateForm.tsx`, `frontend/src/pages/projects/ProjectStatusPill.tsx` (reused by `ProjectPage`), `frontend/src/utils/time.ts#formatRelative` (if the foundation epic has not added one), route entry in `frontend/src/App.tsx`, `pages/index.ts` barrel.
- Query keys: `["projects"]` for the list, `["projects", id]` for a single project (the `ProjectPage` task uses the same key so a retry here warms it).
- Never render or persist the credential after submit; clear the form on success. The field uses `autoComplete="off"`.
- Follow the operator-console tone from the `/frontend-design` skill: monospace for `remote_url` and branch names, colour only on the status pill.

## Edge cases
- A 409 on create means the name is taken: show it on the name field.
- `default_branch` null with `status: ready` cannot occur (DB CHECK); still render defensively.
- Polling must stop when the tab is hidden (TanStack `refetchIntervalInBackground: false`).
- Remote URLs with embedded credentials (`https://user:token@…`) are rejected client-side with "Put the token in the credential field, not the URL".

## Testing
- No unit tests beyond a small `formatRelative` test if created here. Playwright coverage ("project creation from a local bare repository reaching ready") belongs to the End-to-end tests epic.
- Command: `cd frontend && npm run lint && npx tsc -b && npm run build`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- Frontend foundation epic: `PageLayout`, `ProtectedRoute`, `FormField`, `SubmitButton`, `Alert`, `LoadingState`, `EmptyState`, `SectionHeader`, `useFormSubmit`, the route table in `App.tsx`, and the navigation entry for Projects.
- Projects epic (backend): the endpoints above with the documented statuses.