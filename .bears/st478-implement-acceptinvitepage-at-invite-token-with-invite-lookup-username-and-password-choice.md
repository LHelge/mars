---
id: st478
title: "Implement AcceptInvitePage at /invite/:token with invite lookup, username and password choice"
status: in_progress
priority: P1
created: "2026-09-16T20:42:05.860658839Z"
updated: "2026-09-19T11:13:51.831770421Z"
tags:
  - frontend
  - auth
depends_on:
  - "66w65"
parent: "2f5u2"
attempts: 1
---

## Summary
Build the invitee's onboarding page: on load it resolves the invite token through `GET /auth/invite/{token}` to show the invited email and whether the account will be an administrator, lets the invitee choose a username and password, submits `POST /auth/accept-invite`, installs the returned token pair and lands on the dashboard. Invalid, expired or already-used links get a clear dead-end message.

## Documents
- `SPEC.md` "User-facing features", Login and invites: "Admins invite people by email: the invitee receives a link ..., opens it, chooses a username and password, and is logged in. Invites expire after 7 days and can be revoked."
- `SPEC.md` "Auth (`/api/auth`)": `GET /auth/invite/{token}` → `{email, admin, expires_at}` (400 if expired, used or unknown); `POST /auth/accept-invite` `{token, username, password}` → `{user, access_token}` (201) and sets the refresh cookie.
- `docs/data-model.md` `users`: `username` 3–32 chars; `user_invites`: single-use, 7-day expiry, at most one open invite per email.
- `SPEC.md` "Frontend", Routes: `/invite/:token`.
- ADR 0013 (invite-only users), ADR 0026 (link may come from the orchestrator log in development).

## Acceptance criteria
- [ ] `frontend/src/pages/AcceptInvitePage.tsx` uses `useQuery({ queryKey: ["invite", token], queryFn: () => lookupInvite(token), retry: false })`; while loading shows `LoadingState`; on a 400 `ApiError` shows `Alert kind="error"` "This invitation link is invalid, has expired or was already used. Ask an administrator for a new one." with a link to `/login`; on a network error shows "Orchestrator unreachable" with a retry button.
- [ ] On success the form shows the read-only invited email, a note "You will be an administrator" when `admin` is true, the expiry as "Expires <local date/time>", and fields `username` (3–32 characters, `autoComplete="username"`) and `password` + `confirm` (`autoComplete="new-password"`, 10–128, must match) validated client-side before submitting.
- [ ] Submit calls `acceptInvite({ token, username, password })`; on 201 calls `installSession(response)` and navigates to `/` with `replace: true` (no return destination applies to invites). A 400 from the server (invite consumed between lookup and submit, or invalid input) is shown verbatim from `error`; a 409 (username taken) shows "That username is already taken."
- [ ] If the visitor is already authenticated as another user, the page still renders (an admin may test an invite) but shows an info `Alert` "You are signed in as <username>; accepting will switch accounts." and `installSession` replaces the current session (and `queryClient.clear()` runs).
- [ ] The token never appears in the rendered DOM, in `localStorage` or in a query string.
- [ ] Uses `AuthLayout`, `FormField`, `SubmitButton`, `Alert`, `useFormSubmit`, `validatePassword` from `utils/password.ts`; no direct `apiClient` use.
- [ ] `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:unit` passes.

## Implementation notes
- Files: `frontend/src/pages/AcceptInvitePage.tsx`, `frontend/src/pages/index.ts`, `frontend/src/App.tsx` (replace the placeholder route), `frontend/src/utils/password.ts` (create here if the login task has not landed it yet; keep the same signature `validatePassword(value): string | null`).
- Username validation message: "Username must be 3–32 characters". Mirror the model rule only for length; other constraints are the server's and are surfaced from its 400.
- `expires_at` is RFC 3339; format with `Intl.DateTimeFormat` (add `utils/format.ts` with `formatDateTime(iso)` and `formatRelative(iso)`; the dashboard and admin tasks reuse them).

## Edge cases
- Lookup succeeded but `expires_at` is already in the past by the time the user submits: the server answers 400; show that message and offer the login link.
- Mismatching confirm field blocks submit client-side.
- Double submit is prevented by `useFormSubmit`'s loading guard.
- A missing `:token` param renders the invalid-link alert without a request.

## Testing
- Vitest + `@testing-library/react`: renders the email and admin note from a stubbed lookup; shows the invalid-link alert on a stubbed 400; client-side rejects a 2-character username and mismatched passwords; successful accept installs the session (assert `getAccessToken()` equals the stub token) and navigates to `/`.
- Command: `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:unit`. The Playwright "invite acceptance via the logged link" scenario belongs to the End-to-end tests epic.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Authentication, users, invites and email": `GET /auth/invite/{token}` and `POST /auth/accept-invite` with the statuses cited; `LogEmailClient` writes the link to the log for local development (ADR 0026).