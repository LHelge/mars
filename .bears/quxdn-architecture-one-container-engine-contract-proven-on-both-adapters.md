---
id: quxdn
title: "Architecture: one container engine contract, proven on both adapters"
type: epic
status: open
priority: P1
created: "2026-09-17T19:59:24.224452390Z"
updated: "2026-09-17T19:59:24.224452390Z"
tags:
  - orchestrator
  - engine
  - architecture
  - tests
depends_on:
  - naqhy
---

## Why

The architecture survey (2026-09-17) found the engine's normalised semantics stated in three places that disagree. `remove` of a missing container succeeds in `BollardEngine` (`bollard.rs`, `NotFound => Ok(())`, asserted live by `tests/engine.rs remove_missing_is_ok`), fails with `NotFound` in `MockEngine` (asserted by its own unit test) and the trait doc on `ContainerEngine::remove` says a third thing. `stop` of an exited container is `Ok` in bollard and `Conflict` in the mock. `MockStdin` accepts bytes after exit, so the Podman write-after-exit hazard that `streams.rs` exists to surface cannot be reproduced off-engine. Which `EngineError` variant an operation yields is decided per call site in `bollard.rs` by matching engine message text, and `Unsupported` carries three unrelated meanings. The startup probe has a module-sized interface (`ProbeInput`, `ProbeReport`, a 95-line `RecordingEngine` test double) for one caller that discards its report.

The session lifecycle epic (`s52qg`) will write the launcher and `SessionOwner` against `MockEngine`. If the mock's contract differs from bollard's, those tests prove the wrong thing.

## Scope

- The normalised semantics of every trait operation are written into `ARCHITECTURE.md` "Engine adapter" and the trait docs, and proven by one conformance suite that runs against `MockEngine` always and `BollardEngine` when `DOCKER_HOST` is set.
- `MockEngine` matches that contract, including stdin write-after-exit and exec exit codes.
- The probe folds into `bootstrap_engine`; its DTOs and test double go.
- Bespoke trait stubs (`PlaceholderEngine`, `UnreachableEngine`) are replaced by the mock where the mock can serve.

## Documents

`ARCHITECTURE.md` "Engine adapter" (operation table, "Startup probe"), "Orchestrator internals" (Errors); `CLAUDE.md` "Testing expectations" (engine tests); ADR 0004.

## Acceptance criteria

- [ ] Every operation's behaviour on a missing, exited or already-running container is documented once and asserted by the same test against both adapters.
- [ ] A write to the mock's stdin after the container exited fails the way bollard's does.
- [ ] The trait has exactly two implementations in `src/` plus one mock.

## Out of scope

Narrowing the trait's method set: revisit once the launcher shows what it consumes. The images (session images epic).