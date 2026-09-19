---
id: b4t6r
title: "Implement AdminPage: user list, admin toggle, delete, invites with create, resend and revoke, last-admin errors surfaced"
status: done
priority: P1
created: "2026-09-16T20:43:37.613511505Z"
updated: "2026-09-19T11:22:49.071036957Z"
tags:
  - frontend
  - auth
depends_on:
  - "66w65"
parent: "2f5u2"
attempts: 1
---

## Summary
Build the administrator page at `/admin` (behind `AdminRoute`): a users table with per-row admin toggle and delete, and an invites section with a create form (email, admin flag), resend and revoke. Every mutation goes through `services/users`, invalidates the right query keys and surfaces the 409 last-administrator and self-deletion refusals, the 409 duplicate-invite conflict and 403 demotion as readable alerts.

## Documents
- `SPEC.md` "User-facing features", Users (admin): "List and delete users, toggle admin, send and revoke invites."; Login and invites: invites expire after 7 days and can be revoked; delivered by Resend or written to the orchestrator log.
- `SPEC.md` "Users (`/api/users`)": `GET /users` (admin) → `User[]`; `PUT /users/{id}` (admin) `{username, admin}` → `User` (409 if demoting the last admin); `DELETE /users/{id}` (admin) → 204 (409 for the last admin or yourself); `GET /users/invites` (admin) → `Invite[]` (open invites); `POST /users/invites` (admin) `{email, admin?}` → `Invite` (201; 409 if the email has a user or an open invite); `DELETE /users/invites/{id}` (admin) → 204 (revoke); `POST /users/invites/{id}/resend` (admin) → `Invite` (new token and expiry, email sent again). "At least one administrator must remain... Self-demotion is allowed when another administrator remains; self-deletion remains prohibited... A rejected request changes no fields." The `Invite` response never includes the token.
- `SPEC.md` "Authentication": demotion takes effect on the next request (403 for an admin-only action).
- `SPEC.md` "Frontend", Rules: refresh the current user after an authorization 403 so a demotion updates admin navigation.
- `docs/data-model.md` `users` (last-administrator invariant), `user_invites` (one open invite per email; expired invites reaped in the background).
- ADR 0013, ADR 0025, ADR 0026.

## Acceptance criteria
- [ ] `frontend/src/pages/AdminPage.tsx` (route `/admin`, `PageLayout` titled "Administration") renders two sections: "Users" and "Invitations".
- [ ] Users table columns: username (monospace), email, `Admin` toggle (checkbox or switch reflecting `admin`), `must_change_password` indicator, `notify_email` indicator, created (relative), and a `Delete` action. Rows are ordered by username. The current user's row is marked "(you)" and its Delete action is disabled with the tooltip "You cannot delete your own account".
- [ ] Toggling admin calls `updateUser(id, { username: user.username, admin: !user.admin })` (the endpoint requires both fields); success invalidates `["users"]`; a 409 shows `Alert kind="error"` with the server's `error` text ("cannot demote the last administrator" or as the auth epic words it) and the toggle reverts. Demoting yourself is allowed by the UI but preceded by a `window.confirm` "Remove your own administrator role?"; after success the page's next request 403s, `onForbidden` reloads `/users/me` and `AdminRoute` shows the 403 page.
- [ ] Delete calls `deleteUser(id)` after `window.confirm("Delete user <username>? Their sessions and secrets remain attributed to a removed user.")`; 409 shows the server's `error` text (last administrator); success invalidates `["users"]`.
- [ ] Invitations: a form with `email` (type `email`, required) and an `Admin` checkbox; submit calls `createInvite({ email, admin })`; 201 prepends the invite and clears the form; 409 shows "That email already has an account or an open invitation."; 400 shows the server text. The success alert says "Invitation sent to <email>. Without email configured, the link is in the orchestrator log." so local development is discoverable (ADR 0026).
- [ ] Invites table columns: email, admin flag, invited-by username (resolved from the users list, fallback `—`), expires (relative, red `StatusBadge`-style text when in the past), created; actions `Resend` (calls `resendInvite(id)`, replaces the row with the response, shows "Invitation re-sent") and `Revoke` (`revokeInvite(id)` after confirm, removes the row).
- [ ] All mutations use `useMutation` with `onSuccess: () => queryClient.invalidateQueries({ queryKey: ["users"] })` or `["invites"]`; the tables keep previous data while refetching; a non-admin who reaches the page through a stale link sees the `AdminRoute` 403 page, and a 403 from any call here is also surfaced inline.
- [ ] The token never appears anywhere on this page (the API never returns it).
- [ ] `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:unit` passes.

## Implementation notes
- Files: `frontend/src/pages/AdminPage.tsx`, optionally `frontend/src/components/admin/{UsersTable,InvitesPanel}.tsx` if the page exceeds ~250 lines, `frontend/src/services/queryKeys.ts` (add `users.list()`, `invites.list()`), `frontend/src/App.tsx`.
- Use `useFormSubmit` for the invite form; use `useMutation` for row actions so each row has its own pending state (disable that row's controls while pending).
- `window.confirm` is acceptable for v1; no modal component is introduced in this epic.
- Relative time and date helpers from `utils/format.ts`.

## Edge cases
- Two admins demote each other concurrently: one gets 409; show it and refetch so both rows reflect the truth.
- Deleting a user who is currently signed in elsewhere: nothing to do client-side (their next request 401s).
- An invite that expired but has not been reaped yet is still listed by `GET /users/invites` if the backend includes it; render the expiry in the failed-state colour and keep Resend enabled (resend issues a new expiry).
- Email input is trimmed and lower-cased before sending to match the server's normalisation, so the 409 duplicate check behaves predictably.
- Toggle reverts on error: keep the checkbox controlled by the query data, not local state.

## Testing
- Vitest + `@testing-library/react` with stubbed `services/users`: renders users and invites; the current user's Delete is disabled; toggling admin calls `updateUser` with both fields; a stubbed 409 on toggle shows the alert and leaves the checkbox unchanged; the invite form posts the normalised email and shows the success alert; a stubbed 409 shows the duplicate message; Revoke calls `revokeInvite` and the row disappears after invalidation.
- Command: `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:unit`. The Playwright "admin user management" scenario belongs to the End-to-end tests epic.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Authentication, users, invites and email": the six admin endpoints with the statuses cited and the exact 409 `error` wording; this page shows the server text verbatim so wording changes there need no frontend change.