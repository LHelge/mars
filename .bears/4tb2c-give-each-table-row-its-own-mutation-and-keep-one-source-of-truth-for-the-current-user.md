---
id: "4tb2c"
title: Give each table row its own mutation, and keep one source of truth for the current user
status: open
priority: P2
created: "2026-09-21T10:53:48.135364295Z"
updated: "2026-09-21T10:53:48.135364295Z"
tags:
  - frontend
  - technical-review
  - bug
  - react
  - query
parent: "579dz"
---

Problem:
- A useMutation observer follows only its latest mutate(). components/admin/UsersTable.tsx:55-72 and :178-181, components/admin/InvitesPanel.tsx:168-194 and :326-328, and pages/ProjectsPage.tsx:71-89 each hold one mutation per action for the whole table and derive a row's busy state from `variables?.id`. Delete user A on a slow request, then delete user B: variables is now B, A's row loses its spinner and re-enables while its DELETE is still in flight, and a second click gives a 404 banner. Same for resend, revoke and retry-clone. ProjectsPage also keeps `retryErrors: Record<string, string>` that only a successful retry of the same row ever clears, so a row's 409 text stays after the project moved on. SecretRow, ProfileRow and SharedDirRow already do it per row.
- pages/SettingsPage.tsx:90-116, :173-180: the notification checkbox is disabled only while user is undefined, not while the PATCH is pending. On, then off quickly: two PATCHes in flight, each onSuccess writes its own response into the cache and the auth store, and response order decides the final UI.
- The current user lives in both the auth store (services/auth.ts setCurrentUser) and the ["users","me"] query, joined by an effect in SettingsPage.tsx:59-73 that even pushes the optimistic value into the store. Each mutation site must remember both writes (SettingsPage, PasswordChangeForm.tsx:110); AuthBootstrap's onForbidden path updates only the store. After a demotion the next /settings visit runs setCurrentUser(cachedAdminUser) from the 5-minute gcTime cache, so the Admin nav entry and AdminRoute pass again until the refetch lands, and permanently if it fails (UI hint only; the server still refuses).

Acceptance: UserRow, InviteRow and ProjectRow components own their mutations (or busy state is derived from useMutationState); the per-row error state goes with them. The notification checkbox is disabled while its mutation is pending. One source of truth for the current user — simplest is the auth store stays authoritative and Settings reads useAuth().user, with the write in the query/mutation onSuccess rather than an effect. Tests: two overlapping row actions keep both rows busy; demotion followed by a Settings visit does not resurrect the admin UI.

References: frontend/src/components/admin/UsersTable.tsx, InvitesPanel.tsx; frontend/src/pages/ProjectsPage.tsx, SettingsPage.tsx; frontend/src/AuthBootstrap.tsx:67-73; frontend/src/services/auth.ts. Contract: SPEC.md, "Frontend" and "Users"; CLAUDE.md, "Frontend conventions" (useAuth for auth state).