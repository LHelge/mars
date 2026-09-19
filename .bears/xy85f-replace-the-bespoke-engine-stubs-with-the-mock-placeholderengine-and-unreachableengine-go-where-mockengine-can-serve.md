---
id: xy85f
title: "Replace the bespoke engine stubs with the mock: PlaceholderEngine and UnreachableEngine go where MockEngine can serve"
status: done
priority: P3
created: "2026-09-17T20:04:27.851779858Z"
updated: "2026-09-19T19:33:32.657175654Z"
tags:
  - orchestrator
  - engine
  - architecture
  - tests
depends_on:
  - n7tzv
parent: quxdn
attempts: 1
---

## Summary
The 18-method trait forces every stub to implement everything. `routes/health.rs` carries a 90-line `UnreachableEngine` to test one `false`, and `PlaceholderEngine` in `engine/mod.rs` is used by `prelude/state.rs` tests and `tests/shutdown.rs`. With the mock aligned to the contract and `set_unhealthy` already present, these stubs are pass-throughs; remove them where the mock can take their place and keep `PlaceholderEngine` only if something outside the `integration-tests` feature genuinely needs an engine that refuses everything.

## Documents
- `ARCHITECTURE.md` "Orchestrator internals" (`AppState` holds `Arc<dyn ContainerEngine>`; every `Arc<dyn Trait>` has a mock behind `integration-tests`).

## Acceptance criteria
- [ ] `UnreachableEngine` in `routes/health.rs` is deleted; the health unit tests use `MockEngine::set_unhealthy(true)` (gated `#[cfg(all(test, feature = "integration-tests"))]`) or move to `tests/health.rs`, which already does this through `TestApp`.
- [ ] `PlaceholderEngine` is deleted if `prelude/state.rs` tests and `tests/shutdown.rs` can build `AppState` with `MockEngine`; if a non-feature build needs it (for example a `cargo test` without the feature), it stays and its doc says exactly which caller requires it.
- [ ] `ContainerEngine::as_any` stays (tests downcast through it); no trait method is added or removed here.

## Implementation notes
- Files: `orchestrator/src/routes/health.rs`, `orchestrator/src/engine/mod.rs`, `orchestrator/src/prelude/state.rs`, `orchestrator/tests/shutdown.rs`.
- Trait narrowing is out of scope (epic "Out of scope"); note anything the launcher turns out not to need in a follow-up task on the session epic.

## Testing
- `cargo clippy --all-targets -- -D warnings` (no feature) and with `--features integration-tests` both stay clean, which is the constraint that decides whether `PlaceholderEngine` survives.
- `cd orchestrator && cargo fmt && cargo clippy --all-targets -- -D warnings && cargo clippy --all-targets --features integration-tests -- -D warnings && cargo test --features integration-tests` passes.

## Documentation
- None.