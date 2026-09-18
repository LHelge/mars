---
id: n7tzv
title: "Align MockEngine with the engine contract: remove, stop, start and wait semantics, stdin write-after-exit fails, exec exit codes; delete the mock tests that assert the opposite"
status: in_progress
priority: P1
created: "2026-09-17T20:02:26.338561620Z"
updated: "2026-09-18T21:08:41.324778945Z"
tags:
  - orchestrator
  - engine
  - architecture
  - tests
depends_on:
  - "6m8br"
parent: quxdn
attempts: 1
---

## Summary
Make `MockEngine` an adapter of the documented contract rather than a state machine with its own opinions. Today `remove` of a missing container returns `NotFound` (bollard: `Ok`), `stop` of an exited container returns `Conflict` (bollard: `Ok`), `start` of a running container invents `Conflict`, and `MockStdin` accepts every write. After this task the conformance suite from the previous task passes against the mock with no `#[ignore]`.

## Documents
- `ARCHITECTURE.md` "Engine adapter" → "Normalised semantics" (from the previous task), attach note (write after exit fails rather than being silently lost).
- ADR 0004.

## Acceptance criteria
- [ ] The conformance suite passes against `MockEngine` on plain `cargo test --features integration-tests`.
- [ ] `MockStdin::write` returns an `EngineError` once the container has exited (via `MockEngine::exit` or `vanish`), matching the error class `BollardStdin` returns; bytes written before exit are still recorded for `stdin_bytes`/`stdin_lines`.
- [ ] `MockExecSession` reports the scripted exit code through the same path `BollardExec` does, including the exec-on-not-running `Conflict`.
- [ ] Mock unit tests that asserted the old behaviour (`the_state_machine_refuses_what_the_engine_would` and siblings) are deleted or rewritten to assert the contract; the mock's inspector methods (`specs`, `spec_of`, `signals`, `connections`, `exit`, `vanish`, `script_exec_output`, `set_unhealthy`, `fail_next_pull`, `set_missing_images`) are unchanged.
- [ ] The mock keeps refusing what the contract says is refused (for example `remove` without `force` of a running container is `Conflict` on both engines, if the previous task recorded it that way).

## Implementation notes
- Files: `orchestrator/src/engine/mock.rs`, `orchestrator/tests/engine_mock.rs` (from the previous task).
- The mock's job is to let `SessionOwner` and launcher tests observe calls and drive exits; every normalisation it performs must be one the contract names. If a scenario is not in the contract, add it to the contract first (previous task's list in `ARCHITECTURE.md`) rather than to the mock alone.

## Edge cases
- `vanish` (the container disappears under the orchestrator, as on a restart) must make `inspect` and `wait` return `NotFound` and `remove` return `Ok`, exactly as a real engine would after `podman rm` from outside.

## Testing
- `cd orchestrator && cargo fmt && cargo clippy --all-targets -- -D warnings && cargo clippy --all-targets --features integration-tests -- -D warnings && cargo test --features integration-tests` passes; the Engine CI workflow stays green.

## Documentation
- None beyond the previous task; if the mock needs a rule the list lacks, add it there in the same commit.

## Assumes from other epics
- "Session lifecycle": `qtx4x`, `bt9q6` and `9wxhs` are written against the aligned mock (they depend on this task).