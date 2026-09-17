---
id: yy5rt
title: Add testcontainers Postgres harness and migration round-trip test
status: done
priority: P0
created: "2026-09-16T20:26:56.983296956Z"
updated: "2026-09-17T06:50:22.307186823Z"
tags:
  - orchestrator
  - core
  - tests
depends_on:
  - sywed
parent: p5tsd
assignee: claude-opus-subagent
attempts: 1
---

## Summary
Create the shared database test harness every repository test and `TestApp` builds on: a helper that starts a throw-away Postgres 18 through `testcontainers-modules`, returns a connected `PgPool`, and applies the crate's migrations. Add the migration round-trip test that proves every `.up.sql` applies and every `.down.sql` fully reverses it. With no migrations yet the round-trip test passes trivially; each migration task extends it.

## Documents
- `docs/data-model.md` "Migration list for v1" (migrations are applied at startup, each `.down.sql` drops exactly what its `.up.sql` created, in reverse order).
- `ARCHITECTURE.md` "Orchestrator internals" (crate table: `testcontainers-modules` (`postgres`), `sqlx`; `tests/` holds integration tests with `TestApp` on testcontainers Postgres).
- `CLAUDE.md` "Testing expectations", "Backend conventions" (migrations).

## Acceptance criteria
- [ ] `orchestrator/tests/common/mod.rs` exists with a `db` submodule exposing `pub async fn test_pool() -> (ContainerAsync<Postgres>, PgPool)` (or an equivalent guard struct that keeps the container alive) that starts image `postgres:18`, waits until it accepts connections and returns a pool with migrations applied.
- [ ] `pub async fn raw_pool()` (or a flag) returns a pool *without* migrations so the round-trip test controls migration state itself.
- [ ] `orchestrator/tests/migrations.rs` contains one `#[tokio::test]` that: applies `sqlx::migrate!("./migrations")` up; asserts `SELECT count(*) FROM _sqlx_migrations` equals the number of migrations in the directory; runs `Migrator::undo(&pool, 0)` down to nothing; asserts `information_schema.tables` for schema `public` contains only `_sqlx_migrations` and `pg_type WHERE typtype = 'e'` in schema `public` is empty; applies up again and asserts the count once more.
- [ ] The harness compiles with and without `--features integration-tests`.
- [ ] `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Implementation notes
- Files: `orchestrator/tests/common/mod.rs` (`pub mod db;` plus `#![allow(dead_code)]` because each test binary uses a subset), `orchestrator/tests/common/db.rs`, `orchestrator/tests/migrations.rs`.
- Add dev-dependencies with `cargo add --dev`: `testcontainers-modules --features postgres`, `testcontainers` if needed for `ContainerAsync`, `tokio` is already present. Follow the crate table in `ARCHITECTURE.md`; do not hand-edit versions.
- Build the connection string from the container's mapped port: `postgres://postgres:postgres@127.0.0.1:<port>/postgres` (fake credentials are fine; rule 3 of `CLAUDE.md` is about real ones).
- Use `sqlx::migrate!("./migrations")` (path relative to the crate root) so the same `Migrator` is used by `main.rs` at startup and by tests. Reversible migrations created with `sqlx migrate add -r` produce `<version>_<name>.up.sql` / `.down.sql`; the macro understands both.
- Pool options: `PgPoolOptions::new().max_connections(5)`; tests that need concurrency (event append) use more connections, so make the size a parameter with a default.
- Keep the container guard alive for the lifetime of the test (return it; dropping it stops Postgres).

## Edge cases
- The testcontainers client needs a reachable engine socket (`DOCKER_HOST`); document in the module doc comment that the harness uses the default socket resolution of `testcontainers` and that CI runners provide Docker.
- Two tests in one binary run in parallel by default; every test gets its own container, so no shared state.
- `Migrator::undo` requires every migration to be reversible; a missing `.down.sql` must fail the round-trip test, not be skipped.

## Testing
- `tests/migrations.rs` round-trip test described above.
- `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` must pass.

## Documentation
- None: implements the documented test infrastructure as written (`CLAUDE.md` "Testing expectations").

## Assumes from other epics
- "Repository scaffolding, tooling and CI": the `orchestrator/` crate exists with `sqlx` (`postgres`, `runtime-tokio`, `uuid`, `chrono`, `json`), an empty `orchestrator/migrations/` directory, the `integration-tests` cargo feature declared, and `main.rs` running `sqlx::migrate!("./migrations")` at startup.