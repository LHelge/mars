---
id: mqf98
title: "Add routes/secrets.rs: GET/POST /api/secrets, PUT/PATCH/DELETE /api/secrets/{id}, GET /api/secrets/{id}/uses"
status: done
priority: P1
created: "2026-09-16T20:32:09.483672603Z"
updated: "2026-09-17T14:39:19.972138698Z"
tags:
  - orchestrator
  - secrets
  - tests
depends_on:
  - a2cku
parent: t36d2
assignee: claude-opus-subagent
attempts: 1
---

## Summary
Expose the secrets manager over REST in `routes/secrets.rs`: `GET/POST /api/secrets`, `PUT/PATCH/DELETE /api/secrets/{id}` and `GET /api/secrets/{id}/uses`, mapping the service to the documented statuses and the `SecretMeta` shape, with the guarantee that no response body ever carries a value. Every endpoint gets the full happy-path and error-path integration suite the epic requires.

## Documents
- `SPEC.md` "Secrets (`/api/secrets`)" (all six rows, `SecretMeta` fields, "No response ever contains `value`", ownership rule)
- `SPEC.md` "REST API" (bare JSON, 201 for create, 204 for delete, error shape `{status, error}`, status table)
- `CLAUDE.md` "API conventions"; "Backend conventions" (one route module per resource exporting `routes() -> Router<AppState>`, DTOs private to the module); "Testing expectations"

## Acceptance criteria
- [ ] `GET /api/secrets?scope=&scope_id=` → 200 `SecretMeta[]`; `scope` not in `global|project|user` → 400; `scope=project` without `scope_id` → 400; another user's `scope_id` as non-admin → 403; the same call as admin → 200.
- [ ] `POST /api/secrets` `{scope, scope_id?, name, value, orchestrator_only?}` → 201 `SecretMeta`; 400 for a bad name, empty value or bad `scope_id`; 403 for a foreign user scope; 409 for a duplicate.
- [ ] `PUT /api/secrets/{id}` `{value}` → 200 `SecretMeta`; 404 unknown id; 403 foreign user scope; 400 empty value.
- [ ] `PATCH /api/secrets/{id}` `{name?, orchestrator_only?}` → 200 `SecretMeta`; 409 rename to an existing name; 400 bad name; 403; 404; `{}` → 200 unchanged.
- [ ] `DELETE /api/secrets/{id}` → 204; 403; 404.
- [ ] `GET /api/secrets/{id}/uses?limit=` → 200 `{session_id, user_id, purpose, at}[]`; 403; 404; non-numeric `limit` → 400.
- [ ] Every route requires a JWT (401 without) and the auth extractor's `must_change_password` gate applies.
- [ ] `SecretMeta = { id, scope, scope_id, name, orchestrator_only, key_version, created_by, created_at, updated_at, last_used_at }` exactly, `scope_id` / `created_by` / `last_used_at` serialised as `null` when absent, timestamps RFC 3339, `scope` as `global` / `project` / `user`.
- [ ] The router is nested under `/api` in the application builder.

## Implementation notes
- File: `orchestrator/src/routes/secrets.rs` exporting `pub fn routes() -> Router<AppState>`; register in `orchestrator/src/routes/mod.rs`.
- Private DTOs: `CreateSecretBody { scope: SecretScope, scope_id: Option<Uuid>, name: String, value: Zeroizing<String>, #[serde(default)] orchestrator_only: bool }`, `ReplaceBody { value: Zeroizing<String> }`, `PatchBody { name: Option<String>, orchestrator_only: Option<bool> }`, `ListQuery { scope: Option<SecretScope>, scope_id: Option<Uuid> }`, `UsesQuery { limit: Option<u32> }`, `SecretMeta` (`From<SecretMetaRow>`), `SecretUse` (`From<SecretUseRow>`).
- Build `Actor { user_id, admin }` from the auth extractor's current user, taking `admin` from the database-loaded user rather than the JWT claim, matching the auth epic's admin extractor.
- `SecretScope` derives `Deserialize`/`Serialize` with `#[serde(rename_all = "lowercase")]` so query and body parsing share it.
- Do not derive `Debug` on DTOs holding `value` (or elide it) so an accidental `?body` log cannot leak.
- Unknown-id lookups in the service return `NotFound` before the ownership check, so a foreign user's secret is 403 only when it exists (as the SPEC table implies).

## Edge cases
- Malformed UUID in the path → 400 through the `Path` rejection, mapped to the `{status, error}` shape like other routes.
- `scope=user&scope_id=<self>` as non-admin → 200.
- `orchestrator_only` omitted on create → `false`.

## Testing
- Integration tests in `orchestrator/tests/secrets_api.rs` via `TestApp::spawn()`, one `#[tokio::test]` per scenario, users created through `POST /api/test/users` (one admin, two non-admins): for each of the six endpoints the happy path, 401 (no token), 403 (another user's `user`-scope secret, then the same call as admin succeeding), 400 (name `lowercase`, name of 129 chars, empty value, unknown scope string, `project` without `scope_id`), 409 (duplicate on create; duplicate on rename), 404 (random id). One test asserts, for every success response of every endpoint, that the raw body text contains neither the submitted value nor the key `"value"`. One test checks `last_used_at` and `/uses` (ordering, `limit`) after inserting `secret_uses` rows through the repository. Assert with `response.assert_status()` and `response.json::<T>()`.
- `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- none: implements the documented contract as written (the `scope`-omitted and `limit` sentence is added by the service task).

## Assumes from other epics
- "Authentication, users, invites and email": the JWT auth extractor and admin extractor; `POST /api/test/users` behind `integration-tests` for creating admin and non-admin users in tests.