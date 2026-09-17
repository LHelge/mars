---
id: "84dxt"
title: Define the ContainerEngine trait, EngineError and engine domain types
status: done
priority: P0
created: "2026-09-16T20:26:29.486214325Z"
updated: "2026-09-17T15:46:58.452054171Z"
tags:
  - orchestrator
  - engine
  - core
depends_on:
  - sywed
parent: naqhy
assignee: claude-opus-subagent
attempts: 1
---

## Summary
Define the engine-neutral contract every other engine task implements: the `ContainerEngine` trait in `orchestrator/src/engine/mod.rs`, the `EngineError` enum wired into the crate `Error`, and the plain data types (`ContainerSpec`, `Bind`, `ContainerId`, `EngineKind`, `Signal`, `ExitStatus`, `ContainerInfo`, `ContainerSummary`, `StdinWriter`, `ExecSession`) that the bollard implementation, the mock, the launcher (Session lifecycle epic) and the WebSocket terminal (Real-time delivery epic) share. No bollard code is written here; the trait is the boundary that keeps `HostConfig` knowledge inside `engine/`.

## Documents
- `ARCHITECTURE.md` "Engine adapter" (operation table: create / start / stop / kill / remove, attach stdin TTY off, exec + resize TTY on, list with label filter, bind mounts, `NetworkMode`, connect second network before start, image pull, `ExtraHosts`, `Runtime`, `UsernsMode`)
- `ARCHITECTURE.md` "Orchestrator internals" (`engine/` module, `AppState` holds `Arc<dyn ContainerEngine>`, `Error` enum has `#[from] EngineError`, every `Arc<dyn Trait>` has a mock with `as_any()`)
- `ARCHITECTURE.md` "Session container specification" (the fields a spec must be able to carry)
- `ARCHITECTURE.md` "Stop semantics" (`kill` with `SIGINT` then `SIGTERM`)
- `SPEC.md` "WebSocket: session stream" (terminal is an exec with a PTY running `/bin/bash -l` as `agent`, `terminal_resize`, `terminal_closed` with `exit_code`)
- ADR 0004

## Acceptance criteria
- [ ] `orchestrator/src/engine/mod.rs` exports `pub trait ContainerEngine: Send + Sync` that is object-safe (`Arc<dyn ContainerEngine>` compiles) and has exactly the operations listed under Implementation notes, plus `fn kind(&self) -> EngineKind` and `fn as_any(&self) -> &dyn Any`.
- [ ] `EngineError` is a `thiserror` enum in `orchestrator/src/engine/error.rs` with the variants listed below; `crate::prelude::Error` gains `#[from] EngineError` and maps it to HTTP 500 with the generic internal message, logging the real error with `tracing::error!`.
- [ ] `ContainerSpec` and `Bind` are engine-neutral structs (no `bollard` types in their fields) with `Debug + Clone + PartialEq` so the mock can record and tests can assert on them.
- [ ] `Signal` has `Sigint`, `Sigterm`, `Sigkill` and `Display` yields `SIGINT`, `SIGTERM`, `SIGKILL` (the strings the engine `kill` endpoint accepts and the `state_change` event `signal` field uses).
- [ ] `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` pass.

## Implementation notes
- Files: `orchestrator/src/engine/mod.rs` (trait, re-exports), `orchestrator/src/engine/types.rs`, `orchestrator/src/engine/error.rs`; `orchestrator/src/prelude/error.rs` (add the `#[from]` variant). Declare `pub mod bollard; pub mod spec; pub mod probe;` and `#[cfg(feature = "integration-tests")] pub mod mock;` as the later tasks fill them (empty files are fine).
- Object safety: native `async fn` in traits is not dyn-compatible. Use the `async-trait` crate (`cargo add async-trait` in `orchestrator/`) and add the row `Async traits | async-trait` to the crate table in `ARCHITECTURE.md` "Orchestrator internals" in the same commit, unless the scaffolding epic already added it.
- Trait shape (names binding for the other tasks):
  ```rust
  #[async_trait]
  pub trait ContainerEngine: Send + Sync {
      fn kind(&self) -> EngineKind;                                             // detected once at connect
      async fn ping(&self) -> Result<(), EngineError>;                          // GET /api/health engine flag
      async fn ensure_network(&self, name: &str, internal: bool) -> Result<(), EngineError>;
      async fn image_exists(&self, image: &str) -> Result<bool, EngineError>;
      async fn pull_image(&self, image: &str) -> Result<(), EngineError>;
      async fn create(&self, spec: &ContainerSpec) -> Result<ContainerId, EngineError>;
      async fn connect_network(&self, id: &ContainerId, network: &str) -> Result<(), EngineError>;
      async fn start(&self, id: &ContainerId) -> Result<(), EngineError>;
      async fn stop(&self, id: &ContainerId, grace_secs: u32) -> Result<(), EngineError>;
      async fn kill(&self, id: &ContainerId, signal: Signal) -> Result<(), EngineError>;
      async fn remove(&self, id: &ContainerId, force: bool) -> Result<(), EngineError>;
      async fn inspect(&self, id: &ContainerId) -> Result<ContainerInfo, EngineError>;
      async fn wait(&self, id: &ContainerId) -> Result<ExitStatus, EngineError>;
      async fn list_by_label(&self, label_key: &str) -> Result<Vec<ContainerSummary>, EngineError>;
      async fn attach_stdin(&self, id: &ContainerId) -> Result<Box<dyn StdinWriter>, EngineError>;
      async fn exec_pty(&self, id: &ContainerId, cmd: &[String], user: &str, cols: u16, rows: u16) -> Result<Box<dyn ExecSession>, EngineError>;
      fn as_any(&self) -> &dyn Any;
  }
  ```
- Types in `types.rs`:
  - `pub struct ContainerId(pub String)` with `Display`.
  - `pub enum EngineKind { Podman, Docker }`.
  - `pub struct Bind { pub host_source: PathBuf, pub container_target: String, pub read_only: bool }`.
  - `pub struct ContainerSpec { pub image: String, pub name: String, pub labels: BTreeMap<String, String>, pub user: String, pub working_dir: String, pub cmd: Vec<String>, pub env: Vec<(String, String)>, pub binds: Vec<Bind>, pub network: String, pub extra_hosts: Vec<String>, pub runtime: Option<String>, pub open_stdin: bool }`. `env` is an ordered `Vec`, not a map, because the table fixes the order (fixed variables first, secrets after). `network` is the `NetworkMode` at creation; the egress network is connected separately by the caller through `connect_network` before `start`.
  - `pub struct ExitStatus { pub code: i64, pub oom_killed: bool }`.
  - `pub enum ContainerState { Created, Running, Paused, Exited { code: i64 }, Removing, Dead, Unknown(String) }`.
  - `pub struct ContainerInfo { pub id: ContainerId, pub name: String, pub labels: BTreeMap<String, String>, pub state: ContainerState, pub networks: Vec<String>, pub pid: Option<i64> }`.
  - `pub struct ContainerSummary { pub id: ContainerId, pub name: String, pub labels: BTreeMap<String, String>, pub running: bool }`.
  - `pub trait StdinWriter: tokio::io::AsyncWrite + Send + Unpin {}` with a blanket impl.
  - `#[async_trait] pub trait ExecSession: Send { async fn read(&mut self) -> Result<Option<bytes::Bytes>, EngineError>; async fn write(&mut self, data: &[u8]) -> Result<(), EngineError>; async fn resize(&mut self, cols: u16, rows: u16) -> Result<(), EngineError>; async fn close(self: Box<Self>) -> Result<i64, EngineError>; }` (`close` drops stdin and returns the exit code for `terminal_closed`).
- `EngineError` variants: `Connection(String)`, `NotFound(String)`, `Conflict(String)`, `ImagePull { image: String, message: String }`, `Api { status: u16, message: String }`, `Io(#[from] std::io::Error)`, `Unsupported(String)`, `Probe(String)`. Messages must never contain environment values (secrets travel in `ContainerSpec.env`; never `Debug`-print a spec into an error or log at `info`).
- Label and name constants live in `types.rs`: `pub const LABEL_SESSION_ID: &str = "mars.session_id"; pub const LABEL_PROJECT_ID: &str = "mars.project_id"; pub const LABEL_PROFILE_ID: &str = "mars.profile_id"; pub fn session_container_name(sid: Uuid) -> String` returning `mars-session-<sid>`.

## Edge cases
- `ContainerSpec` must not implement `Display`/`Debug` output that prints `env` values: implement `Debug` by hand (or a `redacted()` helper) that prints env keys only, so a debug log of a spec cannot leak a secret (CLAUDE.md rule 3).
- `EngineError::NotFound` is what callers use to distinguish "container gone" (recovery marks the session `parked`) from other failures; document this on the variant.
- `Signal::Display` must produce the exact upper-case names; the `state_change` event schema uses `"SIGINT" | "SIGTERM"`.

## Testing
- Unit tests in `types.rs`: `Signal` display strings; `session_container_name`; `ContainerSpec` debug output contains env keys but not values.
- A compile-time assertion that the trait is object-safe: `fn _assert(_: Arc<dyn ContainerEngine>) {}` in a `#[cfg(test)]` module.
- `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` must pass.

## Documentation
- `ARCHITECTURE.md` "Orchestrator internals" crate table gains `async-trait` (and `bytes` if added explicitly) in the same commit; otherwise none: implements the documented contract as written.

## Assumes from other epics
- "Repository scaffolding, tooling and CI": the crate, `prelude/` with the `Error` enum, `Result<T>`, `AppState` shape and the empty `engine/` module exist.
