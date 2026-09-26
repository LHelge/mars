---
id: ca2va
title: "Container listing fails for a whole stop grace period on Podman: bollard cannot parse the `stopping` state"
status: done
priority: P1
created: "2026-09-26T21:27:20.733978195Z"
updated: "2026-09-26T21:58:57.865579186Z"
tags:
  - orchestrator
  - engine
  - bug
  - ci
attempts: 1
---

## Summary
Release run 36271964898 (commit 159b880) failed `engine (podman)`: `the_sweep_removes_a_dead_runs_containers_and_keeps_a_live_runs` (from 4q3t2) found the dead run's container still there. The cause is in the log: `could not list stale test containers: the container engine is unavailable: Failed to deserialize JSON: unknown variant 'stopping', expected one of '', 'created', 'running', 'paused', 'restarting', 'exited', 'removing', 'dead'`.

`BollardEngine::list_by_label` (`orchestrator/src/engine/bollard.rs` ~1031) deserialises the typed `ContainerSummary` from bollard 0.20, whose state enum has no `stopping`. It retries a `JsonDataError` 20 × 100 ms on the assumption that such a state is "over in milliseconds" (the comment cites Podman 4's `stopped`). On Podman 5/6 a container is `stopping` for its **whole stop grace period** — up to `STOP_GRACE_SECS` (default 20 s) in production, and 30 s in the engine contract suite since rapkz — so the listing fails whenever any container on the engine is being stopped.

This is a production bug, not only a test one: `list_by_label` is what startup recovery (`session/recovery.rs:120`, where a failed listing is fatal), the launcher (`session/launcher.rs:942`) and orphan cleanup (`cron/orphan_cleanup.rs:114`) use.

## Documents
- `ARCHITECTURE.md`, "Engine adapter" (normalised semantics; the list row), "Restart procedure".

## Acceptance criteria
- [ ] A listing succeeds whatever state names the engine reports for any container, including Podman's `stopping` held for a full grace period. `ContainerSummary` needs only `running` (plus id, name, labels, created), so the state must not be able to fail the listing: e.g. request the list without the typed state enum (a raw/untyped response, or a bollard option that tolerates unknown variants), and map `running` from the raw state string. The retry loop and its "milliseconds" rationale go if they are no longer needed.
- [ ] The engine contract suite (`tests/common/engine_contract.rs`, both halves) gains a scenario that lists while a container is in its stop grace period (a container whose command ignores TERM, stopped with a grace of several seconds, listed during it) and asserts the listing succeeds and reports the stopping container as not running (or as running — decide, and document which in "Engine adapter"). MockEngine follows.
- [ ] `cleanup_stale_test_containers` in `tests/common/engine.rs` no longer swallows a failed listing with `eprintln!`: a sweep that cannot list is a test failure, so this class of bug cannot again show up as "the container survived".
- [ ] The failing CI scenario passes; run `cargo test --features integration-tests --test engine --test session_e2e` repeatedly with concurrent stops on Podman.
- [ ] `ARCHITECTURE.md` "Engine adapter" states that a listing tolerates any engine state name.

Discovered from: 4q3t2 (CI of the push of the standalone batch).