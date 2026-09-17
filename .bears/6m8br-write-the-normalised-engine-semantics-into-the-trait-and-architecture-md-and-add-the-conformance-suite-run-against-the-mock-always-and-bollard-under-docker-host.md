---
id: "6m8br"
title: Write the normalised engine semantics into the trait and ARCHITECTURE.md and add the conformance suite run against the mock always and bollard under DOCKER_HOST
status: open
priority: P1
created: "2026-09-17T20:00:39.780107523Z"
updated: "2026-09-17T20:05:17.201360405Z"
tags:
  - orchestrator
  - engine
  - architecture
  - tests
  - docs
depends_on:
  - "48ke5"
  - trxaf
parent: quxdn
---

## Summary
Make the normalisations that `BollardEngine` performs part of the `ContainerEngine` interface instead of per-call-site knowledge in `bollard.rs`, and prove them with one suite of contract tests that any adapter must pass. The suite is a function over `Arc<dyn ContainerEngine>`; `tests/engine.rs` runs it against `BollardEngine` when `DOCKER_HOST` is set and a new off-engine test runs it against `MockEngine` on every `cargo test`. Where the two adapters disagree today, the documented rule wins and the mock is fixed in the next task.

## Documents
- `ARCHITECTURE.md` "Engine adapter" (operation table; attach and exec notes), "Orchestrator internals" → Errors (`EngineError`).
- `CLAUDE.md` "Testing expectations" (engine tests run only with `DOCKER_HOST`).
- ADR 0004.

## Acceptance criteria
- [ ] `ARCHITECTURE.md` "Engine adapter" gains a "Normalised semantics" list, one line per operation, stating the outcome for a missing container, an exited container and an already-running container, and for `attach_stdin` a write after exit. Decide and record at least: `remove` of a missing container is `Ok`; `stop` of an exited container is `Ok`; `kill` and `inspect` of a missing container are `NotFound`; `start` of a running container is `Ok` (idempotent) or `Conflict`, choose one; `exec_pty` on a container that is not running is `Conflict` on both engines (already documented); a stdin write after the container exited returns an error, never a silent success; `wait` on a missing container is `NotFound`.
- [ ] The doc comment of each `ContainerEngine` method in `engine/mod.rs` states the same rule in the same words, and the trait doc says the conformance suite is the definition.
- [ ] `EngineError::Unsupported` no longer carries the reserved-environment-name collision from `spec.rs`; that becomes a distinct variant (`InvalidSpec(String)` or similar, mapped to 400 or 500 as `ARCHITECTURE.md` Errors decides) so a launcher can tell a configuration error from a missing capability.
- [ ] `orchestrator/tests/common/engine_contract.rs` (or `engine/contract.rs` behind `integration-tests`) exposes `pub async fn assert_engine_contract(engine: Arc<dyn ContainerEngine>, image: &str, ...)` covering every line of the "Normalised semantics" list plus create → connect → start → wait ordering, label listing, and signal delivery where the adapter can observe it.
- [ ] `tests/engine.rs` calls the suite against `BollardEngine` and keeps only the scenarios that are genuinely engine-specific (nested binds, `keep-id`, pull, host gateway). Its imports of `spec::{LABEL_PROBE, order_binds, to_bollard}` and the raw `Docker` client in `tests/common/engine.rs` go unless a scenario cannot be asserted through the trait; if one remains, its comment says why.
- [ ] A new `tests/engine_mock.rs` (or a module in `engine/mock.rs`) runs the suite against `MockEngine` with no `DOCKER_HOST`; it may fail on the mock until the next task lands, so the two tasks merge together or the mock test is `#[ignore]`d with the task id in the reason until then.

## Implementation notes
- Files: `orchestrator/src/engine/mod.rs`, `orchestrator/src/engine/error.rs`, `orchestrator/src/engine/spec.rs`, `orchestrator/src/engine/bollard.rs` (only where a per-call-site normalisation needs to change to match the documented rule), `orchestrator/tests/engine.rs`, `orchestrator/tests/common/engine.rs`, `ARCHITECTURE.md`, `CLAUDE.md`.
- The suite needs a container that exits on its own and one that stays up; the stub image from the images epic is not required, `alpine`-style `sleep`/`true` commands against the configured session image suffice, as `tests/engine.rs` does today.
- `says_already` and `says_not_running` in `bollard.rs` stay private; the suite asserts outcomes, not message text.

## Edge cases
- Podman returns 500 where Docker returns 409 for exec on a stopped container; the suite asserts `Conflict` and the adapter keeps both arms.
- A rootless Podman accepts writes to an exited container's stdin; the adapter's drained-output detection is what turns that into an error, so the suite must wait for exit before writing.

## Testing
- `cargo test --features integration-tests` runs the mock half; `DOCKER_HOST=... cargo test --features integration-tests --test engine` runs the bollard half on Podman and Docker (the Engine CI workflow from `48ke5`).
- `cd orchestrator && cargo fmt && cargo clippy --all-targets -- -D warnings && cargo clippy --all-targets --features integration-tests -- -D warnings && cargo test --features integration-tests` passes.

## Documentation
- `ARCHITECTURE.md` "Engine adapter": the "Normalised semantics" list and one sentence under the table pointing at the conformance suite as the executable definition.
- `ARCHITECTURE.md` "Orchestrator internals" → Errors: the new `EngineError` variant if the mapping table there lists variants.
- `CLAUDE.md` "Testing expectations": engine contract tests run against the mock always and against bollard with `DOCKER_HOST`.

## Assumes from other epics
- "Container engine adapter": `tests/engine.rs`, `MockEngine`, `BollardEngine` as merged; the Engine CI workflow (`48ke5`).