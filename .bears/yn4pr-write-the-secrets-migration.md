---
id: yn4pr
title: Write the secrets migration
status: open
priority: P0
created: "2026-09-16T20:31:32.329287655Z"
updated: "2026-09-16T20:31:32.329287655Z"
tags:
  - orchestrator
  - core
  - secrets
depends_on:
  - vnwqh
parent: p5tsd
---

## Summary
Create migration 6, `secrets`: the envelope-encrypted `secrets` table and the `secret_uses` audit table. It is the last migration of the initial set and must be created after `tasks` so the version order matches `docs/data-model.md`.

## Documents
- `docs/data-model.md` "Secrets" (`secrets`, `secret_uses`), "Enums" (`secret_scope`), "Migration list for v1" item 6.
- `ARCHITECTURE.md` "Secrets" (what each column holds; AAD; rotation sweeps by `key_version`).

## Acceptance criteria
- [ ] `secrets` (`id UUID PK`, `scope secret_scope NOT NULL`, `scope_id UUID NULL`, `name TEXT NOT NULL`, `ciphertext BYTEA NOT NULL`, `nonce BYTEA NOT NULL`, `data_key_wrapped BYTEA NOT NULL`, `data_key_nonce BYTEA NOT NULL`, `key_version INTEGER NOT NULL`, `orchestrator_only BOOLEAN NOT NULL DEFAULT FALSE`, `created_by UUID NULL REFERENCES users(id) ON DELETE SET NULL`, `created_at`, `updated_at`) with `UNIQUE NULLS NOT DISTINCT (scope, scope_id, name)`, `CHECK ((scope = 'global') = (scope_id IS NULL))` and index `secrets_key_version_idx (key_version)`.
- [ ] `scope_id` has **no** foreign key (the scope decides the target table).
- [ ] `secret_uses` (`id BIGINT PRIMARY KEY GENERATED ALWAYS AS IDENTITY`, `secret_id UUID NOT NULL REFERENCES secrets(id) ON DELETE CASCADE`, `session_id UUID NULL REFERENCES sessions(id) ON DELETE CASCADE`, `user_id UUID NULL REFERENCES users(id) ON DELETE SET NULL`, `purpose TEXT NOT NULL`, `at TIMESTAMPTZ NOT NULL DEFAULT NOW()`) with `secret_uses_secret_idx (secret_id, at DESC)` and `secret_uses_session_idx (session_id) WHERE session_id IS NOT NULL`.
- [ ] `purpose` is validated by the model (`launch` or `git`), not by a `CHECK`, because the document lists no constraint for it.
- [ ] Down drops `secret_uses` then `secrets`; round-trip test passes; schema assertion checks the unique constraint is declared `NULLS NOT DISTINCT` (`pg_constraint`/`pg_index` `indnullsnotdistinct`).

## Implementation notes
- File pair `orchestrator/migrations/<ts>_secrets.{up,down}.sql`.
- `UNIQUE NULLS NOT DISTINCT` requires Postgres 15+; the harness runs 18.
- Name the unique constraint `secrets_scope_scope_id_name_key` (the Postgres default) so the repository can match it for `Conflict`.

## Edge cases
- Inserting a `global` secret with a non-null `scope_id` or a `project` secret with a null one must fail the `CHECK`; add that to the schema test.

## Testing
- `tests/migrations.rs` round-trip and the schema assertions.
- `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` must pass.

## Documentation
- None: implements the documented schema as written.

## Assumes from other epics
- none.