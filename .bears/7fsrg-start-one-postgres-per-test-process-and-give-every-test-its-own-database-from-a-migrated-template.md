---
id: "7fsrg"
title: Start one Postgres per test process and give every test its own database from a migrated template
status: done
priority: P1
created: "2026-09-17T20:27:12.643071038Z"
updated: "2026-09-17T21:42:36.522691642Z"
tags:
  - orchestrator
  - tests
  - docs
parent: yq6c3
assignee: claude-opus-subagent
attempts: 1
---

## Summary
Replace "a throw-away Postgres per test" in `tests/common/db.rs` with "a throw-away Postgres per test process and a throw-away database per test". The first test in a binary starts the `postgres:18` container, applies the crate's migrations to a template database once, and every test then gets `CREATE DATABASE test_<id> TEMPLATE mars_template` on that server, which takes milliseconds instead of the ~8 s a container start costs. `test_pool()`, `test_pool_with()`, `raw_pool*()` and `TestApp::spawn()` keep their signatures where possible so the 328 call sites do not change; only the guard type they return may change.

## Documents
- `CLAUDE.md` "Testing expectations" (backend integration tests use `TestApp::spawn()` from `tests/common/mod.rs`: testcontainers Postgres, migrations applied) and "Code quality" (`tests/health.rs` needs a container engine).
- `README.md` "Development" (`DOCKER_HOST` at the Podman socket for the tests).
- `docs/data-model.md` is untouched: the schema is the same, applied once to the template.

## Acceptance criteria
- [ ] `tests/common/db.rs` holds a process-wide `SharedPostgres` (a `tokio::sync::OnceCell` or `std::sync::OnceLock` around the container and an admin pool) started on first use; `test_pool()` returns a pool on a fresh database cloned from the migrated template; `raw_pool()` returns a fresh empty database (the migration round-trip test needs an unmigrated one); the `_with(max_connections)` variants survive.
- [ ] Each test's database is dropped when its guard drops (`DROP DATABASE ... WITH (FORCE)`), or is left to die with the container; either way a test never sees another test's rows, which `two_apps_run_side_by_side_on_independent_containers` and the race tests in `tests/common/races.rs` assert today and must still assert.
- [ ] The container does not outlive the test process: verify with `podman ps -a` empty after `cargo test --features integration-tests`. Choose one mechanism and document it in the module doc: testcontainers' resource reaper (check it works against rootless Podman on this machine; `TESTCONTAINERS_RYUK_*` variables), a `libc::atexit` hook that removes the container by id (the test harness ends with `process::exit`, which runs `atexit` handlers but no destructors), or the container started with auto-remove and stopped from the hook. A leaked container is a failure of this task, not a pitfall to record.
- [ ] `tests/engine.rs` keeps its own per-scenario containers and serial execution (`u6zkz`); it does not use the shared server.
- [ ] Wall time of `cargo test --features integration-tests` with `DOCKER_HOST` set is under 60 s on the development machine (measured before: 264 s); state the measured number in the commit message.
- [ ] `cargo test --features integration-tests --test health` still passes when run alone, and two binaries running concurrently (`cargo nextest` is not required, but `cargo test --test a & cargo test --test b`) each get their own server without a port clash: the container publishes a random host port as today.

## Implementation notes
- Files: `orchestrator/tests/common/db.rs`, `orchestrator/tests/common/app.rs` (the guard field type), `orchestrator/tests/common/races.rs`, `orchestrator/tests/migrations.rs`, `orchestrator/Cargo.toml` (`libc` as a dev-dependency only if the `atexit` route is taken), `CLAUDE.md`, `README.md`.
- The template: connect as the image's default superuser, `CREATE DATABASE mars_template`, run `MIGRATOR` on it once, then mark it `is_template = true` and disconnect from it so `TEMPLATE` clones do not fail with "source database is being accessed by other users"; clones need a pool per test on the new database name.
- Database names: `test_` plus a UUID without hyphens, 63-byte limit respected.
- `DEFAULT_MAX_CONNECTIONS` stays at 5 per test pool; Postgres 18's default `max_connections` of 100 is enough for a binary running its tests in parallel on 16 threads, but set `-c max_connections=300` on the container command to leave room for the fan-out tests.
- The `sqlx::migrate!` embedding pitfall in the skill (touch the files that use the `Migrator` after editing migrations) still applies; do not remove that note.

## Edge cases
- A test that panics before its guard drops leaves a database on the server; that is fine, the server dies with the process.
- The first test to call `test_pool()` pays the container start; tests must not assume timing.
- `tests/shutdown.rs` and other tests that build `AppState` without a database keep working with no container started.

## Testing
- `cd orchestrator && cargo fmt && cargo clippy --all-targets -- -D warnings && cargo clippy --all-targets --features integration-tests -- -D warnings && cargo test --features integration-tests` passes with `DOCKER_HOST` set; run it twice and confirm `podman ps -a` is empty after each.
- CI: the Orchestrator CI workflow runs the same suite against the runner's Docker; confirm the reaper or hook works there too (the workflow runs on the epic's closing push).

## Documentation
- `CLAUDE.md` "Testing expectations": one Postgres per test process, one database per test cloned from a migrated template; and "Code quality": the container engine is still needed.
- `README.md` "Development": the sentence on what the tests need, if it says "a container per test".
- Module doc of `tests/common/db.rs`: the mechanism and the reaper choice.