---
id: jc3uu
title: Implement DashboardPage with running and parked sessions across projects and human-state tasks on a 30 s refetch
status: open
priority: P1
created: "2026-09-16T20:43:04.756699001Z"
updated: "2026-09-16T20:43:04.756699001Z"
tags:
  - frontend
  - sessions
  - tracker
depends_on:
  - "66w65"
parent: "2f5u2"
---

## Summary
Build the home page: three TanStack Query lists (`GET /sessions?state=running`, `GET /sessions?state=parked`, `GET /tasks?state_kind=human`) refetched every 30 seconds, rendered as dense tables with each row linking to `/sessions/:id` or `/projects/:project_id/tasks/:number`. This task also creates the thin `services/sessions.ts`, `services/tasks.ts` and `services/projects.ts` modules with only the list calls the dashboard needs (the project/session and board epics extend them), and a project-name lookup so rows show the project rather than a UUID.

## Documents
- `SPEC.md` "User-facing features", Dashboard: "The home page lists running and parked sessions across all projects and every task waiting in a human state."
- `SPEC.md` "Frontend", Dashboard: "`DashboardPage` loads `GET /sessions?state=running`, `GET /sessions?state=parked` and `GET /tasks?state_kind=human` through TanStack Query with a 30-second refetch interval, and links each row to its session or task."
- `SPEC.md` "Sessions": `GET /sessions` `?state=` → `Session[]` across all projects (dashboard); the `Session` shape (`title`, `state`, `kind`, `branch`, `last_activity_at`, `cost_usd`, `task_id`, `project_id`).
- `SPEC.md` "Tasks": `GET /tasks` `?state_kind=human` → `Task[]` across all projects (dashboard); the `Task` shape (`number`, `title`, `priority`, `needs_human_reason`, `assignee_user_id`, `attempts`, `project_id`, `updated_at`).
- `SPEC.md` "Projects": `GET /projects` → `Project[]`; `Project.name`, `status`.
- `SPEC.md` "Frontend", Routes: `/` (DashboardPage), `/sessions/:id`, `/projects/:id/tasks/:number`.
- `SPEC.md` "Non-goals": no pagination of task and session lists.
- `CLAUDE.md` "Frontend conventions": server state through TanStack Query; the tone is a dense operator console with colour reserved for state.

## Acceptance criteria
- [ ] `frontend/src/services/sessions.ts` exports `listSessions(params?: { state?: SessionState })` → `Session[]` via `apiGet("/sessions?state=...")`; `frontend/src/services/tasks.ts` exports `listHumanTasks()` → `Task[]` via `apiGet("/tasks?state_kind=human")`; `frontend/src/services/projects.ts` exports `listProjects()` → `Project[]`. Query keys are centralised in `frontend/src/services/queryKeys.ts` (`sessions.list(state)`, `tasks.human()`, `projects.list()`), which later epics extend.
- [ ] `frontend/src/pages/DashboardPage.tsx` (route `/`, inside `PageLayout` titled "Dashboard") runs the three queries with `refetchInterval: 30_000` and `refetchIntervalInBackground: false`, plus `projects.list()` with the default interval, and renders three sections with `SectionHeader`: "Running sessions", "Parked sessions", "Needs a human".
- [ ] Session rows show: `StatusBadge` for `state`, title (or `Untitled session` when `title` is null, in a muted style), project name (fallback: short id), `kind`, `branch` in monospace, `last_activity_at` as relative time, `cost_usd` formatted to 2 decimals; the whole row is a `Link` to `/sessions/{id}`. Running rows are ordered by `last_activity_at` descending; parked rows by `parked_at` descending.
- [ ] Task rows show: `#number` in monospace, title, project name, priority (`P0`–`P3` from `0`–`3`), `attempts`, `needs_human_reason` (truncated to one line, full text in `title` attribute), assignee username when resolvable from the users list (admins only; otherwise the id is omitted, not shown raw), `updated_at` relative; row links to `/projects/{project_id}/tasks/{number}`. Ordered by priority then `updated_at` descending.
- [ ] Each section shows `LoadingState` on first load, `EmptyState` ("No running sessions", "No parked sessions", "Nothing is waiting for a human") when the list is empty, and an inline `Alert kind="error"` with a retry button when the query errors while keeping the previous data visible (`placeholderData: keepPreviousData`).
- [ ] The page never calls `fetch`; only the three services and `services/users.listUsers` (guarded by `isAdmin`).
- [ ] The temporary `/` placeholder from the routing task is removed.
- [ ] `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:unit` passes.

## Implementation notes
- Files: `frontend/src/pages/DashboardPage.tsx`, `frontend/src/services/{sessions,tasks,projects,queryKeys}.ts`, `frontend/src/utils/format.ts` (`formatRelative`, `formatDateTime`, `formatUsd`), `frontend/src/App.tsx`.
- Build a `Map<projectId, Project>` from the projects query with `useMemo`; rows render `project?.name ?? project_id.slice(0, 8)`.
- Use plain `<table>` markup with sticky headers; no virtualisation (lists are small, no pagination in v1).
- `refetchInterval` also applies to `projects.list()` only if it is cheap; default is fine since names rarely change.

## Edge cases
- A session whose project was deleted concurrently: render with the short id, never crash on a missing map entry.
- `GET /tasks?state_kind=human` returns tasks from several projects with overlapping numbers; the link must always use the task's own `project_id`.
- `last_activity_at` null (session just created): show `—`.
- Clock skew makes `formatRelative` negative: clamp to "just now".
- Tab hidden: `refetchIntervalInBackground: false` pauses polling; `refetchOnWindowFocus` refreshes on return.

## Testing
- Vitest + `@testing-library/react` with a `QueryClientProvider` and stubbed services: renders one row per section with the right links (`/sessions/<id>`, `/projects/<pid>/tasks/<n>`); empty arrays render the three empty states; a rejected `listSessions("running")` shows the error alert while parked rows still render; queries are created with `refetchInterval: 30_000` (assert through the query's `options` or a spy on `useQuery`).
- Command: `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:unit`.

## Documentation
- none: implements `SPEC.md` "Frontend", Dashboard as written.

## Assumes from other epics
- "Session lifecycle: launcher, owner, recovery and sessions API": `GET /sessions?state=`.
- "Task tracker: states, tasks, leases, dependencies and events": `GET /tasks?state_kind=human`.
- "Projects, agent profiles and shared directories": `GET /projects`.
- "Frontend project and session views" / "Frontend task board...": the `/sessions/:id` and `/projects/:id/tasks/:number` targets; until then the rows link to the placeholders.