---
id: mv5w9
title: "Add the Orchestrator CI workflow: fmt, clippy -D warnings and tests with SQLX_OFFLINE=true"
status: open
priority: P1
created: "2026-09-16T20:29:26.192364222Z"
updated: "2026-09-16T20:29:26.192364222Z"
tags:
  - infra
  - orchestrator
depends_on:
  - apjkw
parent: sywed
---

## Summary
Add `.github/workflows/orchestrator.yml`, the "Orchestrator CI" workflow from the `README.md` "CI" table: on pushes to `main` and pull requests touching `orchestrator/**`, run `cargo fmt --check`, `cargo clippy -- -D warnings` (with and without `integration-tests`), and `cargo test --features integration-tests` with `SQLX_OFFLINE=true`. Integration tests use testcontainers, so the job relies on the Docker daemon present on `ubuntu-latest`.

## Documents
- `README.md` "CI": row `Orchestrator CI | orchestrator/** | fmt, clippy, tests with SQLX_OFFLINE=true`.
- `CLAUDE.md` "Backend conventions": "CI builds with `SQLX_OFFLINE=true`; 'no cached data for this query' means `.sqlx/` is stale"; "Code quality" backend chain; "Testing expectations": integration tests use testcontainers Postgres; engine tests run only when `DOCKER_HOST` is set (not here).

## Acceptance criteria
- [ ] `.github/workflows/orchestrator.yml` named `Orchestrator CI`, triggers `push` (branches `main`) and `pull_request`, `paths: ["orchestrator/**", ".github/workflows/orchestrator.yml"]`; `concurrency` group `orchestrator-${{ github.ref }}` with cancel-in-progress; `permissions: contents: read`.
- [ ] `env: { SQLX_OFFLINE: "true", CARGO_TERM_COLOR: always, RUST_BACKTRACE: "1" }` at workflow level; `DOCKER_HOST` is **not** set, so `tests/engine.rs` (Container engine epic) stays skipped here.
- [ ] One job `check` on `ubuntu-latest`, `defaults.run.working-directory: orchestrator`, steps: `actions/checkout@v4`; a toolchain step that honours `rust-toolchain.toml` (`dtolnay/rust-toolchain@stable` reads it, or `rustup show` to trigger installation) with `rustfmt` and `clippy`; `Swatinem/rust-cache@v2` with `workspaces: orchestrator`; `cargo fmt --all -- --check`; `cargo clippy --all-targets -- -D warnings`; `cargo clippy --all-targets --features integration-tests -- -D warnings`; `cargo test --features integration-tests`.
- [ ] `git` is available on the runner (it is on `ubuntu-latest`) so the Git operations epic's real-repository tests run unchanged.
- [ ] The workflow is green on the scaffold (`tests/health.rs` pulls the Postgres 18 testcontainer successfully), and the run URL is linked in the PR.

## Implementation notes
- Files: `.github/workflows/orchestrator.yml`.
- `cargo sqlx prepare` is not run in CI; `.sqlx/` is committed by the Database epic and `SQLX_OFFLINE=true` makes a stale cache fail the build, which is the intended signal.
- Pin third-party actions by major tag; no `@master`.
- Keep a single job; splitting fmt/clippy/test into a matrix is not worth three toolchain installs at this size.
- Set `timeout-minutes: 30` on the job.

## Edge cases
- testcontainers needs `/var/run/docker.sock`; `ubuntu-latest` provides it for the runner user without extra setup. If a future runner change breaks this, the workflow should fail loudly rather than skip tests.
- `cargo test` without `--features integration-tests` is deliberately not run (mocks are behind the feature; the plain build is covered by the first clippy step).

## Testing
- PR with the workflow shows the check; `main` run after merge is green.
- Local equivalent: `cd orchestrator && SQLX_OFFLINE=true cargo fmt --all -- --check && cargo clippy --all-targets --features integration-tests -- -D warnings && cargo test --features integration-tests`.

## Documentation
- none: implements `README.md` "CI" as written.

## Assumes from other epics
- Database epic commits `.sqlx/`; Container engine epic adds a separate Podman/Docker matrix job for `tests/engine.rs` with `DOCKER_HOST` set (that job is theirs, not a placeholder here).