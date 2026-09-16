---
id: "4qe6q"
title: Implement SettingsPage with the own-password form and the notify_email escalation opt-out
status: open
priority: P2
created: "2026-09-16T20:44:41.026181343Z"
updated: "2026-09-16T20:44:41.026181343Z"
tags:
  - frontend
  - auth
depends_on:
  - hwebm
parent: "2f5u2"
---

## Summary
Build the user's own settings page at `/settings`: account details (username, email, role), the `PasswordChangeForm` embedded for a self-service change that keeps the browser signed in, and the `notify_email` toggle that opts the user out of the task-escalation emails through `PATCH /users/me`. It is the last page of the epic and reuses everything the earlier tasks produced.

## Documents
- `SPEC.md` "User-facing features", Login and invites: "Each user can turn off the escalation emails described under 'Task board' on their own settings page."; Task board: "Every escalation into the human state emails the task's assignee, or every admin when there is none; each user can opt out."
- `SPEC.md` "Users (`/api/users`)": `GET /users/me` → `User`; `PATCH /users/me` `{notify_email?}` → `User`; `POST /users/{id}/password` (self) → `{user, access_token}`.
- `SPEC.md` "Frontend", Routes: `/settings` (own password and `notify_email`).
- `docs/data-model.md` `users.notify_email`: `BOOLEAN NOT NULL DEFAULT TRUE`, "Receive escalation emails".
- `README.md` "Operating notes": "Escalations to `needs_human` email the task's assignee, or every admin when there is none; each user can opt out under settings. Without `RESEND_API_KEY` these go to the log like invites."

## Acceptance criteria
- [ ] `frontend/src/pages/SettingsPage.tsx` (route `/settings`, `PageLayout` titled "Settings") renders three sections with `SectionHeader`: "Account" (username in monospace, email, "Administrator" or "Member", member since), "Notifications" and "Password".
- [ ] Notifications: a checkbox "Email me when a task I am assigned to, or any task when I am an administrator without an assignee, needs a human" bound to `user.notify_email` from `useQuery(["users","me"], getMe)` (seeded from `services/auth`'s current user as `initialData`); changing it calls `updateMe({ notify_email })` via `useMutation`, optimistic update with rollback on error, then `setCurrentUser(response)` and `queryClient.setQueryData(["users","me"], response)`; success shows a transient `Alert kind="success"` "Preferences saved"; an error shows the server text and reverts.
- [ ] Password: mounts `PasswordChangeForm` with `onSuccess` showing `Alert kind="success"` "Password changed. Other sessions of your account have been signed out." and no navigation. After success the page remains usable (the new token pair is installed by the form; verify by the next `getMe` succeeding).
- [ ] The page never calls any endpoint other than `GET /users/me`, `PATCH /users/me` and `POST /users/{id}/password`.
- [ ] `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:unit` passes.

## Implementation notes
- Files: `frontend/src/pages/SettingsPage.tsx`, `frontend/src/App.tsx` (replace the placeholder route), `frontend/src/services/queryKeys.ts` (`users.me()`).
- Keep `services/auth` and the query cache in sync: whichever `User` object arrives last wins; call `setCurrentUser` in both mutations' `onSuccess` so `PageLayout` and `ProtectedRoute` see the current values.
- The date helpers come from `utils/format.ts`.

## Edge cases
- The optimistic toggle must roll back on a 400/500 and on a network error; use `onMutate`/`onError` with the previous cache value.
- A 403 `password change required` cannot occur here because `ProtectedRoute` redirects first; a 403 for any other reason is shown verbatim.
- If `getMe` returns `admin: false` for a user whose cached snapshot said `true` (demoted meanwhile), `setCurrentUser` propagates it and the Admin nav link disappears.

## Testing
- Vitest + `@testing-library/react`: renders account details from a stubbed `getMe`; toggling the checkbox calls `updateMe({ notify_email: false })`, the checkbox flips immediately, and a stubbed rejection flips it back and shows the error; the embedded form's success shows the success alert without navigation (assert the location is still `/settings`).
- Command: `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:unit`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Authentication, users, invites and email": `PATCH /users/me` and `POST /users/{id}/password`.
- "Task tracker: states, tasks, leases, dependencies and events": honours `notify_email` when sending escalation emails; nothing further is needed from the frontend.