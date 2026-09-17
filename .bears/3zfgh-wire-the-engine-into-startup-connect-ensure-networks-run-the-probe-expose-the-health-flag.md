---
id: "3zfgh"
title: "Wire the engine into startup: connect, ensure networks, run the probe, expose the health flag"
status: done
priority: P1
created: "2026-09-16T20:30:21.205403371Z"
updated: "2026-09-17T18:38:27.121935500Z"
tags:
  - orchestrator
  - engine
depends_on:
  - jjzya
  - "2f25v"
  - dmnan
parent: naqhy
assignee: claude-opus-subagent
attempts: 1
---

## Summary
Connect the engine adapter to the running orchestrator: `main.rs` builds `BollardEngine` from `DOCKER_HOST` after migrations, creates the two session networks, runs the startup probe and aborts with a clear log line when it fails, then stores `Arc<dyn ContainerEngine>` in `AppState`; `GET /api/health` reports `engine` from `ping()`. This is the point at which the epic's "probe fails startup with a clear log line" criterion becomes observable, and it gives the Session lifecycle epic a ready `AppState.engine`.

## Documents
- `ARCHITECTURE.md` "Orchestrator internals" (`main.rs`: config, pool, migrations, listeners, recovery, spawn services; `AppState` holds `Arc<dyn ContainerEngine>`), "Engine adapter" → "Startup probe" ("Before adopting any session"), "Networks" (created at startup if missing), "Restart procedure" (probe precedes step 2)
- `SPEC.md` "Health" (`GET /api/health` → `{ orchestrator: true, database: bool, engine: bool }`, 200 or 503, unauthenticated)
- `README.md` "Configuration" (`DOCKER_HOST`, `SESSION_NETWORK_INTERNAL`, `SESSION_NETWORK_EGRESS`, `SESSION_EXTRA_HOSTS`, `SESSION_IMAGE_DEFAULT`, `DATA_DIR`, `DATA_DIR_HOST`), "Podman setup"

## Acceptance criteria
- [ ] Startup order in `main.rs`: config → pool → migrations → `BollardEngine::connect(&config.docker_host)` → `ensure_network(internal, true)` and `ensure_network(egress, false)` → `run_startup_probe` → build `AppState` → listeners → recovery → cron. Any engine failure before the listeners logs `error!` with `reason = %e` and exits non-zero; the process does not bind a port first.
- [ ] Probe failure log line is exactly `startup probe failed; refusing to start` with field `reason` carrying the `EngineError::Probe` message from the probe task (which already names the uid pair and the fix for each engine).
- [ ] `GET /api/health` returns `engine: true` when `ping()` succeeds and `engine: false` with status 503 when it fails; `database` behaviour is unchanged. The handler bounds `ping()` with a 2 s timeout so a hung socket cannot stall health checks.
- [ ] `TestApp::spawn()` builds `AppState` with `Arc<MockEngine>` and skips network creation and the probe (no filesystem, no socket); an integration test asserts `GET /api/health` → 200 with `engine: true` and, after `MockEngine::set_unhealthy(true)`, → 503 with `engine: false`.
- [ ] `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` pass.

## Implementation notes
- Files: `orchestrator/src/main.rs`, `orchestrator/src/prelude/state.rs` (or wherever `AppState` is defined: add `pub engine: Arc<dyn ContainerEngine>` if the scaffolding epic left a placeholder), the health route module (`orchestrator/src/routes/health.rs`), `orchestrator/tests/common/mod.rs` (mock wiring), `orchestrator/tests/health.rs`.
- Factor the engine bootstrap into `pub async fn bootstrap_engine(config: &Config) -> Result<Arc<dyn ContainerEngine>, EngineError>` in `engine/mod.rs` (connect + networks + probe) so `main.rs` stays a sequence of calls and the engine tests can call the same function end to end.
- `ProbeInput` comes from `Config`: `image = config.session_image_default`, `data_dir`, `data_dir_host`, `network_internal`, `network_egress`, `extra_hosts`.
- Log at `info`: `engine connected` with `engine_kind`, `engine_version`; `session networks ready` with `internal`, `egress`.
- Health JSON shape and status codes are fixed by `SPEC.md`; do not add fields.

## Edge cases
- `DOCKER_HOST` pointing at a socket the user cannot read: `connect` fails with `Connection`; the log must name the socket path once here (startup log only) so the operator can fix permissions.
- A probe that passes but `ensure_network` warns about a mismatched `internal` flag: startup continues (warning already logged by the engine task).
- Health must not create containers or networks; only `ping`.
- Do not run the probe on every health check or on reconnect; it is a startup gate only.

## Testing
- Integration (`TestApp`): health 200/503 with the mock's `set_unhealthy`; `AppState.engine.as_any().downcast_ref::<MockEngine>()` works from a test.
- Live: `bootstrap_engine` end-to-end is exercised by the engine test suite task on both engines (`DOCKER_HOST` set).
- `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` must pass.

## Documentation
- None: implements the documented contract as written (`README.md` "Podman setup" already states the refusal to start).

## Assumes from other epics
- "Repository scaffolding, tooling and CI": `main.rs` startup skeleton, `Config` fields above, `GET /api/health` with a `database` check.
- "Database schema, models, repositories and test harness": `TestApp::spawn()` exists and takes the engine from this epic's mock.
