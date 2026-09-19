---
id: hwebm
title: Implement ChangePasswordPage and the reusable PasswordChangeForm installing the replacement token pair
status: done
priority: P1
created: "2026-09-16T20:42:35.079485420Z"
updated: "2026-09-19T11:19:12.468185085Z"
tags:
  - frontend
  - auth
depends_on:
  - "66w65"
parent: "2f5u2"
attempts: 1
---

## Summary
Deliver the self-service password change: a `PasswordChangeForm` component (current password, new password, confirm) that posts `POST /users/{id}/password` for the signed-in user, installs the returned `{user, access_token}` so this browser stays signed in, and a `ChangePasswordPage` at `/change-password` that hosts it for the forced first-login change and then continues to the preserved return destination. The same form is embedded by the settings page.

## Documents
- `SPEC.md` "Authentication": "While the current user's `must_change_password` is true, every authenticated endpoint other than `POST /auth/login`, `POST /auth/logout`, `POST /auth/refresh`, `GET /users/me` and `POST /users/{id}/password` answers 403 with error `password change required`. The frontend routes such a user to the change-password page. Changing the password clears the flag; a self-service password change issues a fresh token pair"; "A self-service change requires `current_password` and creates a replacement refresh token in that transaction, then returns its cookie and a matching access token after commit; this browser stays signed in."; "A successful self-service password change installs its new pair before reopening streams."
- `SPEC.md` "Users (`/api/users`)": `POST /users/{id}/password` JWT (self) or admin, `{current_password?, password}` → `{user, access_token}` (self) or 204 (admin). "Changing your own password returns a fresh token pair for the new `auth_version`; all previous token pairs are invalidated."
- `SPEC.md` "Frontend", Rules: "Self-service password changes replace the token pair and reconnect streams without replaying pending inputs."; Copy links: preserve the destination through login and any required first-login password change.
- `SPEC.md` "User-facing features": passwords 10–128 characters; the seeded admin must change the password at first login.
- `README.md` "Start": "Changing your own password keeps the current browser signed in with new credentials."
- ADR 0025.

## Acceptance criteria
- [ ] `frontend/src/components/PasswordChangeForm.tsx` exports `PasswordChangeForm({ onSuccess?: () => void, requireCurrent?: boolean = true })`: fields `current_password` (`autoComplete="current-password"`), `password` and `confirm` (`autoComplete="new-password"`); client validation 10–128 and match; submit calls `changePassword(user.id, { current_password, password })` from `services/users`; on success calls `installSession(response)` (the new pair) and then `onSuccess`. Because `installSession` notifies `services/auth` subscribers, the session and board epics' stream hooks can reconnect with the fresh token; this task adds a `onCredentialsReplaced(handler)` registration in `services/auth.ts` fired by `installSession` when a user was already signed in, and documents in a comment that stream hooks subscribe to it.
- [ ] Error mapping: 400 with `error` mentioning the current password (or any 400) → shown verbatim under the form; 401 → handled by `apiClient` (sign-out); a `TypeError` → "Orchestrator unreachable". The three fields are cleared after success; only the new-password fields after a failure.
- [ ] `frontend/src/pages/ChangePasswordPage.tsx` (route `/change-password`, inside `ProtectedRoute` but exempt from the must-change redirect): `AuthLayout` titled "Set a new password"; when `user.must_change_password` is true it shows the info line "You must change your password before continuing."; after success it navigates to `useReturnTo() ?? "/"` with `replace: true`. A user whose flag is already false can still use the page (it then behaves like the settings form) and is sent to `from ?? "/settings"` afterwards.
- [ ] The page never calls any endpoint other than `POST /users/{id}/password` while the flag is set (the 403 gate would reject it); `PageLayout` is not used here because its nav would trigger gated requests.
- [ ] `services/users.changePassword` types the self result as `AuthResponse` and the admin result as `undefined`; the form asserts the self response is present.
- [ ] `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:unit` passes.

## Implementation notes
- Files: `frontend/src/components/PasswordChangeForm.tsx`, `frontend/src/pages/ChangePasswordPage.tsx`, `frontend/src/services/auth.ts` (add `onCredentialsReplaced`), `frontend/src/App.tsx` (replace the placeholder route), `frontend/src/utils/password.ts` (shared `validatePassword`).
- `installSession` ordering: set token → set user → notify subscribers → fire `onCredentialsReplaced`. TanStack Query caches are **not** cleared on a self-service change (same user, still authorised); only `["users","me"]` is updated via `queryClient.setQueryData`.
- The `must_change_password` flag on the returned `user` is false; `ProtectedRoute` reads it from `services/auth`, so no extra refetch is needed before navigating.

## Edge cases
- Wrong current password: the server answers 400 (or 401 per the auth epic's implementation; both are shown as "Current password is incorrect" when the `error` text says so, otherwise verbatim). Do not sign out on that 401: `apiClient` must treat a 401 from `POST /users/{id}/password` for the current user as a credential error, not as token expiry, on the **retried** request only (the first 401 still triggers the single refresh; if the retry also 401s, the error is surfaced). Confirm with the auth epic which status is used and add a note to the form.
- New password equal to the current one is allowed by the spec; do not block it client-side.
- A concurrent tab: after this tab installs the new pair, the other tab's next request 401s, its refresh 401s (old refresh token revoked), and it signs out. That is the documented behaviour; nothing to do.

## Testing
- Vitest + `@testing-library/react`: with a stubbed `changePassword` returning `{user: {...must_change_password: false}, access_token: "new"}` the form installs the token (assert `getAccessToken() === "new"`), fires `onCredentialsReplaced` once, and the page navigates to `state.from` (`/sessions/abc`) or `/`; a stubbed 400 shows its `error` text and keeps the current-password field cleared; client-side validation blocks a 9-character password and a mismatch.
- Command: `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:unit`. Playwright "forced password change" belongs to the End-to-end tests epic.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Authentication, users, invites and email": `POST /users/{id}/password` returning `{user, access_token}` for self with the new refresh cookie.
- "Frontend project and session views" / "Frontend task board...": their stream hooks subscribe to `onCredentialsReplaced` to reconnect without replaying pending inputs.