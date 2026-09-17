---
id: a2cku
title: "Implement SecretsService: create, replace, rename re-encryption, delete, list and uses with ownership rules"
status: in_progress
priority: P1
created: "2026-09-16T20:30:47.404552253Z"
updated: "2026-09-17T13:07:28.946438926Z"
tags:
  - orchestrator
  - secrets
depends_on:
  - qafug
parent: t36d2
assignee: claude-opus-subagent
attempts: 1
---

## Summary
Deliver the domain operations behind the secrets API in `secrets/service.rs`: create, replace value, patch (rename and/or `orchestrator_only`), delete, list and uses, each enforcing the user-scope ownership rule, scope existence, name and value validation and the rename-re-encrypts-under-the-new-AAD rule in one transaction. The route module (task 5) becomes a thin adapter over this module.

## Documents
- `SPEC.md` "Secrets (`/api/secrets`)" (semantics of each row; "User-scoped secrets are listed, changed and deleted only by their owner or an admin (403 otherwise); global and project secrets by any user"; "rename re-encrypts under new AAD"; "409 if exists"; "No response ever contains `value`")
- `SPEC.md` "REST API" status table (400 validation, 403 forbidden, 404 unknown, 409 duplicate name)
- `docs/data-model.md` `secrets` (name pattern `^[A-Z][A-Z0-9_]{0,127}$`, `scope_id` rules, AAD, "Renaming a secret therefore re-encrypts the value under the new AAD (decrypt, re-encrypt, one transaction); it does not change the data key")
- `ARCHITECTURE.md` "Secrets" → "Credential handling and transcripts" (zeroize buffers, secret names only in tracing spans)

## Acceptance criteria
- [ ] `Actor { user_id: Uuid, admin: bool }` is the caller identity passed to every operation.
- [ ] `create(actor, CreateSecret { scope, scope_id: Option<Uuid>, name, value: Zeroizing<String>, orchestrator_only })`: `scope_id` must be `None` for `global` (else `BadRequest("scope_id must be empty for global scope")`), `Some` for `project` (else `BadRequest("scope_id is required for project scope")`), and for `user` defaults to `actor.user_id`; a non-admin passing another user's id → `Error::Forbidden`; unknown project/user id → `BadRequest("unknown scope_id")`; invalid name → `BadRequest` from `SecretError::InvalidName`; empty value or value over 65 536 bytes → `BadRequest`; duplicate → `Conflict`; seals with `crypto::seal` under `aad(scope, scope_id, name)`; returns `SecretMetaRow`.
- [ ] `replace_value(actor, id, value)`: loads the row `FOR UPDATE`, applies `authorize(actor, &row)` (user scope: owner or admin, else `Forbidden`; missing → `NotFound`), validates the value, seals under the current AAD with a fresh data key, `update_value` in the same transaction.
- [ ] `patch(actor, id, PatchSecret { name: Option<String>, orchestrator_only: Option<bool> })`: when `name` differs from the current name, validates it, `crypto::open`s under the old AAD, `reseal`s under the new AAD and calls `update_name_and_value` (name, flag, ciphertext, nonce) in one transaction; duplicate → `Conflict`; when only the flag changes, `update_flag` with no crypto; when nothing changes, returns the current meta (200).
- [ ] `delete(actor, id)`: authorize then delete; missing → `NotFound`.
- [ ] `list(actor, scope: Option<SecretScope>, scope_id: Option<Uuid>)`: `scope=user` without `scope_id` lists the caller's; with another user's `scope_id` requires admin (`Forbidden`); `scope=project` requires `scope_id` (`BadRequest`); `scope=global` with a `scope_id` → `BadRequest`; `scope` omitted returns global + every project secret + the caller's user secrets (admins: all users).
- [ ] `uses(actor, id, limit: Option<u32>)`: authorize (same rule as changes) then `list_uses`; `limit` defaults to 50 and is capped at 500.
- [ ] No plaintext value is ever logged or included in an error message; tracing spans carry `secret_name`, `scope`, `scope_id` only.

## Implementation notes
- File: `orchestrator/src/secrets/service.rs`; a `SecretsService<'a> { pool: &'a PgPool, keyring: &'a SecretsKeyring }` constructed by routes from `AppState`, or free functions with the same arguments; follow whatever style the auth epic used for its services.
- Transaction boundaries: `create` (scope check + insert), `replace_value` (load `FOR UPDATE` + update), `patch` (load `FOR UPDATE` + open + reseal + update) each in one `pool.begin()`. The `FOR UPDATE` prevents a concurrent rename and a rotation `update_wrap` from interleaving on one row.
- `fn authorize(actor: &Actor, scope: SecretScope, scope_id: Option<Uuid>) -> Result<()>`: `scope == User && scope_id != Some(actor.user_id) && !actor.admin` → `Error::Forbidden`. Global and project secrets are open to any authenticated user (v1 has no project membership).
- Value limit constant `MAX_SECRET_VALUE_BYTES: usize = 65_536` in `orchestrator/src/models/secret.rs` (add if the schema epic did not); values are `Zeroizing<String>` from deserialisation onward.

## Edge cases
- Rename to the same name: no crypto; still applies the flag if given.
- Rename never changes scope or `scope_id` (scope changes are delete + create); the AAD's scope part stays the same.
- Toggling `orchestrator_only` on a name listed in a profile's `secrets` array is allowed; the resolver (task 6) handles the skip at launch.
- A row whose `key_version` is unknown to the keyring cannot be renamed or replaced: log `error!(key_version)` and return `Error::Internal` (startup verification should make this unreachable).
- `patch` with `orchestrator_only` on a `GIT_CREDENTIAL` row is allowed (users may misconfigure; the git provider reads it regardless of the flag).

## Testing
- Integration tests in `orchestrator/tests/secrets_service.rs` via `TestApp` (call the service directly with the pool and keyring, users created through the users repository): create global/project/user; duplicate → `Conflict`; bad name → `BadRequest`; empty and oversize value → `BadRequest`; unknown project id → `BadRequest`; non-admin creating for another user → `Forbidden`; admin creating for another user succeeds; owner/other/admin matrix for `replace_value`, `patch`, `delete`, `uses`; rename re-encryption: after rename `crypto::open` under the new AAD succeeds, under the old AAD fails, `data_key_wrapped` unchanged, `updated_at` advanced; rename to an existing name → `Conflict`; flag-only patch leaves `ciphertext` unchanged; list visibility matrix including `scope` omitted for a non-admin and an admin; `uses` limit default and cap.
- `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- `SPEC.md` "Secrets (`/api/secrets`)": add one sentence stating what `GET /secrets` returns when `scope` is omitted (global, all project, and the caller's user secrets; all users for admins) and the default and maximum `limit` for `/uses` (50 / 500). The table leaves both unspecified; no other document changes.

## Assumes from other epics
- "Database schema, models, repositories and test harness": `Secret` model with `SecretError::InvalidName` and the name regex; `TestApp`; a users repository to create test users.