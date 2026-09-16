---
id: d9h6f
title: "Implement BollardEngine container lifecycle: create, start, stop, kill, remove, inspect, wait, list, pull, connect network"
status: open
priority: P1
created: "2026-09-16T20:28:14.680136717Z"
updated: "2026-09-16T20:28:14.680136717Z"
tags:
  - orchestrator
  - engine
depends_on:
  - kjvte
  - jjzya
parent: naqhy
---

## Summary
Fill in the container lifecycle half of `BollardEngine`: creating a container from a `ContainerSpec` through `to_bollard`, connecting it to the egress network before start, starting, stopping, killing with a named signal, removing, inspecting, waiting for exit, listing by label and pulling an absent image. These are the operations the launcher, stop semantics, recovery and orphan cleanup rely on, using only the endpoints in the "Engine adapter" table.

## Documents
- `ARCHITECTURE.md` "Engine adapter" (operation table; "`kill` with a named signal is used for SIGINT/SIGTERM"; "Recovery lists `mars.session_id`"; "connect to a second network before start"; "image pull at launch, when the image is absent")
- `ARCHITECTURE.md` "Session container specification" (Image row: pulled at launch if absent, pull failure fails the launch with the engine's message; Networks row)
- `ARCHITECTURE.md` "Stop semantics", "Restart procedure" step 2, "Background jobs" orphan cleanup
- `docs/data-model.md` `sessions.container_id`
- ADR 0004

## Acceptance criteria
- [ ] `create(&spec)` calls `to_bollard(spec, self.kind)`, passes `CreateContainerOptions { name: spec.name, platform: None }` and returns the engine id as `ContainerId`; a name conflict surfaces as `EngineError::Conflict`.
- [ ] `connect_network(id, network)` connects with default endpoint settings; connecting a container that is already on the network is `Ok(())` (Docker returns 403/409 "already exists": map both to success only for this call).
- [ ] `start`, `stop(grace_secs)` (`StopContainerOptions { t: grace_secs }`), `kill(signal)` (`KillContainerOptions { signal: signal.to_string() }`), `remove(force)` (`RemoveContainerOptions { force, v: false, link: false }`) behave as documented; `remove` on a missing container returns `Ok(())`; `kill` on an already-exited container returns `EngineError::Conflict` (the engine's 409) so the owner can treat it as "already gone".
- [ ] `inspect(id)` maps `ContainerInspectResponse` to `ContainerInfo` (state string → `ContainerState`, `exit_code`, `NetworkSettings.Networks` keys → `networks`, `State.Pid` → `pid`); 404 → `EngineError::NotFound`.
- [ ] `wait(id)` uses `WaitContainerOptions { condition: "not-running" }` and returns `ExitStatus { code: status_code, oom_killed }` (`oom_killed` from a follow-up `inspect`); if the container is already exited it returns immediately with that code.
- [ ] `list_by_label(key)` lists with `all: true` and filter `label=<key>` and maps to `ContainerSummary { running: state == "running" }`.
- [ ] `image_exists` uses `inspect_image` (404 → `false`); `pull_image` drives `create_image(CreateImageOptions { from_image: image })` to completion and converts any error item in the stream to `EngineError::ImagePull { image, message }` with the engine's message verbatim.
- [ ] `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` pass.

## Implementation notes
- File: `orchestrator/src/engine/bollard.rs` (replace the `Unsupported` stubs from the previous task).
- Image references with a tag or digest are passed through unchanged; do not default a tag (`latest`) yourself — bollard/the engine does.
- `pull_image` must log progress at `debug` only, with `image = %image`; the stream is verbose.
- `wait`: bollard returns a stream of `ContainerWaitResponse`; take the first item. Podman's compat API also supports `not-running`. If the stream ends without an item, `inspect` and derive the code from `state`.
- `inspect` state mapping: `"created"` → `Created`, `"running"` → `Running`, `"paused"` → `Paused`, `"exited"` → `Exited { code }`, `"removing"` → `Removing`, `"dead"` → `Dead`, other → `Unknown(s)`.
- No new `HostConfig` fields: everything comes from `to_bollard`. Do not set `Init`; whether it is needed is open question 7 and is decided by the engine test suite task.
- Keep every method a thin call plus mapping; retries and grace timing belong to the session owner.

## Edge cases
- `create` when the image is absent returns 404 from the engine; do not auto-pull inside `create` — the launcher calls `image_exists` then `pull_image` first so the pull failure message can be stored in `sessions.error`. Map the 404 to `EngineError::NotFound("image ...")` so a race (image removed between check and create) is still a clear error.
- `stop` on a container that already exited: engine returns 304; map to `Ok(())`.
- `remove(force = true)` on a running container must succeed (used by orphan cleanup and end-of-session).
- Docker returns 403 "endpoint with name ... already exists" and Podman 409 on double `connect_network`; treat both as `Ok` only when the message contains `already` — otherwise propagate.
- Never log `ContainerSpec.env` values; log `container = %id, name = %spec.name` on create.

## Testing
- Unit tests in `bollard.rs`: `ContainerState` string mapping; `ContainerInspectResponse` → `ContainerInfo` mapping from a constructed response (networks keys, exit code, pid); `ContainerSummary.running` mapping.
- Live behaviour of every method is exercised by `tests/engine.rs` (engine test suite task) on both engines; list there the scenarios this task must satisfy: create/start/wait exit code, kill SIGINT/SIGTERM, remove missing container ok, list by label, pull absent image, connect second network before start visible in `inspect().networks`.
- `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` must pass.

## Documentation
- None: implements the documented contract as written.

## Assumes from other epics
- none beyond the scaffolding crate.
