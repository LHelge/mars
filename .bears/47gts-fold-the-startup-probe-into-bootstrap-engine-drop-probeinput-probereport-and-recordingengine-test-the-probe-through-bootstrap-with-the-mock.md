---
id: "47gts"
title: "Fold the startup probe into bootstrap_engine: drop ProbeInput, ProbeReport and RecordingEngine, test the probe through bootstrap with the mock"
status: open
priority: P2
created: "2026-09-17T20:02:43.362443951Z"
updated: "2026-09-17T20:02:43.362443951Z"
tags:
  - orchestrator
  - engine
  - architecture
depends_on:
  - "6m8br"
parent: quxdn
---

## Summary
`probe.rs` is a one-caller procedure with a module-sized interface: `ProbeInput` (six fields copied from `Config`), `ProbeReport` (discarded by its only caller; `file_uid` equals `own_uid` by construction), a private `check_probe_file` tested past the interface, and a 95-line `RecordingEngine` test double that delegates all 18 trait methods to observe call order. Inline the probe into `bootstrap_engine` as a private step that takes `&Config` and the engine, keep the behaviour and the fatal error text, and test it through `bootstrap_engine` with `MockEngine`'s own recorders.

## Documents
- `ARCHITECTURE.md` "Engine adapter" → "Startup probe" (what the probe proves on each engine; a failed probe is a fatal startup error with the reason in the log), "Restart procedure" (probe precedes adoption).
- `README.md` "Podman setup" (refusal to start).

## Acceptance criteria
- [ ] `engine/probe.rs` is removed or reduced to private functions of `engine/mod.rs`; `ProbeInput`, `ProbeReport` and `RecordingEngine` no longer exist; `PROBE_TIMEOUT` and `LABEL_PROBE` stay where the spec builder needs them.
- [ ] `bootstrap_engine(config, engine)` (or the existing signature) runs connect → networks → probe in the documented order; the `startup probe failed; refusing to start` log line and the `EngineError::Probe` message naming the uid pair and the fix per engine are unchanged (`tests/engine.rs` asserts the message on both engines already; keep that assertion).
- [ ] The probe's own checks (file exists, owned by our uid, writable, directory removed afterwards, timeout) are tested through `bootstrap_engine` with `MockEngine` scripting the container's behaviour, using the mock's `specs`/`spec_of` to assert the probe spec and label, and with a temporary `DATA_DIR` for the file checks.
- [ ] `main.rs` matches only `Err` from bootstrap, as it does today.

## Implementation notes
- Files: `orchestrator/src/engine/{mod,probe,spec}.rs`, `orchestrator/src/main.rs`, `orchestrator/tests/engine.rs`.
- The probe container's `HostConfig` must remain identical to a session's; keep using `build_probe_spec` from `spec.rs` so the "same HostConfig" claim in `ARCHITECTURE.md` stays true by construction.

## Edge cases
- The mock cannot write the probe file; the test writes it itself between `start` and `wait` (the mock exposes `exit`) or scripts a hook; choose the simplest and say so in the test.

## Testing
- `cd orchestrator && cargo fmt && cargo clippy --all-targets -- -D warnings && cargo clippy --all-targets --features integration-tests -- -D warnings && cargo test --features integration-tests` passes; live probe scenarios in `tests/engine.rs` still pass on both engines.

## Documentation
- None: `ARCHITECTURE.md` describes the probe's behaviour, not its module; verify the "Startup probe" paragraph still reads true and adjust wording only if it names `ProbeReport`.