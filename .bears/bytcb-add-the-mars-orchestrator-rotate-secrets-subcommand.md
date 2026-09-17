---
id: bytcb
title: Add the mars-orchestrator rotate-secrets subcommand
status: in_progress
priority: P2
created: "2026-09-16T20:32:24.245693491Z"
updated: "2026-09-17T13:57:07.006194103Z"
tags:
  - orchestrator
  - secrets
  - docs
depends_on:
  - sz5t2
parent: t36d2
assignee: claude-opus-subagent
attempts: 1
---

## Summary
Add the `rotate-secrets` subcommand to the `mars-orchestrator` binary so an operator can re-wrap rows immediately after adding a new master key instead of waiting for the hourly job. It loads config, connects, runs the keyring startup verification and `rewrap_outdated`, prints the report and exits with a code scripts can act on.

## Documents
- `ARCHITECTURE.md` "Secrets" → "Rotation" ("run `mars-orchestrator rotate-secrets` (a subcommand of the same binary) or wait for the cron job")
- `README.md` "Operating notes" rotation bullet; "Configuration" (`SECRETS_MASTER_KEYS`)
- `ARCHITECTURE.md` "Orchestrator internals" (crate table is binding; `main.rs` startup order)

## Acceptance criteria
- [ ] `mars-orchestrator rotate-secrets` runs `Config::from_env()`, opens the pool, runs migrations (so the schema is current), builds the keyring, runs `verify_against_db`, then `rewrap_outdated`, prints `rewrapped=<n> skipped=<n> remaining=<n>` on stdout and exits 0; exits 1 with the error on stderr when configuration, verification or the sweep fails; exits 2 when `remaining > 0` after a successful sweep so scripts can tell "not finished" from "done".
- [ ] `mars-orchestrator` with no arguments starts the server exactly as before; any other argument prints `usage: mars-orchestrator [rotate-secrets]` to stderr and exits 64.
- [ ] The subcommand does not start the HTTP or MCP listeners, the session registry, recovery or cron.
- [ ] The binary is named `mars-orchestrator` (`[[bin]] name` in `orchestrator/Cargo.toml` if the package name differs).

## Implementation notes
- Files: `orchestrator/src/main.rs` (dispatch on `std::env::args().nth(1)`), optionally `orchestrator/src/cli.rs` for the subcommand body.
- Do not add `clap`: the crate table in `ARCHITECTURE.md` is binding and one subcommand does not justify a dependency. If a later task needs a real argument parser it adds the crate and the table row in the same commit.
- Factor the shared bootstrap (`Config` → pool → migrations → keyring + `verify_against_db`) out of `main` into a `bootstrap()` helper used by both paths so the server and the subcommand cannot drift.
- `tracing` is initialised in both paths; the report line goes to stdout via `println!` in addition to the `info!` log.

## Edge cases
- `DATABASE_URL` unreachable → exit 1 with the connection error.
- Running while the server is up is safe by design (row-guarded updates in `rewrap_outdated`); say so in the README bullet.
- A newest key that no row uses yet and no outdated rows → exit 0 with all zeros.

## Testing
- Integration test in `orchestrator/tests/rotate_secrets_cli.rs` behind `integration-tests`: spawn the built binary (`env!("CARGO_BIN_EXE_mars-orchestrator")`) with `DATABASE_URL` from the `TestApp` container and `SECRETS_MASTER_KEYS` carrying versions 1 and 2 after seeding rows under version 1 through the repository; assert exit 0, the stdout report, and every row on version 2; run with a row under an unconfigured version present → exit 1; run with a bogus argument → exit 64. Provide the other required variables from the same values `TestApp` uses.
- `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- `README.md` "Operating notes": extend the rotation bullet with the invocation (`podman-compose exec orchestrator mars-orchestrator rotate-secrets`, or `cargo run -- rotate-secrets` on a development host), the exit codes 0 / 1 / 2, and that it is safe while the server runs.

## Assumes from other epics
- "Repository scaffolding, tooling and CI": `main.rs` startup order and `Config::from_env()`.
- "Deployment packaging: compose, nginx and orchestrator image": the orchestrator image keeps the binary on `PATH` so `exec ... mars-orchestrator rotate-secrets` works.