---
id: au4vs
title: Add AppState and tracing initialisation with env-filter to the prelude
status: in_progress
priority: P1
created: "2026-09-16T20:28:15.561168698Z"
updated: "2026-09-17T05:28:47.905549642Z"
tags:
  - orchestrator
  - core
depends_on:
  - "3hvy9"
  - vyfma
parent: sywed
assignee: claude-opus-subagent
attempts: 1
---

## Summary
Add the two remaining prelude pieces every handler and background task depends on: `AppState`, the cloneable struct axum hands to every handler, and `init_tracing(&Config)`, the single place the `tracing` subscriber is installed with an `env-filter` driven by `RUST_LOG`. `AppState` starts with the fields this epic can populate (`Arc<Config>`, `PgPool`) and documents the extension points the trait-owning epics fill in, so their additions are mechanical.

## Documents
- `ARCHITECTURE.md` "Orchestrator internals": "`AppState` is cloned into every handler and holds: `Arc<Config>`, the `PgPool`, `Arc<dyn ContainerEngine>`, `Arc<dyn EmailClient>`, `Arc<dyn GitCredentialProvider>`, the `SecretsKeyring`, the `SessionRegistry` (handles to running session owner tasks), and the broadcast senders for event fan-out. Every `Arc<dyn Trait>` has a mock behind the `integration-tests` feature".
- `ARCHITECTURE.md` "Orchestrator internals", Crates: `tracing`, `tracing-subscriber` (`env-filter`).
- `CLAUDE.md` "Backend conventions": "Logging is `tracing` with structured fields (`session_id = %id`), never string-formatted ids. Never log event payloads at `info` or above." Rule 3 on secrets in logs.
- `README.md` "Configuration": `RUST_LOG` log filter, `info` by default.

## Acceptance criteria
- [ ] `orchestrator/src/prelude/state.rs` defines `#[derive(Clone)] pub struct AppState { pub config: Arc<Config>, pub pool: PgPool }` with `impl AppState { pub fn new(config: Arc<Config>, pool: PgPool) -> Self }` and a doc comment listing, verbatim from the architecture sentence, the fields other epics add and which epic owns each (`engine: Arc<dyn ContainerEngine>` Container engine; `email: Arc<dyn EmailClient>` Authentication; `git_credentials: Arc<dyn GitCredentialProvider>` Git operations; `keyring: SecretsKeyring` Secrets manager; `registry: SessionRegistry` Session lifecycle; broadcast senders Real-time delivery).
- [ ] `axum::extract::FromRef<AppState> for Arc<Config>` and `for PgPool` are implemented so handlers can extract `State<PgPool>` directly.
- [ ] `orchestrator/src/prelude/telemetry.rs` exposes `pub fn init_tracing(filter: &str) -> std::result::Result<(), TelemetryError>` building `tracing_subscriber::registry()` with `EnvFilter::try_new(filter)` (falling back to `EnvFilter::new("info")` with a `warn!` if the string is invalid) and a `fmt` layer that writes to stderr, includes target and level, uses compact single-line output with RFC 3339 UTC timestamps, and never `.pretty()`; a second call returns `TelemetryError::AlreadyInitialised` rather than panicking (tests call it repeatedly).
- [ ] `init_tracing` honours an `RUST_LOG` environment override above `Config::rust_log` only through `Config` (the config task already reads `RUST_LOG`); the function takes the string and does not read the environment itself.
- [ ] `prelude/mod.rs` re-exports `AppState`, `init_tracing`, and the tracing macros (`tracing::{debug, error, info, instrument, warn}`) so modules use them through the prelude; `Arc` and `PgPool` are also re-exported (`pub use std::sync::Arc; pub use sqlx::PgPool;`) because nearly every module needs them.
- [ ] `cd orchestrator && cargo fmt && cargo clippy --all-targets --features integration-tests -- -D warnings && cargo test --features integration-tests` pass.

## Implementation notes
- Files: `orchestrator/src/prelude/state.rs`, `orchestrator/src/prelude/telemetry.rs`, `orchestrator/src/prelude/mod.rs`.
- Structured-field discipline is enforced by convention, not the compiler; add a short module doc in `telemetry.rs` quoting the `CLAUDE.md` rule and the two examples (`session_id = %id`, never `format!`), and note that event payloads and secret values must never appear at `info` or above.
- The `tower-http` `TraceLayer` is attached to the router in the startup task, not here.
- Do not introduce a global `OnceLock<AppState>`; state is passed explicitly.

## Edge cases
- Invalid `RUST_LOG` (for example `RUST_LOG=verbose`) must not abort startup: fall back to `info` and warn with the offending string (the string is not a secret).
- Calling `init_tracing` inside integration tests when another test already installed a subscriber must not panic: use `try_init()` and map the error.

## Testing
- Unit tests: `AppState` is `Clone + Send + Sync + 'static` (a `fn assert_send_sync<T: Send + Sync + 'static>()` compile-time check); `init_tracing("info")` then `init_tracing("debug")` returns `AlreadyInitialised` the second time; `init_tracing("not a filter")` returns `Ok` (fallback path).
- Command: `cd orchestrator && cargo fmt && cargo clippy --all-targets --features integration-tests -- -D warnings && cargo test --features integration-tests`.

## Documentation
- none: implements `ARCHITECTURE.md` "Orchestrator internals" as written.

## Assumes from other epics
- Container engine, Authentication (email), Git operations, Secrets manager, Session lifecycle and Real-time delivery epics each add exactly one field to `AppState` and its `FromRef` impl, and the Database epic's `TestApp::spawn()` constructs `AppState` with the mocks.