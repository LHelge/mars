---
id: "66w65"
title: Wire the route table, ProtectedRoute, AdminRoute, authenticated bootstrap via GET /users/me and the sign-out path
status: open
priority: P0
created: "2026-09-16T20:41:12.006070444Z"
updated: "2026-09-16T20:41:12.006070444Z"
tags:
  - frontend
  - auth
depends_on:
  - t85sb
  - ea6xs
parent: "2f5u2"
---

## Summary
Replace the scaffold's placeholder route with the documented route table, the `ProtectedRoute` and `AdminRoute` guards, the authenticated startup that loads `GET /users/me`, the 403-driven current-user refresh, and the sign-out path that clears TanStack Query caches and navigates to `/login`. It also creates `services/users.ts`, the typed wrapper for every `/api/users` endpoint, because the bootstrap is its first consumer and the settings, change-password and admin pages all reuse it. Pages that belong to later tasks or epics are mounted as small `NotFound`/placeholder elements so the table is complete from this commit.

## Documents
- `SPEC.md` "Frontend", Routes: `/login`, `/invite/:token`, `/change-password`, `/forgot-password`, `/reset-password/:token`, `/` (DashboardPage), `/projects`, `/projects/:id`, `/projects/:id/tasks/:number`, `/sessions/:id`, `/secrets`, `/settings`, `/admin`.
- `SPEC.md` "Frontend", Rules: "`ProtectedRoute` redirects a user whose current `must_change_password` is set to the change-password page. Token role/flag snapshots are UI hints only; the backend always checks the current user. Load `GET /users/me` at authenticated startup and refresh the current user after an authorization 403 so a demotion updates admin navigation. A failed refresh with 401 clears the access token from memory and `localStorage`, clears authenticated query and stream stores, closes streams and returns to login."
- `SPEC.md` "Frontend", Copy links: "Opening the link uses normal authentication; preserve the internal destination through login and any required first-login password change, then open that task drawer or session. Only accept same-origin application paths as return destinations."
- `SPEC.md` "Authentication": while `must_change_password` is true every authenticated endpoint other than `POST /auth/login`, `POST /auth/logout`, `POST /auth/refresh`, `GET /users/me` and `POST /users/{id}/password` answers 403 with error `password change required`; demotion takes effect on the next request (403), deletion on the next request (401).
- `SPEC.md` "Users (`/api/users`)" table (all rows) and the `User`/`Invite` shapes.
- `CLAUDE.md` "Frontend conventions": protected routes use `ProtectedRoute`, admin routes `AdminRoute`; server state through TanStack Query.

## Acceptance criteria
- [ ] `frontend/src/services/users.ts` exports `getMe()`, `updateMe(body: UpdateMeRequest)`, `listUsers()`, `getUser(id)`, `updateUser(id, body: UpdateUserRequest)`, `deleteUser(id)`, `changePassword(id, body: PasswordChangeRequest): Promise<AuthResponse | undefined>` (self returns `{user, access_token}`, admin-for-another returns 204 → `undefined`), `listInvites()`, `createInvite(body)`, `revokeInvite(id)`, `resendInvite(id)`; all through `apiClient`.
- [ ] `frontend/src/App.tsx` declares every route above. Unauthenticated routes (`/login`, `/invite/:token`, `/forgot-password`, `/reset-password/:token`) render outside `ProtectedRoute`; `/change-password` is inside `ProtectedRoute` but exempt from the must-change redirect; `/admin` is wrapped in `AdminRoute`; `/projects`, `/projects/:id`, `/projects/:id/tasks/:number`, `/sessions/:id` render a `PlaceholderPage` named export (one line "not implemented yet") that the project/session and board epics replace; `/` renders a temporary protected placeholder until the dashboard task lands; a catch-all renders `NotFoundPage` inside `PageLayout` when authenticated.
- [ ] `frontend/src/components/ProtectedRoute.tsx`: when `!isAuthenticated` → `<Navigate to="/login" state={{ from }} replace />` where `from = location.pathname + location.search` **only if** it is a same-origin application path (starts with a single `/`, not `//`, no scheme, not `/login`, `/invite/*`, `/forgot-password`, `/reset-password/*`); otherwise `from` is omitted. When authenticated but the current user is not loaded yet → `LoadingState`. When `user.must_change_password` is true and the route is not `/change-password` → `<Navigate to="/change-password" state={{ from }} replace />`. Otherwise renders `<Outlet />`.
- [ ] `frontend/src/components/AdminRoute.tsx`: inside `ProtectedRoute`; renders `<Outlet />` when `user.admin`, otherwise a `403`-style page (`Alert kind="error"` "Administrator access required") without redirecting, so a just-demoted admin sees why.
- [ ] `frontend/src/utils/returnTo.ts` exports `safeReturnTo(value: unknown): string | null` implementing the same-origin rule above and `useReturnTo()` reading `location.state.from`; the login and change-password pages use it to navigate after success (`from ?? "/"`).
- [ ] `frontend/src/AuthBootstrap.tsx` (mounted in `main.tsx` inside `QueryClientProvider` and `BrowserRouter`): on mount, if a token exists and no user is loaded, calls `getMe()`; a 401 (after the client's single refresh attempt) results in sign-out; success calls `setCurrentUser`. It registers `onForbidden(() => getMe().then(setCurrentUser).catch(() => {}))`, `onPasswordChangeRequired(() => navigate("/change-password"))` and `onSignOut(() => { queryClient.clear(); navigate("/login", { replace: true }) })`. It renders `LoadingState` until the initial `getMe` settles.
- [ ] A `SignOutRegistry` note in code comments states that the session and board epics register their store resets and stream closes through `onSignOut`; nothing else is needed from those epics for the "closes stores" criterion.
- [ ] `QueryClient` defaults: `retry: 1` except never retry on `ApiError` with status 401/403/404; `staleTime: 5_000`; `refetchOnWindowFocus: true`.
- [ ] The Playwright smoke test from the scaffold still passes: `/` without a token redirects to `/login`, which renders without an orchestrator.
- [ ] `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:unit` passes.

## Implementation notes
- Files: `frontend/src/App.tsx`, `frontend/src/main.tsx`, `frontend/src/AuthBootstrap.tsx`, `frontend/src/components/{ProtectedRoute,AdminRoute}.tsx`, `frontend/src/pages/{PlaceholderPage,NotFoundPage}.tsx`, `frontend/src/services/users.ts`, `frontend/src/utils/{returnTo,index}.ts`, `frontend/src/queryClient.ts`; delete `HealthPlaceholderPage` from the scaffold (keep `services/health.ts`).
- Use `react-router` v7 data-less `<Routes>/<Route>` with layout routes for the guards: `<Route element={<ProtectedRoute />}> ... </Route>`.
- The `from` destination is carried in router `state`, not in the URL, so tokens and search strings never leak into the address bar.
- `useReturnTo()` must strip `state.from` values that fail `safeReturnTo` (e.g. `https://evil.example`, `//evil.example`, `/login`).
- The must-change redirect is driven by the **current** `user.must_change_password` from `/users/me`, never by decoding the JWT.

## Edge cases
- Token present but `/users/me` fails with a network error: show an `Alert` "orchestrator unreachable" with a retry button rather than signing out (only a 401 signs out).
- A user already on `/change-password` with the flag cleared (after success) is navigated to `from ?? "/"` by that page, not by the guard.
- `onForbidden` after a demotion: the re-fetched user has `admin: false`; `PageLayout` hides the Admin link and `AdminRoute` shows the 403 page on the next render.
- Deleted user: `/users/me` → 401 → refresh → 401 → sign-out; no infinite loop because `apiClient` retries once.
- `signOut` runs handlers in registration order; navigation must be last so store resets happen before the login page mounts.

## Testing
- Vitest with `MemoryRouter`: `safeReturnTo` accepts `/projects/abc/tasks/42?x=1`, rejects `//evil`, `https://x`, `/login`, `` (empty); `ProtectedRoute` redirects an unauthenticated visit to `/login` with `state.from` set; redirects a `must_change_password` user to `/change-password` and keeps `from`; renders the outlet otherwise; `AdminRoute` renders the 403 alert for a non-admin. `AuthBootstrap`: with a stored token and a stubbed `getMe` 401 (after a stubbed refresh 401) the sign-out handler runs and the location becomes `/login`.
- Command: `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:unit`.

## Documentation
- none: implements `SPEC.md` "Frontend" routes and rules as written.

## Assumes from other epics
- "Authentication, users, invites and email": `GET /users/me` and the 403 `password change required` gate.
- "Frontend project and session views" and "Frontend task board, task detail and hand-off controls": replace the placeholders for `/projects*` and `/sessions/:id` and register their store resets with `onSignOut`.