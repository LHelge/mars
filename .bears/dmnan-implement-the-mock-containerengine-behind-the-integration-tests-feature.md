---
id: dmnan
title: Implement the mock ContainerEngine behind the integration-tests feature
status: done
priority: P1
created: "2026-09-16T20:28:45.110649935Z"
updated: "2026-09-17T17:14:47.363719572Z"
tags:
  - orchestrator
  - engine
  - tests
depends_on:
  - kjvte
parent: naqhy
assignee: claude-opus-subagent
attempts: 1
---

## Summary
Provide `MockEngine` in `orchestrator/src/engine/mock.rs`, compiled only with `--features integration-tests`, so that every integration test (`TestApp::spawn()`) and every session, recovery, cron and WebSocket test can run without a container engine. The mock records each `ContainerSpec` it was asked to create, tracks per-container state transitions, captures bytes written to attached stdin, lets a test simulate container exit with a code, serves a scripted exec session for terminal tests, and is downcastable through `as_any()`.

## Documents
- `ARCHITECTURE.md` "Orchestrator internals" ("Every `Arc<dyn Trait>` has a mock behind the `integration-tests` feature so the whole API can be tested without an engine")
- `CLAUDE.md` "Backend conventions" (mock with `as_any()` for downcasting), "Testing expectations" (`TestApp::spawn()` uses the mock engine)
- `ARCHITECTURE.md` "Restart procedure" (recovery lists by label; a gone container → `parked`), "Stop semantics" (SIGINT then SIGTERM recorded)

## Acceptance criteria
- [ ] `MockEngine::new(kind: EngineKind)` implements `ContainerEngine`; `kind()` returns the configured kind (default `Podman`) so tests can check the userns rule through the spec builder.
- [ ] `create` records the spec, assigns id `mock-<n>` (monotonic), state `Created`; returns `Conflict` if a container with the same name exists and is not removed.
- [ ] `start` moves `Created → Running`; `start` on a non-existent id → `NotFound`; `kill` and `stop` on a non-running container → `Conflict`; `remove` deletes the record (`force = false` on a running container → `Conflict`, `force = true` succeeds).
- [ ] `kill(signal)` appends the signal to `signals(id) -> Vec<Signal>`; it does not exit the container by itself. `MockEngine::exit(id, code)` transitions to `Exited { code }` and wakes every pending `wait` with `ExitStatus { code, oom_killed: false }`; `MockEngine::vanish(id)` drops the record so `inspect`/`wait` return `NotFound` (recovery's "container gone" path).
- [ ] `attach_stdin` returns a writer whose bytes are appended to `stdin_bytes(id) -> Vec<u8>`; `stdin_lines(id) -> Vec<String>` splits on `\n` for test assertions on stream-json input.
- [ ] `exec_pty` records `ExecRequest { container, cmd, user, cols, rows }` and returns a `MockExecSession` that echoes writes back as reads, records `resize` calls and returns exit code 0 on `close`; `script_exec_output(id, bytes)` lets a test queue output.
- [ ] `list_by_label(key)` returns every recorded container that has the label key and is not removed; `ensure_network`, `connect_network`, `pull_image` record their calls (`networks()`, `connections(id)`, `pulled_images()`) and succeed; `image_exists` returns `false` for images in `missing_images` (settable) and `true` otherwise; `fail_next_pull(message)` makes the next `pull_image` return `ImagePull` with that message.
- [ ] `ping` returns `Ok` unless `set_unhealthy(true)`; `as_any()` returns `self`.
- [ ] `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` pass; the crate also builds without the feature (the module is `#[cfg(feature = "integration-tests")]`).

## Implementation notes
- File: `orchestrator/src/engine/mock.rs`; gate in `engine/mod.rs` with `#[cfg(feature = "integration-tests")] pub mod mock;`.
- State: `Arc<Mutex<MockState>>` with `containers: BTreeMap<String, MockContainer { spec: ContainerSpec, state: ContainerState, signals: Vec<Signal>, stdin: Vec<u8>, networks: Vec<String>, exit_waiters: Vec<oneshot::Sender<ExitStatus>> }>`, `next_id`, `pulled`, `missing_images`, `next_pull_error`, `exec_requests`, `unhealthy`. Use `std::sync::Mutex` (never held across an await) and `tokio::sync::oneshot`/`Notify` for `wait`.
- Stdin writer: a small struct implementing `AsyncWrite` that pushes into the shared `Vec<u8>` under the mutex; `StdinWriter` blanket impl covers it.
- Inspection helpers for tests: `specs() -> Vec<ContainerSpec>`, `spec_of(id) -> Option<ContainerSpec>`, `state_of(id)`, `signals(id)`, `stdin_bytes(id)`, `connections(id)`, `networks()`, `pulled_images()`, `exec_requests()`. `container_id_for_session(session_id) -> Option<ContainerId>` looks up by the `mars.session_id` label — the sessions epic's tests will use it.
- `TestApp::spawn()` (Database epic) constructs `Arc<MockEngine>` and stores it in `AppState.engine`; tests downcast with `app.state.engine.as_any().downcast_ref::<MockEngine>()`. Add a `TestApp::engine(&self) -> &MockEngine` helper there if `tests/common/mod.rs` exists when this task is implemented; otherwise leave a note in the report.
- The mock does not touch the filesystem: the probe function must be skipped in `TestApp` (startup wiring task) rather than simulated here.

## Edge cases
- `wait` on an already `Exited` container resolves immediately; `wait` on a `Running` one parks until `exit`/`vanish`; `vanish` resolves waiters with `NotFound`? No: `vanish` drops waiters so `wait` returns `EngineError::NotFound` — implement by sending an `Err` through the oneshot (`oneshot::Sender<Result<ExitStatus, EngineError>>`).
- `create` after `remove` of the same name must succeed (relaunch reuses `mars-session-<sid>`).
- Multiple `attach_stdin` calls on one container append to the same buffer (recovery reattaches).
- Locking: never hold the mutex while awaiting a oneshot.

## Testing
- Unit tests in `mock.rs`: create/start/kill/exit/wait sequence; `wait` blocks then resolves on `exit`; `vanish` → `NotFound`; stdin capture and `stdin_lines`; name conflict; `list_by_label` filtering; `fail_next_pull`; exec echo and resize recording.
- `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` and a plain `cargo build` (feature off) must pass.

## Documentation
- None: implements the documented contract as written.

## Assumes from other epics
- "Database schema, models, repositories and test harness": `tests/common/mod.rs` with `TestApp::spawn()` that wires `Arc<dyn ContainerEngine>`; if it predates this task with a stub engine, replace the stub with `MockEngine` here.
