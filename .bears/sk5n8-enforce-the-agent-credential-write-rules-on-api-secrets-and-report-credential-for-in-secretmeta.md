---
id: sk5n8
title: Enforce the agent-credential write rules on /api/secrets and report credential_for in SecretMeta
status: done
priority: P1
created: "2026-09-20T22:22:12.386070200Z"
updated: "2026-09-21T06:32:35.844106534Z"
tags:
  - orchestrator
  - secrets
  - migration
depends_on:
  - a8bga
parent: rdkmk
attempts: 1
---

## Summary
Move the "never both" conflict from launch time to the moment it is created. A scope holds at most one agent credential per backend (409 on create and rename, backed by a partial unique index), a credential cannot be `orchestrator_only` (400), and `SecretMeta` gains the derived `credential_for` so the frontend can group credentials without knowing names.

## Documents
- `SPEC.md` "Secrets": the `POST`/`PATCH` rows, `SecretMeta` with `credential_for`, the "Agent credentials" paragraph (exact error strings)
- `docs/data-model.md` `secrets`, `secrets_claude_credential_idx`
- `ARCHITECTURE.md` "Secrets", Agent credentials
- ADR 0036

## Acceptance criteria
- [ ] Migration (`sqlx migrate add -r secrets_claude_credential_idx`): `CREATE UNIQUE INDEX secrets_claude_credential_idx ON secrets (scope, scope_id) NULLS NOT DISTINCT WHERE name IN ('ANTHROPIC_API_KEY', 'CLAUDE_CODE_OAUTH_TOKEN')`. Before creating it, a `DO` block raises an exception whose message says which scope holds both credentials and that one must be deleted, so the failure is readable. `.down.sql` drops the index.
- [ ] `POST /secrets` with a credential name at a scope that already holds a credential of the same backend (either name) → 409 `this scope already has an agent credential (<NAME>); replace or delete it first`, `<NAME>` being the existing row's name. The same name at the same scope stays the existing plain 409.
- [ ] `PATCH /secrets/{id}` applies both rules to the resulting row: renaming into a credential name at an occupied scope → the 409 above; setting `orchestrator_only: true` on a credential, or renaming an orchestrator-only secret into a credential name → 400 `an agent credential cannot be orchestrator-only`.
- [ ] `POST /secrets` with a credential name and `orchestrator_only: true` → the same 400.
- [ ] The service checks first for the friendly message; a unique violation on `secrets_claude_credential_idx` from a concurrent write maps to the same 409 (without the name if it cannot be read back cheaply) and never to a 500.
- [ ] `SecretMeta.credential_for`: `"claude"` for the two names, else `null`; computed from `agent::credential_backend_of`, no column. Present in every response that returns `SecretMeta`.
- [ ] A test asserts that the literal names in the index predicate equal the Claude backend's `credential_names()` (read `pg_get_indexdef`), so the migration and the adapter cannot drift.
- [ ] `docs/data-model.md` migration order list mentions the new migration if that list enumerates files; `.sqlx/` regenerated; both clippy invocations and the test suite pass.

## Implementation notes
- Files: `orchestrator/migrations/`, `orchestrator/src/models/secret.rs` (validation + error enum variants), `orchestrator/src/secrets/service.rs`, `orchestrator/src/repositories/` secrets repository (scope in the `WHERE`; an "existing credential at scope" query taking the name list as `&[String]`), `orchestrator/src/routes/secrets.rs` (DTO field, error mapping).
- The orchestrator-only rule belongs in the model (it needs only the name and the flag); the one-per-scope rule needs the database and belongs in the service, inside the same transaction as the write.
- Rename re-seals under the new AAD through the sealed envelope; do the rule check before the re-seal.

## Edge cases
- Credentials of the same backend at *different* scopes are fine (that is the whole resolution model); only the same `(scope, scope_id)` conflicts.
- `GIT_CREDENTIAL` is orchestrator-only and not an agent credential; untouched.
- A user may only write their own user scope (existing 403 rules) — the new checks run after authorisation so a forbidden caller learns nothing about what the scope holds.

## Testing
- `tests/secrets_api.rs`: happy path with `credential_for`; 409 second credential same scope (other name); allowed at another scope; 400 orchestrator-only on create and on patch both ways; rename into occupied scope 409; unauthenticated/forbidden paths unchanged.
- `tests/migrations.rs`: up/down round trip; the readable failure when both exist.
- A concurrency test firing the two creates at once and asserting exactly one 201 and one 409.
