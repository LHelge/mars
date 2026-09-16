---
id: t36d2
title: Secrets manager
type: epic
status: open
priority: P1
created: "2026-09-16T20:12:07.402364467Z"
updated: "2026-09-16T20:15:11.929002446Z"
tags:
  - orchestrator
  - secrets
depends_on:
  - qacxf
---

## Scope

Envelope encryption and the write-only secrets API (ADR 0006).

- `secrets/`: `SecretsKeyring` from `SECRETS_MASTER_KEYS` or `SECRETS_MASTER_KEY_FILE`, highest version for new rows, startup verification that one row per `key_version` present can be unwrapped; AES-256-GCM per-row data keys wrapped by the master key; AAD `<scope>:<scope_id or empty>:<name>`; zeroization of plaintext buffers.
- Rename re-encrypts under the new AAD in one transaction. Rotation: `mars-orchestrator rotate-secrets` subcommand and the re-wrap routine (batches of 100) that the cron epic schedules.
- Resolution at launch: `global` -> `project` -> `user` precedence, `orchestrator_only` rows never injected and never shadowed by a lower-precedence row, missing names reported as `launch_warning`, one `secret_uses` row per injected secret. Exposed as a function the session launcher calls.
- `GET/POST /api/secrets`, `PUT/PATCH/DELETE /api/secrets/{id}`, `GET /api/secrets/{id}/uses` with the user-scope ownership rule (403 for another user's secrets unless admin).
- Project `GIT_CREDENTIAL` convention: project-scoped orchestrator-only secret looked up by fixed name (consumed by the Git operations epic).

## Documents

`ARCHITECTURE.md` "Secrets"; `SPEC.md` "Secrets (`/api/secrets`)"; `docs/data-model.md` "Secrets"; `README.md` "Configuration" (master key rotation note); ADR 0006.

## Acceptance criteria

- [ ] Encrypt/decrypt round-trip, AAD mismatch failure, rename re-encryption and rotation re-wrap are unit-tested.
- [ ] Keyring refuses to start when a `key_version` in the table has no key.
- [ ] Every endpoint has happy-path, 401, 403 (foreign user scope), 400 (bad name) and 409 (duplicate) tests; no response ever contains a value.
- [ ] Resolution precedence and `orchestrator_only` shadowing rules are tested.

## Out of scope

File-based injection (documented hardening, non-goal); the cron trigger for rotation (Background jobs epic).