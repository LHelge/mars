---
id: hqbzq
title: Keep a loaded page mounted when a background refetch fails, and stop retrying 4xx answers
status: open
priority: P1
created: "2026-09-21T10:51:48.800893256Z"
updated: "2026-09-21T10:51:48.800893256Z"
tags:
  - frontend
  - technical-review
  - bug
  - react
  - query
parent: "579dz"
---

Problem: in TanStack Query v5 a failed background refetch yields `isError === true` while `data` stays defined (isRefetchError). queryClient.ts sets refetchOnWindowFocus and a 5 s staleTime, and several queries set `retry: false`, so one failed request is enough.
- pages/ProjectPage.tsx ProjectView branches on project.isError before it uses project.data (useProject has retry: false). A user writing a system prompt in the profile editor or a first message in the launch form alt-tabs away and back while the orchestrator restarts or nginx answers 502 once: the whole page becomes the error Alert; ProjectHeader, the settings form, the tab panel, the editor and the board with its task stream unmount, and "Try again" brings back a blank form. The 3 s cloning poll flickers the same way.
- pages/project/ProfilesTab.tsx has the same branch order, so a failed profiles refetch alone unmounts the profile editor. pages/AcceptInvitePage.tsx checks `error !== null` before data too (field state survives there because it lives in the parent).
- The opposite slip on first load: DashboardPage, ProjectsPage, SessionsTab and SharedDirsTab render the error Alert and the EmptyState together, because isError with no data means isPending is false and the row count is 0 ("Could not load projects" directly above "No projects yet — New project").
- Root of the single-failure sensitivity: queryClient.ts shouldRetry exempts only 401/403/404, so a 400/409/422/429 on a query is retried once after a second, and 13 call sites override with `retry: false`, which also removes the one retry that would have absorbed a transient 502.
- pages/ProfileEditorPage.tsx computes its "not a queue state" / "not created yet" orphan lists against arrays that are [] until their queries resolve and never renders states.isError: on a cold cache every served state and declared secret flashes as broken, and after one failed task-states read the fieldset claims the project has no queue states, inviting the user to "clean up" into a profile that serves nothing. (The file changed after the review; re-verify which of this remains.)

Acceptance: a page or tab that has data keeps rendering it when a refetch fails and shows the failure as a non-blocking banner with a retry; the blocking error state is only for "no data". EmptyState renders only on isSuccess. shouldRetry never retries an ApiError below 500 and retries network errors and 5xx once; the now-redundant `retry: false` overrides are deleted (keep any that are there for a different, stated reason). Orphan/derived warnings render only when their source query isSuccess, and its error is shown with a retry. Do this once in a shared component — DashboardPage's `Section` (DashboardPage.tsx:75-116) is already the right shape, and the "Alert + Try again" block is copied 14 times (ProjectPage, ProfilesTab, SessionsTab, SharedDirsTab, DashboardPage, ProjectsPage, AcceptInvitePage, UsersTable, InvitesPanel, SecretsManager, TaskDetail, ...): promote a `QueryErrorAlert` / query boundary to components/ and migrate the callers. Tests: a component test that fails a refetch under a half-filled form and asserts the form state survives; first-load failure shows no empty state.

References: frontend/src/queryClient.ts; frontend/src/pages/ProjectPage.tsx; frontend/src/pages/project/useProject.ts, ProfilesTab.tsx; frontend/src/pages/ProfileEditorPage.tsx; frontend/src/pages/DashboardPage.tsx. Contract: SPEC.md, "Frontend"; CLAUDE.md, "Frontend conventions" (shared UI list gains the new component).