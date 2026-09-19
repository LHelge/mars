---
id: "6e4m6"
title: Implement LoginPage, ForgotPasswordPage and ResetPasswordPage
status: in_progress
priority: P1
created: "2026-09-16T20:41:42.942553996Z"
updated: "2026-09-19T11:04:11.339588795Z"
tags:
  - frontend
  - auth
depends_on:
  - "66w65"
parent: "2f5u2"
attempts: 1
---

## Summary
Deliver the three unauthenticated entry pages in `AuthLayout`: `LoginPage` (username and password, throttle and credential errors, return-destination handling, first-login redirect), `ForgotPasswordPage` (identifier → always-204 confirmation) and `ResetPasswordPage` (`/reset-password/:token`, new password → 204 → back to login). They are the first pages that exercise `useFormSubmit`, `installSession` and `useReturnTo` end to end.

## Documents
- `SPEC.md` "User-facing features", Login and invites: 15-minute access token, 30-day refresh cookie; passwords 10–128 characters; "Login is throttled: after 10 failed attempts for one username or one client address within 15 minutes, the endpoint answers 429 for the next 15 minutes; password-reset requests are limited to 3 per identifier per hour and still answer 204."; the seeded `admin` must change the password at first login before doing anything else.
- `SPEC.md` "Auth (`/api/auth`)": `POST /auth/login` `{username, password}` → `{user, access_token}` (429 when throttled); `POST /auth/request-password-reset` `{identifier}` → 204 (always); `POST /auth/reset-password` `{token, password}` → 204.
- `SPEC.md` "Authentication": "Reset by link returns 204 without logging the user in; they then log in with the new password."; the frontend routes a `must_change_password` user to the change-password page.
- `SPEC.md` "Frontend", Routes: `/login`, `/forgot-password`, `/reset-password/:token`; Copy links: preserve the internal destination through login and any required first-login password change.
- `README.md` "Start": bootstrap credentials `admin` / `changeme`; "Open `PUBLIC_URL`, log in as `admin`, and you are required to set a new password before anything else works."
- `ARCHITECTURE.md` "Trust boundaries", 1: users exist only through invites.

## Acceptance criteria
- [ ] `frontend/src/pages/LoginPage.tsx` (route `/login`): fields `username` (`autoComplete="username"`, autofocus) and `password` (`autoComplete="current-password"`); submit calls `login()`, then `installSession(response)`, then navigates to `/change-password` (carrying `state.from`) when `response.user.must_change_password` is true, otherwise to `useReturnTo() ?? "/"` with `replace: true`.
- [ ] Login errors: 401 → `Alert` "Invalid username or password"; 429 → "Too many failed attempts. Try again in 15 minutes."; any other `ApiError` → its `error` text; network failure → "Orchestrator unreachable". The password field is cleared after any failure; the username is kept.
- [ ] An already-authenticated visitor to `/login` is redirected to `/` (or `state.from`) without rendering the form.
- [ ] `LoginPage` links to `/forgot-password` and shows the line "Accounts are created by invitation." (no sign-up link).
- [ ] `frontend/src/pages/ForgotPasswordPage.tsx` (route `/forgot-password`): one field `identifier` (label "Username or email"); on submit calls `requestPasswordReset(identifier)`; **regardless of outcome** (204 or throttled 204) renders the success state "If that account exists, a reset link has been sent." with a link back to `/login`. Only a network error shows an error alert.
- [ ] `frontend/src/pages/ResetPasswordPage.tsx` (route `/reset-password/:token`): fields `password` and `confirm` (`autoComplete="new-password"`); client-side validation: 10–128 characters, confirm matches; submit calls `resetPassword(token, password)`; on 204 renders "Password updated. Sign in with your new password." with a link to `/login` (it does **not** install a session); 400 → `Alert` "This reset link is invalid or has expired." with a link to `/forgot-password`.
- [ ] All three pages use `AuthLayout`, `FormField`, `SubmitButton`, `Alert` and `useFormSubmit`; none imports `fetch` or `apiClient` directly (only `services/auth`).
- [ ] `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:unit` passes.

## Implementation notes
- Files: `frontend/src/pages/{LoginPage,ForgotPasswordPage,ResetPasswordPage}.tsx`, `frontend/src/pages/index.ts`, `frontend/src/utils/password.ts` (`validatePassword(value): string | null` returning the message "Password must be 10–128 characters" — reused by the invite and change-password pages), `frontend/src/App.tsx` (swap placeholders for the real pages).
- Read the token with `useParams<{ token: string }>()`; never echo it into the DOM or a query string.
- Password length is measured in UTF-16 code units on the client; the server's rule is authoritative, so a 400 from the server is still shown verbatim.
- Keep the login form submit on Enter; disable the button while `loading`.

## Edge cases
- Submitting an empty username or password shows the field error without a request.
- A 403 `password change required` cannot happen on login, but a stale token plus a 403 on the redirect target is handled by `ProtectedRoute`, not here.
- `ResetPasswordPage` with a missing `:token` param renders the invalid-link alert immediately.
- Throttled password-reset requests still answer 204, so the forgot page must not try to detect throttling.

## Testing
- Vitest + `@testing-library/react` with `MemoryRouter` and stubbed `services/auth` functions: login success navigates to `/`; login success with `state.from = "/sessions/abc"` navigates there; login success with `must_change_password` navigates to `/change-password` keeping `from`; 401 and 429 messages; forgot page shows the success copy even when the stub rejects with a 429 `ApiError` (defensive) and shows an error only on `TypeError`; reset page rejects a 9-character password client-side and shows the invalid-link alert on a 400.
- Command: `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:unit`. Playwright scenarios (login and forced password change) belong to the End-to-end tests epic.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Authentication, users, invites and email": the three endpoints with the statuses cited, including 429 on login and the always-204 reset request.