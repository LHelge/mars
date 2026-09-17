---
id: p5tsd
title: Database schema, models, repositories and test harness
type: epic
status: done
priority: P0
created: "2026-09-16T20:11:44.236505473Z"
updated: "2026-09-17T08:29:34.016926725Z"
tags:
  - orchestrator
  - core
depends_on:
  - sywed
---

## Scope

Implement `docs/data-model.md` in full and the shared testing infrastructure that every backend epic uses.

- The six reversible migrations in the listed order: `enums`, `users` (with the seeded admin), `projects`, `sessions`, `tasks` (including the deferred `current_handoff_id`, `sessions.task_id`, `sessions.handoff_id` columns), `secrets`. Every `.down.sql` fully reverses its `.up.sql`.
- Domain models in `models/` with validation rules and a per-model error enum (username, password length, project name, state-name pattern, label pattern, secret-name pattern, shared-dir path rules, priority range, ...).
- One `XRepository<'a>` per aggregate in `repositories/` holding all SQL through `sqlx::query!`/`query_as!`, scope in the `WHERE` clause, helpers accepting the caller's transaction. This epic delivers the basic CRUD and locking primitives; feature epics add their domain queries to these repositories.
- The event-append primitive: lock session row, `MAX(seq)+1` insert, `pg_notify` in the same transaction (ADR 0021, 0028). The tracker mutation transaction primitive: lock project row, allocate task number, append `task_events` with notify.
- `cargo sqlx prepare` output committed under `.sqlx/`.
- `tests/common/mod.rs` with `TestApp::spawn()`: testcontainers Postgres, migrations applied, seeded admin removed, mock engine / email / git credentials, fixed test master key, `integration-tests` feature. Mock implementations may be stubs until their traits are defined by later epics; this epic defines the trait signatures they need.

## Documents

`docs/data-model.md` (all sections); `ARCHITECTURE.md` "Orchestrator internals", "Task tracker" (locking discipline); `CLAUDE.md` "Testing expectations".

## Acceptance criteria

- [ ] Migrations apply and roll back cleanly on Postgres 18; the schema matches every table, column, constraint, index and enum in `docs/data-model.md`.
- [ ] Model validation is unit-tested for each documented rule.
- [ ] `TestApp::spawn()` works and a smoke integration test runs against it.
- [ ] CI builds with `SQLX_OFFLINE=true`.

## Out of scope

Endpoint handlers and domain logic (feature epics).