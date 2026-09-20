---
id: h3uux
title: Write secrets manager, settings and admin user-management E2E specs
status: in_progress
priority: P2
created: "2026-09-16T20:45:10.533082166Z"
updated: "2026-09-20T17:26:54.273854353Z"
tags:
  - frontend
  - secrets
  - auth
  - tests
depends_on:
  - ku8up
parent: "6s8j7"
attempts: 1
---

## Summary
Cover the "Secrets" and "Users (admin)" feature paragraphs plus the settings page: a write-only secrets round trip at global, project and user scope (create, replace, rename, delete, list with metadata, orchestrator-only flag, uses list, value never displayed, other users' user-scoped secrets hidden), the `notify_email` toggle, and admin user management (list, toggle admin, last-admin refusals, delete, self-delete refusal, non-admin locked out of `/admin`).

## Documents
- `SPEC.md` "User-facing features", "Secrets", "Users (admin)" and the settings sentence in "Login and invites" (opt out of escalation emails).
- `SPEC.md` "Secrets" table (`GET /secrets?scope=&scope_id=`; `POST` `{scope, scope_id?, name, value, orchestrator_only?}` → 201 `SecretMeta`, 409 if exists; `PUT /{id}` `{value}`; `PATCH /{id}` `{name?, orchestrator_only?}`; `DELETE` → 204; `GET /{id}/uses` → `{session_id, user_id, purpose, at}[]`; `SecretMeta = { id, scope, scope_id, name, orchestrator_only, key_version, created_by, created_at, updated_at, last_used_at }`; no response contains `value`; user-scoped secrets visible only to owner or admin, 403 otherwise).
- `SPEC.md` "Users" table (`GET /users` admin; `PUT /users/{id}` `{username, admin}` → 409 demoting the last admin; `DELETE /users/{id}` → 409 for the last admin or yourself; `PATCH /users/me` `{notify_email?}`; `User` fields) and the "At least one administrator must remain" paragraph.
- `SPEC.md` "Authentication" (demotion takes effect on the next request; frontend refreshes the current user after an authorization 403 so admin navigation updates).
- `SPEC.md` "Frontend" routes `/secrets`, `/settings`, `/admin`; `AdminRoute`.
- `ARCHITECTURE.md` "Secrets" (envelope encryption; `key_version`; `secret_uses` written at launch).

## Acceptance criteria
- [ ] `frontend/tests/secrets.spec.ts`: `global secret round trip`: create `API_TOKEN` with value `fake-value-1` → listed with scope `global`, `key_version` 1, created/updated timestamps, no value shown anywhere in the DOM (`expect(page.getByText("fake-value-1")).toHaveCount(0)` after creation and after reload); replace value → `updated_at` changes; rename to `API_TOKEN_2` → listed under the new name; toggle `orchestrator_only` → badge shown; duplicate create of `API_TOKEN_2` → 409 error shown; delete → gone.
- [ ] `project and user scope`: from the project page secrets tab create `PROJECT_KEY`; from `/secrets` with scope `user` create `MY_KEY`; a second user does not see `MY_KEY` in their user scope but sees `PROJECT_KEY` on the project; an admin selecting the first user's id in the scope picker sees `MY_KEY`.
- [ ] `uses list after a launch`: declare `PROJECT_KEY` on the default profile, launch a stub session, wait `running`, end it; the secret's `last_used_at` is set and the uses view lists one row with the session id and a purpose (the launch); the value still never appears.
- [ ] `validation`: an empty name and a name with spaces show 400 errors from the API.
- [ ] `frontend/tests/settings.spec.ts`: toggle `notify_email` off on `/settings`; reload; still off; `GET /users/me` through the helper API confirms `notify_email: false`.
- [ ] `frontend/tests/admin.spec.ts`: `non-admin cannot open /admin` (redirect or forbidden state, no user list rendered); `admin lists users and toggles admin`: admin A promotes user U → U's row shows admin; U, in a second context, now sees the admin navigation after its next request (open `/admin`); A demotes U → U's `/admin` visit fails at the next request and the navigation entry disappears after the app refreshes the current user.
- [ ] `last administrator is protected`: with A as the only admin, demoting A shows the 409 message and the toggle stays on; deleting A shows the 409 "yourself" message; promote U, then A can demote itself (allowed when another admin remains) and loses `/admin` access.
- [ ] `delete a user`: A deletes U; U's open context is logged out at its next request/refresh (401 → `/login`); U cannot log in again.
- [ ] `invites section` is covered by the auth spec; this file only asserts the section is present for admins.

## Implementation notes
- Files: `frontend/tests/secrets.spec.ts`, `frontend/tests/settings.spec.ts`, `frontend/tests/admin.spec.ts`.
- Use `createTestUser({admin: true})` for admins; the seeded admin is reserved for the auth spec.
- Value-never-shown assertions must scan the full page (`page.locator("body")` text and input values, including `type="password"` inputs' `value` attribute) after creation and after reload; the composer of a session or a diff is not involved here.
- Uses-list: the secrets epic writes `secret_uses` rows at launch (`ARCHITECTURE.md` "Launch sequence": "secret_uses rows written"); the purpose string is the secrets epic's; assert non-empty.

## Edge cases
- Demotion is enforced "on the next request": drive U's context to make a request (navigate to `/admin` or trigger `GET /users/me`) before asserting; allow up to 10 s.
- `DELETE /users/{id}` of the seeded admin is not attempted.
- Secrets created in one test are scoped to that test's user/project; global secrets leak across tests, so use unique names (`uniqueName("API_TOKEN")` upper-cased) and delete them in `afterEach`.

## Testing
- The three spec files; run twice against one stack.
- Command: `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:e2e`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Frontend foundation": `SecretsPage` (with scope picker and uses view), `SettingsPage`, `AdminPage`, `AdminRoute`, current-user refresh after 403.
- "Secrets manager": `/api/secrets` routes and `secret_uses` on launch; "Authentication, users, invites and email": `/api/users` routes and the last-administrator transaction.
- "Frontend project and session views": project secrets tab.