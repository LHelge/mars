---
id: vbpey
title: Add a `healthcheck` subcommand to the orchestrator binary for compose health checks
status: done
priority: P1
created: "2026-09-16T20:40:34.048883391Z"
updated: "2026-09-19T15:53:00.317289805Z"
tags:
  - orchestrator
  - infra
depends_on:
  - s52qg
  - "2f5u2"
parent: "5czwa"
attempts: 1
---

## Summary
The orchestrator image contains `git` and the binary and nothing else (`ARCHITECTURE.md`, "Trust boundaries"), so a compose `healthcheck` cannot shell out to `curl` or `wget`. This task adds `mars-orchestrator healthcheck`, a subcommand of the same binary (the pattern `ARCHITECTURE.md` "Secrets" already establishes for `mars-orchestrator rotate-secrets`) that performs `GET http://127.0.0.1:<API_PORT>/api/health` and exits 0 on HTTP 200, 1 otherwise. The compose task uses it as the orchestrator service's health check.

## Documents
- `SPEC.md` "Health": `GET /api/health` → `{ orchestrator: true, database: bool, engine: bool }` with 200 or 503, unauthenticated, "for compose health checks".
- `ARCHITECTURE.md` "Trust boundaries" (orchestrator image: "contains `git` and nothing else beyond the binary"), "Secrets" (`mars-orchestrator rotate-secrets`, "a subcommand of the same binary"), "Orchestrator internals" (`main.rs`; no argument-parsing crate is in the Crates table, so argument handling is `std::env::args()`).
- `README.md` "Configuration" (`API_PORT`, default 7000), "Deployment shape" (orchestrator bullet).
- `CLAUDE.md` rule 1 and "Backend conventions" (`Result` everywhere; `unwrap`/`expect` only at startup; structured `tracing`).

## Acceptance criteria
- [ ] `mars-orchestrator healthcheck` reads `API_PORT` from `Config`-compatible sources (`.env` through `dotenvy`, then the environment; default 7000) **without** requiring the full `Config::from_env()` to succeed (the health check must not fail because, for example, `SECRETS_MASTER_KEYS` is unset in the health-check environment; it runs in the same container so normally everything is set, but the subcommand only needs the port).
- [ ] It sends `GET http://127.0.0.1:<API_PORT>/api/health` with `reqwest` (already a dependency, rustls), a 3 s connect timeout and a 5 s total timeout, and exits with status 0 if and only if the response status is 200. Any other status, timeout or connection error exits 1 and prints one line to stderr of the form `healthcheck: <reason>` (for example `healthcheck: status 503`, `healthcheck: connection refused`). Nothing is printed on success.
- [ ] The subcommand never initialises the database pool, the engine, migrations or tracing; it must run in well under a second when the API answers.
- [ ] `main.rs` dispatches on `std::env::args().nth(1)`: `None` → normal startup; `Some("healthcheck")` → this subcommand; `Some("rotate-secrets")` → left as a named hook for the Secrets manager epic if it does not exist yet (do not implement it here); any other value → print `usage: mars-orchestrator [healthcheck|rotate-secrets]` to stderr and exit 2.
- [ ] `README.md` "Deployment shape", orchestrator bullet, gains one sentence: the image ships only the binary and `git`, and compose health checks run `mars-orchestrator healthcheck`, which probes `/api/health` on the local API port. `ARCHITECTURE.md` "Trust boundaries" paragraph on the orchestrator container is not changed (the subcommand is part of the binary).
- [ ] `cd orchestrator && cargo fmt && cargo clippy --all-targets --features integration-tests -- -D warnings && cargo test --features integration-tests` pass.

## Implementation notes
- Files: `orchestrator/src/main.rs` (argument dispatch), new `orchestrator/src/healthcheck.rs` (or `src/cli/healthcheck.rs` if a `cli` module is preferred; keep it out of `routes/`), `README.md`.
- Put the probe in `pub async fn run_healthcheck(api_port: u16) -> std::process::ExitCode` in the library crate so it is unit-testable; `main.rs` only resolves the port and calls it.
- Port resolution: `dotenvy::dotenv().ok()` (and the `../.env` fallback the config task added, if exposed as a helper), then `std::env::var("API_PORT")` parsed as `u16`, default 7000; an unparsable value exits 1 with `healthcheck: invalid API_PORT`.
- Use `reqwest::Client::builder().connect_timeout(3s).timeout(5s).build()`; do not follow redirects; ignore the body.
- Bind address note: the API listener binds `0.0.0.0` (scaffolding epic), so `127.0.0.1` works inside the container.

## Edge cases
- Health returns 503 while the database or engine is down: exit 1 so compose marks the service unhealthy and `depends_on: condition: service_healthy` holds nginx back; this is intended.
- A `tokio` runtime is required for `reqwest`; build a small current-thread runtime for the subcommand or reuse `#[tokio::main]` and branch inside it; either is fine as long as the normal startup path is untouched.
- `RUST_LOG` must not affect the subcommand's output (no tracing init) so compose health logs stay clean.

## Testing
- Unit test in `healthcheck.rs`: bind an `axum` router on port 0 that answers `/api/health` with 200, call `run_healthcheck(port)` → `ExitCode::SUCCESS`; same with 503 → `ExitCode::FAILURE`; a closed port → `ExitCode::FAILURE` within the timeout.
- Integration: `cargo run -- healthcheck` against the running skeleton (manual, recorded in the PR).
- Command: `cd orchestrator && cargo fmt && cargo clippy --all-targets --features integration-tests -- -D warnings && cargo test --features integration-tests`.

## Documentation
- `README.md` "Deployment shape" (one sentence, same commit). No ADR: adding `curl` to the image was rejected by the existing image rule in `ARCHITECTURE.md`, not by a new decision.

## Assumes from other epics
- "Repository scaffolding, tooling and CI": `main.rs` startup, `Config`, `.env` loading, `GET /api/health` on `API_PORT`.
- "Secrets manager": owns `rotate-secrets`; this task only reserves the dispatch arm.