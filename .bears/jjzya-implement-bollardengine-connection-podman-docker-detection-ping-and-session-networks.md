---
id: jjzya
title: Implement BollardEngine connection, Podman/Docker detection, ping and session networks
status: done
priority: P1
created: "2026-09-16T20:27:43.136998691Z"
updated: "2026-09-17T16:42:20.151568564Z"
tags:
  - orchestrator
  - engine
depends_on:
  - "84dxt"
parent: naqhy
assignee: claude-opus-subagent
attempts: 1
---

## Summary
Create `orchestrator/src/engine/bollard.rs` with the `BollardEngine` struct: connecting to the socket named by `DOCKER_HOST`, detecting whether the engine is Podman or Docker from `/version` once at connect time, implementing `ping` for the health endpoint, and `ensure_network` so the two session networks (`SESSION_NETWORK_INTERNAL` as `internal: true`, `SESSION_NETWORK_EGRESS` as an ordinary bridge) exist at startup. Container operations are added in the next tasks; this task establishes the struct, the error mapping and the engine-kind rule the spec builder depends on.

## Documents
- `ARCHITECTURE.md` "Engine adapter" (`UsernsMode` "Set when `/version` reports Podman"; `ExtraHosts` on Podman 4.x+)
- `ARCHITECTURE.md` "Networks" (`mars-sessions` `internal: true`, no gateway; `mars-egress` ordinary bridge; both created at startup if missing, names from config)
- `README.md` "Configuration" (`DOCKER_HOST`, `SESSION_NETWORK_INTERNAL`, `SESSION_NETWORK_EGRESS`), "Podman setup", "Running locally" (socket URLs)
- `SPEC.md` "Health" (`engine: bool`)
- ADR 0004

## Acceptance criteria
- [ ] `BollardEngine::connect(docker_host: &str) -> Result<BollardEngine, EngineError>` accepts `unix://<path>` (bollard `connect_with_socket`) and `tcp://` / `http://` (`connect_with_http`), fails with `EngineError::Connection` naming the scheme for anything else, and calls `/version` immediately so a dead socket fails at startup rather than at first launch.
- [ ] `kind()` returns `EngineKind::Podman` when `/version` reports a component whose name contains `Podman` (case-insensitive) or `Platform.Name` contains `Podman`; otherwise `Docker`. The detection is logged once at `info` with `engine_kind = %kind` and `engine_version = %version`.
- [ ] `ping()` maps `GET /_ping` success to `Ok(())` and any failure to `EngineError::Connection`.
- [ ] `ensure_network(name, internal)` inspects the network, creates it with driver `bridge` and `Internal: internal` when it gets 404, treats a 409 on create as success (concurrent creation), and returns `Ok(())` when the network already exists even if its `internal` flag differs (log a `warn!` naming the network in that case; do not recreate).
- [ ] All `bollard::errors::Error` values are converted by one `impl From<bollard::errors::Error> for EngineError`: `DockerResponseServerError { status_code: 404 }` → `NotFound(message)`, `409` → `Conflict(message)`, other server errors → `Api { status, message }`, transport/IO/hyper errors → `Connection(message)`.
- [ ] `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` pass.

## Implementation notes
- File: `orchestrator/src/engine/bollard.rs`. Struct: `pub struct BollardEngine { docker: bollard::Docker, kind: EngineKind, version: String }`. Implement `ContainerEngine` for it with the remaining methods returning `todo!()`-free stubs that return `EngineError::Unsupported("not implemented")` until the next tasks replace them (clippy must stay clean; no `todo!` in non-test code).
- Use bollard's `API_DEFAULT_VERSION` and a 120 s timeout for `connect_with_socket`; strip the `unix://` prefix before passing the path.
- `/version`: `docker.version().await` → `bollard::models::SystemVersion { components: Option<Vec<ComponentVersion>>, platform: Option<SystemVersionPlatform>, version: Option<String>, .. }`. Podman's compat API reports a component named `Podman Engine`.
- Networks: `docker.inspect_network::<String>(name, None)`; on `NotFound` call `docker.create_network(CreateNetworkOptions { name, driver: "bridge", internal, check_duplicate: true, ..Default::default() })`. Keep `EnableIPv6`, `IPAM` and `Options` unset (outside the table).
- Log with structured fields (`network = %name`), never string-formatted.
- Health: the scaffolding epic's `GET /api/health` currently reports `engine: bool`; the wiring task (later in this epic) will call `ping()`; this task only provides it.

## Edge cases
- `DOCKER_HOST` unset: bollard's `connect_with_defaults` would fall back to `/var/run/docker.sock`; do not use it. `Config::from_env()` must treat `DOCKER_HOST` as required (it is listed without a default in `README.md`) — if the scaffolding epic made it optional, keep the fallback out of `engine/` and let `main.rs` fail fast.
- Podman ≥ 5 reports API version 1.41 through the compat socket; bollard's default (1.44+) may negotiate down. If `version()` fails with a version-mismatch message, retry once with `docker.negotiate_version()` and log the negotiated version.
- A network that exists but is attached to the wrong driver (`macvlan`) is left alone with a `warn!`; recreating a network in use is not this component's business.
- Never include the socket path in error messages returned to HTTP callers; it stays in the startup log only.

## Testing
- Unit tests in `bollard.rs` for the pure parts: the `EngineKind` decision from a constructed `SystemVersion` (Podman component, Podman platform name, plain Docker, empty components); `From<bollard::errors::Error>` status mapping (404 → `NotFound`, 409 → `Conflict`, 500 → `Api`); `connect` scheme rejection for `ssh://` and an empty string (no socket needed: the rejection happens before connecting).
- Engine-backed behaviour (`ensure_network` idempotence, `ping`) is covered by `tests/engine.rs` in the engine test suite task; do not add a socket-requiring test here.
- `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` must pass.

## Documentation
- None: implements the documented contract as written. If the version-negotiation retry turns out to be required on Podman 5, add one sentence to `README.md` "Podman setup".

## Assumes from other epics
- "Repository scaffolding, tooling and CI": `bollard` is in `Cargo.toml`; `Config` exposes `docker_host`, `session_network_internal`, `session_network_egress`.
