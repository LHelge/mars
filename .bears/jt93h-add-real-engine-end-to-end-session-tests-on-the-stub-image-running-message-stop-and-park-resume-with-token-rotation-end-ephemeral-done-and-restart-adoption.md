---
id: jt93h
title: "Add real-engine end-to-end session tests on the stub image: running, message, stop and park, resume with token rotation, end, ephemeral done and restart adoption"
status: open
priority: P2
created: "2026-09-16T20:35:39.864280371Z"
updated: "2026-09-16T20:35:39.864280371Z"
tags:
  - orchestrator
  - sessions
  - engine
  - tests
depends_on:
  - "9a3gw"
  - gy74f
parent: s52qg
---

## Summary
Add `orchestrator/tests/session_e2e.rs`, a test binary that runs only when `DOCKER_HOST` is set (like `tests/engine.rs`) and drives the full lifecycle against a real engine with the stub session image: a conversational session reaches `running`, accepts a message and shows the replayed turn, parks on stop with the signal recorded, resumes on the next message with a rotated token and `resumed: true`, ends with the branch fetched back; an ephemeral session reaches `done` with cost accumulated; and a simulated orchestrator restart adopts the running container without rotating the token. This is the epic's third and fourth acceptance criteria on real containers.

## Documents
- `ARCHITECTURE.md` "Session image" (stub contract: emits `system`/`init`, replays a fixture transcript one turn per stdin line or the whole file under `-p`, exits cleanly on `SIGINT`), "Launch sequence", "Stop semantics", "Restart procedure", "Session container specification", "Development on the host" (`DATA_DIR = DATA_DIR_HOST`, `MCP_URL` through the host gateway, `SESSION_EXTRA_HOSTS`), "Uid contract".
- `SPEC.md` "Sessions" (endpoints used), "AgentEvent" (`init.resumed`, `state_change.signal`, `result.cost_usd`).
- `CLAUDE.md` "Testing expectations" (engine tests run only with `DOCKER_HOST`; CI runs them on both Podman and Docker).
- `README.md` "CI" (Orchestrator CI and E2E rows).
- ADRs 0003, 0010, 0029.

## Acceptance criteria
- [ ] The test binary starts with `if std::env::var("DOCKER_HOST").is_err() { return; }` in every test (or a shared guard) and reads the stub image name from `MARS_STUB_IMAGE` (default `mars-session-stub:dev`); it fails clearly if the image is absent (no silent skip when `DOCKER_HOST` is set).
- [ ] `TestApp::spawn_with_engine()` (or an option on `spawn`) builds the app with the real bollard engine, a `tempfile` data directory used as both `DATA_DIR` and `DATA_DIR_HOST`, `MCP_URL` pointing at the host gateway, and a project created from a local bare repository under that directory.
- [ ] Conversational scenario: `POST /projects/{pid}/sessions {profile_id}` with a profile whose `image` is the stub → within 30 s the session is `running`, `events` contains `init` with `resumed: false` and a `state_change creating → running`, `cli_session_id` is set, the container `mars-session-<sid>` exists with the labels and the `mcp.json` bind; `POST .../input {message}` → the stub replays one turn and `text`/`tool_call`/`tool_result`/`result` events appear with contiguous `seq`, `cost_usd` > 0 after `result`; `POST .../stop` → within `STOP_GRACE_SECS` the session is `parked`, `state_change` has `signal: "SIGINT"`, the container is gone, `container_id` null; `POST .../input` → a new container starts, `mcp_token_hash` differs from before, `mcp.json` contains a different token, `init` has `resumed: true`, queued message is delivered (its `user_message` precedes the replayed turn); `POST .../end` → `done`, `git` sync event `ok: true`, `refs/sessions/<sid>` exists in the mirror, no container remains.
- [ ] Ephemeral scenario: an ephemeral profile on the stub with `message` → the recorded command uses `-p`, the session reaches `done` without any input accepted (`POST .../input` → 409), `cost_usd` and token counters are non-zero, container removed.
- [ ] Restart adoption: with a conversational session `running`, clear the registry and abort its owner (a `TestApp::simulate_restart()` helper that drops owners without touching containers), call `recover()`, assert the session is still `running`, `mcp_token_hash` unchanged, and a subsequent `input` is delivered and replayed with no duplicated events (`COUNT(*) = MAX(seq)`, distinct `_offset`s).
- [ ] The Orchestrator CI workflow's engine job runs this binary on both Podman and Docker after building the stub image; the E2E workflow is unaffected.

## Implementation notes
- Files: `orchestrator/tests/session_e2e.rs`, `orchestrator/tests/common/mod.rs` (real-engine spawn option, `simulate_restart`), `.github/workflows/orchestrator.yml` (engine job step).
- Poll with a helper `wait_for_state(app, sid, state, timeout)` reading `GET /sessions/{id}` every 250 ms; never sleep fixed durations.
- The stub's fixture transcript comes from the images epic; the assertions on event kinds must reference that fixture's expected sequence file rather than hard-coding counts.
- Use the API (HTTP through `axum-test`) rather than calling services directly so the scenario is the user's path.

## Edge cases
- The engine is Podman without `keep-id` support: the startup probe already fails `TestApp::spawn_with_engine`; report that error, do not mask it.
- Leftover containers from a previous failed run: the test removes any container labelled `mars.session_id` for its own project ids in a `Drop` guard.
- The host gateway name differs between Podman and Docker (`host.containers.internal` vs `host.docker.internal`): derive from the engine's `/version` and set `SESSION_EXTRA_HOSTS` accordingly; the MCP listener need not be reachable for these scenarios (the stub does not call MCP), but the `launch_warning` about `mars-orchestrator` must not fail the run.

## Testing
- This task is the test suite; it passes locally with `DOCKER_HOST` set against Podman and in CI on both engines: `cd orchestrator && DOCKER_HOST=... MARS_STUB_IMAGE=... cargo test --features integration-tests --test session_e2e`.
- `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes without `DOCKER_HOST` (the binary exits early).

## Documentation
- `README.md` "Development": one sentence naming `MARS_STUB_IMAGE` next to the existing session-image build command; `README.md` "CI": note that the engine job also runs the session end-to-end binary.

## Assumes from other epics
- "Session container images: claude and stub": the stub image, its fixture transcript and expected-event file.
- "Container engine adapter": the bollard engine, the startup probe and the engine test harness pattern in `tests/engine.rs`.
- "Repository scaffolding, tooling and CI": the orchestrator CI workflow with the Podman and Docker engine matrix.

## Correction: `running` on stdin attach, not on `init` (ADR 0032, task 3z8xu)
The pinned CLI writes nothing, `init` included, until its first stdin line (`ARCHITECTURE.md`, "Launch sequence"; `docs/decisions/0032-run-state-on-stdin-attach.md`). Where the text above disagrees, this section wins.
- A conversational session launched with no message is `running` with `cli_session_id` null. Assert the id is set only after `POST .../input` has produced the first `init`.
