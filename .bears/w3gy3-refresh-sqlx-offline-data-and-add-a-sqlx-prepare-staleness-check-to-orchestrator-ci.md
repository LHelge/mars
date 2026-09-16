---
id: w3gy3
title: Refresh .sqlx offline data and add a sqlx prepare staleness check to Orchestrator CI
status: open
priority: P2
created: "2026-09-16T20:33:29.826633868Z"
updated: "2026-09-16T20:33:29.826633868Z"
tags:
  - orchestrator
  - core
  - infra
  - docs
depends_on:
  - fcezm
  - "2xrbu"
  - qwesr
  - rvddq
  - dm7hz
  - bptgh
parent: p5tsd
---

## Summary
Close the epic's last acceptance criterion: with every repository merged, regenerate `.sqlx/` once from a clean database, prove the crate builds and tests with `SQLX_OFFLINE=true`, and make Orchestrator CI fail when `.sqlx/` is stale by running `cargo sqlx prepare --check` against a service Postgres. Record the workflow in `README.md`, "CI".

## Documents
- `CLAUDE.md` "Backend conventions" (after changing any query, run `cargo sqlx prepare` in `orchestrator/` and commit `.sqlx/`; CI builds with `SQLX_OFFLINE=true`; "no cached data for this query" means `.sqlx/` is stale).
- `README.md` "CI" (Orchestrator CI: fmt, clippy, tests with `SQLX_OFFLINE=true`), "Running locally" (Postgres container command).
- Epic acceptance criterion: "CI builds with `SQLX_OFFLINE=true`".

## Acceptance criteria
- [ ] `orchestrator/.sqlx/` contains exactly one `query-<hash>.json` per `sqlx::query!`/`query_as!` call site after `cargo sqlx prepare --workspace` (or `cargo sqlx prepare` in `orchestrator/`) run against a database with all six migrations applied; stale files removed.
- [ ] `SQLX_OFFLINE=true cargo build --features integration-tests --tests` and `SQLX_OFFLINE=true cargo clippy --all-targets --features integration-tests -- -D warnings` succeed with no `DATABASE_URL` set.
- [ ] The Orchestrator CI workflow gains a job (or step) that starts `postgres:18` as a service, sets `DATABASE_URL`, runs `sqlx migrate run` and `cargo sqlx prepare --check`, so a PR with a changed query and an unrefreshed `.sqlx/` fails.
- [ ] The migration round-trip test and all repository tests run in CI (the runner provides Docker for testcontainers); the workflow shows them green on `main`.
- [ ] `README.md` "CI" table row for Orchestrator CI lists the `.sqlx/` staleness check.
- [ ] `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes locally.

## Implementation notes
- Files: `orchestrator/.sqlx/*.json`, `.github/workflows/orchestrator.yml` (name per the scaffolding epic), `README.md`.
- Install the CLI in CI with `cargo install sqlx-cli --no-default-features --features postgres` (cache it) or use a prebuilt binary action; pin the version to the one matching the `sqlx` crate in `Cargo.lock`.
- `cargo sqlx prepare --check` requires the same feature set as the build, so pass `-- --features integration-tests --all-targets`.
- Keep the existing fmt/clippy/test steps running with `SQLX_OFFLINE=true` and no database, as the README already documents; the prepare check is an additional job.

## Edge cases
- `query!` macros inside `#[cfg(feature = "integration-tests")]` code must be included in the prepare run or offline builds with the feature fail; the `--all-targets --features integration-tests` arguments cover it.
- Deleting obsolete `query-*.json` files is part of the refresh; `cargo sqlx prepare` does not remove them by itself in every version, check with `git status`.

## Testing
- The CI run itself; locally reproduce the check with the README Postgres container: `sqlx migrate run && cargo sqlx prepare --check -- --all-targets --features integration-tests`.
- `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` must pass.

## Documentation
- `README.md` "CI": Orchestrator CI row mentions the `.sqlx/` staleness check.

## Assumes from other epics
- "Repository scaffolding, tooling and CI": the Orchestrator CI workflow file exists with fmt, clippy and `SQLX_OFFLINE=true` test steps.