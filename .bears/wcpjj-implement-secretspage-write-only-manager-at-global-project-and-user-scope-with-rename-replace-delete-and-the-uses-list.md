---
id: wcpjj
title: "Implement SecretsPage: write-only manager at global, project and user scope with rename, replace, delete and the uses list"
status: open
priority: P1
created: "2026-09-16T20:44:16.196344291Z"
updated: "2026-09-16T20:44:16.196344291Z"
tags:
  - frontend
  - secrets
depends_on:
  - "66w65"
parent: "2f5u2"
---

## Summary
Build the secrets manager at `/secrets` and the reusable `SecretsManager` component the project page's secrets tab mounts later: a scope selector (global, a project, my user, or for admins another user), a metadata table of the secrets in that scope, a create form (name, value, orchestrator-only), and per-row actions to replace the value, rename, toggle orchestrator-only, delete and expand the recent uses. The value is entered once and never displayed again; `services/secrets.ts` wraps the six endpoints.

## Documents
- `SPEC.md` "User-facing features", Secrets: "A write-only manager at global, project and user scope: create, replace, rename, delete, and list names with metadata (scope, created, updated, orchestrator-only, key version, last use). The value is never shown after entry."
- `SPEC.md` "Secrets (`/api/secrets`)": `GET /secrets` `?scope=&scope_id=` → `SecretMeta[]` (user scope returns the caller's; admins may select another user with `scope_id`); `POST /secrets` `{scope, scope_id?, name, value, orchestrator_only?}` → `SecretMeta` (201; 409 if exists); `PUT /secrets/{id}` `{value}` → `SecretMeta`; `PATCH /secrets/{id}` `{name?, orchestrator_only?}` → `SecretMeta` (rename re-encrypts); `DELETE /secrets/{id}` → 204; `GET /secrets/{id}/uses` `?limit=` → `{session_id, user_id, purpose, at}[]`. "No response ever contains `value`. User-scoped secrets are listed, changed and deleted only by their owner or an admin (403 otherwise); global and project secrets by any user."
- `docs/data-model.md` `secrets`: `name` matches `^[A-Z][A-Z0-9_]{0,127}$`; `scope_id` is `users.id` for `user`, `projects.id` for `project`, NULL for `global`; `orchestrator_only` rows are never injected into containers; `key_version`. `secret_uses`: `purpose` is `launch` or `git`.
- `ARCHITECTURE.md` "Secrets": resolution order `global` → `project` → `user`, the last found wins; `orchestrator_only` rows are skipped and not shadowed; the project `GIT_CREDENTIAL` convention (`SPEC.md` "Projects": `credential` on create is stored as the project-scoped orchestrator-only secret `GIT_CREDENTIAL`).
- `README.md` "Operating notes": rotate the master key, "remove the old entry once `GET /api/secrets` shows no row on the old version" (why `key_version` is displayed).
- `SPEC.md` "Frontend", Routes: `/secrets`; `/projects/:id` has a secrets tab (project/session epic mounts this component).

## Acceptance criteria
- [ ] `frontend/src/services/secrets.ts` exports `listSecrets(params: { scope: SecretScope; scope_id?: string })`, `createSecret(body)`, `replaceSecretValue(id, value)`, `patchSecret(id, body)`, `deleteSecret(id)`, `listSecretUses(id, limit = 20)`; query keys `secrets.list(scope, scopeId)`, `secrets.uses(id)` in `queryKeys.ts`.
- [ ] `frontend/src/components/secrets/SecretsManager.tsx` exports `SecretsManager({ scope, scopeId, title? })` and is what both `/secrets` and the later project tab render; `frontend/src/pages/SecretsPage.tsx` (route `/secrets`, `PageLayout` titled "Secrets") adds the scope selector above it: radio/segmented control `Global` | `Project` (with a `<select>` of `listProjects()` names) | `My secrets` (scope `user`, no `scope_id`) | for admins `Another user` (scope `user` with `scope_id` from `listUsers()`). The selection is kept in the URL search params `?scope=&scope_id=` so it survives reload and can be linked.
- [ ] Table columns: name (monospace), orchestrator-only (icon + tooltip "Never injected into session containers"), key version, created by (username when resolvable, else `—`), created, updated, last used (relative, `never` when null); rows sorted by name. A help line under the header explains precedence in one sentence: "At launch, project secrets override global ones and your user secrets override both; orchestrator-only secrets are never injected."
- [ ] Create form: `name` (uppercased as typed, validated client-side against `^[A-Z][A-Z0-9_]{0,127}$` with the message "Name must be an environment-variable name: uppercase letters, digits and underscores, starting with a letter"), `value` (textarea, `autoComplete="off"`, `spellCheck={false}`), `orchestrator_only` checkbox; 201 prepends the row and clears the form (the value is cleared from component state immediately); 409 shows "A secret with that name already exists in this scope."; 400 shows the server text.
- [ ] Row actions: `Replace value` opens an inline form with a single value field posting `PUT /secrets/{id}`; `Rename` opens an inline name field posting `PATCH`; the orchestrator-only checkbox posts `PATCH { orchestrator_only }`; `Delete` after `window.confirm("Delete <name>? Sessions launched later will not receive it.")` posts `DELETE`; `Uses` expands a sub-row loading `GET /secrets/{id}/uses?limit=20` and lists `at` (relative), `purpose` and a link to `/sessions/{session_id}` when set, or the user id/username for `git` uses by a user, or "mirror fetch" when both are null.
- [ ] A 403 on a user-scoped list or mutation (another user's secrets, non-admin) shows `Alert kind="error"` "You can only manage your own secrets." and the table stays empty; global/project scopes never 403 for a signed-in user.
- [ ] Nothing on the page, in query cache, in the URL or in `localStorage` ever holds a secret value after the request resolves; the value textarea is unmounted on success.
- [ ] `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:unit` passes.

## Implementation notes
- Files: `frontend/src/pages/SecretsPage.tsx`, `frontend/src/components/secrets/{SecretsManager,SecretRow,CreateSecretForm,SecretUsesList}.tsx`, `frontend/src/services/secrets.ts`, `frontend/src/services/queryKeys.ts`, `frontend/src/utils/secretName.ts` (`SECRET_NAME_RE`, `validateSecretName`), `frontend/src/App.tsx`.
- `useMutation` per action with `onSuccess` invalidating `secrets.list(scope, scopeId)`; rename and value replacement also `setQueryData` with the returned `SecretMeta` for an instant update.
- `GIT_CREDENTIAL` in a project scope: show a small note "Used by git operations for this project" on that row; it is otherwise an ordinary secret.
- Clear the `value` state in the mutation's `onSettled`, not only on success, so a failed request still does not keep the value longer than necessary; the user retypes it.
- The scope selector's project `<select>` reads `listProjects()`; a project in `cloning`/`error` is selectable (secrets can be created before the clone finishes).

## Edge cases
- Renaming to an existing name in the same scope: 409, show the duplicate message and keep the inline editor open.
- Rename re-encrypts on the server; treat a 500 like any other error (generic message), never retry automatically.
- `scope=user` with `scope_id` equal to the current user's id is allowed and equals "My secrets"; normalise the URL to omit `scope_id` in that case.
- A non-admin editing the URL to `scope=user&scope_id=<other>` gets 403; render the alert, do not redirect.
- Large uses lists: `limit=20` with a "Show more" button doubling the limit up to 200; no pagination endpoint exists.

## Testing
- Vitest + `@testing-library/react` with stubbed `services/secrets`, `services/projects`, `services/users`: lists rows with metadata and `never` for null `last_used_at`; client-side rejects `lowercase` and `1ABC` names and accepts `MY_TOKEN_2`; create posts `{scope, scope_id, name, value, orchestrator_only}` and the value field is empty afterwards; 409 message on duplicate; replace-value posts `{value}`; delete calls after confirm; uses sub-row renders a session link for `launch` rows and "mirror fetch" for null/null rows; the admin-only "Another user" option is absent for a non-admin.
- Command: `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:unit`. The Playwright "secrets manager round trip" scenario belongs to the End-to-end tests epic.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Secrets manager": the six `/api/secrets` endpoints and the user-scope ownership rule (403).
- "Projects, agent profiles and shared directories": `GET /projects` for the project scope selector.
- "Frontend project and session views": mounts `SecretsManager` with `scope="project"` in the project page's secrets tab.