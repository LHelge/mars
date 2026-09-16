---
id: svv8w
title: "Write auth E2E specs: login, forced first-login password change, invite acceptance and password reset via logged links"
status: open
priority: P1
created: "2026-09-16T20:42:49.210367011Z"
updated: "2026-09-16T20:42:49.210367011Z"
tags:
  - frontend
  - auth
  - tests
depends_on:
  - ku8up
parent: "6s8j7"
---

## Summary
Cover the "Login and invites" feature paragraph end to end: the seeded administrator's forced password change at first login, ordinary login and logout, an admin inviting a user whose link is read from the orchestrator log and accepted in the browser, password reset through the same logged-link mechanism, and the errors a user sees for bad credentials, expired links and short passwords. These specs exercise the frontend foundation's auth pages against the real auth routes.

## Documents
- `SPEC.md` "User-facing features", "Login and invites" (no self-registration; seeded admin must change password first; invites by email link, 7-day expiry, revocable; passwords 10–128 characters; 15-minute access token, 30-day refresh cookie; reset works like invites; login throttling 429 after 10 failures).
- `SPEC.md` "Authentication" (`must_change_password` gate: every endpoint except login/logout/refresh/`GET /users/me`/`POST /users/{id}/password` answers 403 `password change required`; frontend routes such a user to the change-password page; self-service change keeps the browser signed in).
- `SPEC.md` "Auth" table (`POST /auth/login` → `{user, access_token}`; `GET /auth/invite/{token}` → `{email, admin, expires_at}` or 400 if expired, used or unknown; `POST /auth/accept-invite` `{token, username, password}` → 201; `POST /auth/request-password-reset` `{identifier}` → 204 always; `POST /auth/reset-password` `{token, password}` → 204; `POST /auth/logout` → 204).
- `SPEC.md` "Users" table (`POST /users/invites` `{email, admin?}` → 201 `Invite`, 409 if the email has a user or an open invite; `DELETE /users/invites/{id}` → 204; `POST /users/invites/{id}/resend` → new token).
- `SPEC.md` "Frontend" routes: `/login`, `/invite/:token`, `/change-password`, `/forgot-password`, `/reset-password/:token`, `/settings`; "Copy links" paragraph (return destination preserved through login and forced password change).
- `README.md` "Start" (`admin`/`changeme`).

## Acceptance criteria
- [ ] `frontend/tests/auth.spec.ts` with `test.describe.configure({ mode: "serial" })` only for the seeded-admin block; every other test creates its own users.
- [ ] `seeded admin must change password before anything else`: log in as `admin`/`changeme` through the UI; assert redirect to `/change-password`; navigating to `/projects` bounces back to `/change-password`; submitting a 9-character password shows a validation error; submitting `current_password: changeme` and a new valid password lands on `/` still signed in (no login form), and `GET /users/me` through the page's API (`page.request` with the stored token, or a visible username in the header) shows `must_change_password: false`. This is the only spec allowed to use the seeded admin; it runs first in the file.
- [ ] `login rejects a wrong password`: UI shows an error alert, stays on `/login`, no token in `localStorage`.
- [ ] `login and logout`: user from `createTestUser`; login lands on `/` (dashboard); logout from the header returns to `/login`; visiting `/` afterwards redirects to `/login`.
- [ ] `deep link is preserved through login`: open `/secrets` unauthenticated → `/login`; log in → land on `/secrets`.
- [ ] `admin invites a user who accepts via the logged link`: admin user from `createTestUser({admin: true})`; `off = logOffset()`; in the admin page invite `invitee-<hex>@example.test`; `readLoggedLink("invite", email, off)`; open the link in a fresh context; the page shows the invited email; choose username and password; assert 201 flow lands on `/` signed in as the new user; the admin page no longer lists the invite as open. Reopening the same link shows the "used" error from `GET /auth/invite/{token}` (400).
- [ ] `revoked invite cannot be accepted`: invite, revoke from the admin page, open the link → error state, no form.
- [ ] `password reset via logged link`: user from `createTestUser`; `/forgot-password` with the username submits and shows the neutral confirmation (204 always); read the `reset-password` link from the log; open it, set a new password → success message and redirect to `/login` (reset does not log in); login with the old password fails, with the new one succeeds.
- [ ] `settings page changes own password and keeps the session`: `/settings`, change with `current_password`; the page stays signed in; a subsequent API call from the page succeeds; a second context that had logged in earlier with the old token pair is signed out at its next refresh (open `/` there, expect `/login`).
- [ ] All tests pass in `npm run test:e2e` against the stack and take under 90 s combined.

## Implementation notes
- Files: `frontend/tests/auth.spec.ts`.
- Use `page.getByRole`/`getByLabel` selectors on the foundation pages (`FormField` labels `Username`, `Password`, `Current password`, `New password`, `Email`); add `data-testid` only where a role query is ambiguous, and document each new test id in the coverage task's list.
- The `readLoggedLink` helper (helpers task) needs `RUST_LOG=info`, which the stack sets; the invite email lands in `orchestrator.log` because `RESEND_API_KEY` is unset (ADR 0026).
- The revocation-on-password-change assertion works through `POST /auth/refresh` returning 401 for the old cookie; drive it by opening a page in the old context and waiting for `/login`.
- Login throttling (429 after 10 failures within 15 minutes) is not exercised here because it would poison the shared client address for the rest of the run; it is covered by backend integration tests. Note this in the coverage table.

## Edge cases
- The forced-change spec must run before any other spec that could log in as `admin`; no other spec uses the seeded account. Because `workers: 1` and the file order is alphabetical (`auth.spec.ts` first), a `test.describe.serial` block at the top of the file is sufficient; do not rely on `--grep` ordering.
- Invite links contain `PUBLIC_URL=http://localhost:5173`; the test opens them as-is.
- A resend replaces the token: assert the first link is rejected after a resend in the revoke test's sibling case when cheap; otherwise leave it to backend tests.

## Testing
- The spec file itself; run twice in a row against the same stack to prove independence from prior runs (the seeded admin test is skipped with `test.skip` when `admin`/`changeme` no longer logs in and the run is not fresh, so a rerun without `test:e2e:up` still passes).
- Command: `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:e2e`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Frontend foundation": `LoginPage`, `AcceptInvitePage`, `ChangePasswordPage`, `ForgotPasswordPage`, `ResetPasswordPage`, `SettingsPage`, `AdminPage` invite section, `ProtectedRoute` return-destination handling, logout control in `PageLayout`.
- "Authentication, users, invites and email": all `/auth` and `/users/invites` routes, `LogEmailClient`.