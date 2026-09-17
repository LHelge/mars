---
id: suzac
title: Write the enums and users migrations with the seeded administrator
status: done
priority: P0
created: "2026-09-16T20:28:18.962593114Z"
updated: "2026-09-17T06:56:23.440470629Z"
tags:
  - orchestrator
  - core
  - auth
depends_on:
  - yy5rt
parent: p5tsd
assignee: claude-opus-subagent
attempts: 1
---

## Summary
Create the first two reversible migrations: `enums` (all seven Postgres enum types) and `users` (`users` with the seeded administrator, `refresh_tokens`, `user_invites`, `password_reset_tokens`). Both must apply and fully roll back on Postgres 18 and pass the round-trip test.

## Documents
- `docs/data-model.md` "Enums" (table of types and values), "Users and authentication" (`users`, `refresh_tokens`, `user_invites`, `password_reset_tokens` with every column, constraint and index), "Migration list for v1" items 1 and 2.
- `README.md` "Start" (seeded credentials `admin` / `changeme`; ADR 0024).
- `CLAUDE.md` "Backend conventions" (`sqlx migrate add -r <name>`; enum values only added, never removed or renamed; `docs/data-model.md` changes in the same commit).

## Acceptance criteria
- [ ] `sqlx migrate add -r enums` creates `.up.sql` with `CREATE TYPE project_status AS ENUM ('cloning','ready','error')`, `profile_kind ('conversational','ephemeral')`, `agent_backend ('claude')`, `session_state ('creating','running','parked','done','failed')`, `task_state_kind ('queue','human','terminal')`, `task_dependency_kind ('blocks','discovered_from','related')`, `secret_scope ('global','user','project')`; `.down.sql` drops the seven types in reverse order.
- [ ] `sqlx migrate add -r users` creates `users` (`id UUID PK`, `username TEXT NOT NULL UNIQUE`, `email TEXT NOT NULL UNIQUE`, `password_hash TEXT NOT NULL`, `auth_version BIGINT NOT NULL DEFAULT 0 CHECK (auth_version >= 0)`, `must_change_password BOOLEAN NOT NULL DEFAULT FALSE`, `admin BOOLEAN NOT NULL DEFAULT FALSE`, `notify_email BOOLEAN NOT NULL DEFAULT TRUE`, `created_at`/`updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()`).
- [ ] `refresh_tokens` (`id UUID PK`, `user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE`, `token_hash TEXT NOT NULL UNIQUE`, `expires_at TIMESTAMPTZ NOT NULL`, `revoked_at TIMESTAMPTZ NULL`, `created_at`) with indexes `refresh_tokens_user_id_idx (user_id)` and `refresh_tokens_expires_at_idx (expires_at)`.
- [ ] `user_invites` (`id`, `email TEXT NOT NULL`, `token_hash TEXT NOT NULL UNIQUE`, `admin BOOLEAN NOT NULL DEFAULT FALSE`, `invited_by UUID NULL REFERENCES users(id) ON DELETE SET NULL`, `expires_at NOT NULL`, `accepted_at NULL`, `accepted_user_id UUID NULL REFERENCES users(id) ON DELETE SET NULL`, `created_at`) with `CREATE UNIQUE INDEX user_invites_open_email_idx ON user_invites (email) WHERE accepted_at IS NULL` and `user_invites_expires_at_idx (expires_at)`.
- [ ] `password_reset_tokens` (`id`, `user_id ... ON DELETE CASCADE`, `token_hash TEXT NOT NULL UNIQUE`, `expires_at NOT NULL`, `used_at NULL`, `created_at`) with `password_reset_tokens_user_id_idx (user_id)` and `password_reset_tokens_expires_at_idx (expires_at)`.
- [ ] The `users` up migration inserts the administrator: `id = '00000000-0000-0000-0000-000000000001'`, `username = 'admin'`, `email = 'admin@localhost'`, `password_hash` = an Argon2id PHC string of `changeme`, `must_change_password = TRUE`, `admin = TRUE`.
- [ ] The `users` down migration drops `password_reset_tokens`, `user_invites`, `refresh_tokens`, `users` in that order (the seeded row goes with the table).
- [ ] The round-trip test in `tests/migrations.rs` passes; a new test verifies the seeded hash: after migrating, `argon2::Argon2::default().verify_password(b"changeme", &PasswordHash::new(&hash)?)` succeeds and `must_change_password` and `admin` are true.
- [ ] `docs/data-model.md` `users` section records the seeded email `admin@localhost` in the same commit.

## Implementation notes
- Files: `orchestrator/migrations/<ts>_enums.up.sql`, `.down.sql`; `<ts>_users.up.sql`, `.down.sql`. Create them with `sqlx migrate add -r` so the version prefixes keep the documented order (`enums` before `users`).
- Generate the seed hash once with the `argon2` crate (default `Argon2id` parameters, random salt) and paste the PHC string into the migration as a literal; it is a fixed default documented in `README.md`, not a real credential. Do not compute it in SQL.
- `sha2`/`argon2` are already in the crate table; add `argon2` with `cargo add` if the scaffolding did not.
- Use `TIMESTAMPTZ` everywhere; `NOW()` defaults exactly as listed.
- Name every index exactly as the document does; unnamed constraints are acceptable only where the document gives no name (the `CHECK`s).

## Edge cases
- `CREATE TYPE` is not transactional-safe across `ADD VALUE` in the same transaction, but the initial create is fine; later value additions are separate migrations.
- The partial unique index on `user_invites` must be created with `CREATE UNIQUE INDEX ... WHERE accepted_at IS NULL`, not a table constraint.
- The seeded admin row must be idempotent-safe for a fresh database only; migrations run once, so no `ON CONFLICT` is needed.

## Testing
- Extend `tests/migrations.rs`: round-trip still passes; add `seeded_admin_is_present_with_changeme_hash`.
- `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` must pass.

## Documentation
- `docs/data-model.md` `users`: add the seeded administrator's email (`admin@localhost`) next to the fixed id. Nothing else changes.

## Assumes from other epics
- "Repository scaffolding, tooling and CI": `orchestrator/migrations/` exists and `main.rs` applies migrations at startup.