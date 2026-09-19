---
id: "2f5u2"
title: "Frontend foundation: auth, layout, dashboard, settings, admin and secrets pages"
type: epic
status: done
priority: P1
created: "2026-09-16T20:14:25.683075901Z"
updated: "2026-09-19T11:27:36.504409599Z"
tags:
  - frontend
depends_on:
  - t36d2
---

## Scope

The application shell and every page that needs only the auth, users and secrets APIs.

- `services/apiClient.ts` (`apiGet`/`apiPost`/`apiPut`/`apiPatch`/`apiDelete`, bearer attach, refresh once on 401, clear auth and route to login on refresh failure), `services/auth` with in-memory token mirrored to `localStorage`, `useAuth()`, `useFormSubmit()`, `types/` mirroring `SPEC.md` shapes in `snake_case`.
- Layouts and shared UI: `AuthLayout`, `PageLayout`, `FormField`, `SubmitButton`, `Alert`, `LoadingState`, `EmptyState`, `SectionHeader`, `ProtectedRoute` (with the `must_change_password` redirect and return-destination preservation for same-origin paths), `AdminRoute`.
- Pages: `LoginPage`, `AcceptInvitePage`, `ChangePasswordPage`, `ForgotPasswordPage`, `ResetPasswordPage`, `DashboardPage` (running and parked sessions across projects and human-state tasks, 30 s refetch), `SettingsPage` (own password, `notify_email`), `AdminPage` (users, invites, resend, revoke, admin toggle, delete with last-admin errors surfaced), `SecretsPage` (write-only manager at global, project and user scope with uses list).
- Operator-console visual language established through the `/frontend-design` skill: dark-friendly, dense, monospace for code and logs, colour reserved for state.

## Documents

`SPEC.md` "Frontend" (structure, rules, routes, dashboard), "User-facing features"; `ARCHITECTURE.md` "Frontend architecture"; `CLAUDE.md` "Frontend conventions".

## Acceptance criteria

- [ ] All listed routes render; components never call `fetch` directly.
- [ ] A refresh 401 clears local auth, closes stores and routes to login; a self-service password change keeps the browser signed in.
- [ ] `npm run lint && npx tsc -b && npm run build` pass.

## Out of scope

Project, session and board views (their own epics); Playwright suites (End-to-end tests epic).