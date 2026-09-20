---
id: "9pvaj"
title: Add StopReason (user, idle, stalled) to the owner stop path so idle conversational sessions park and idle ephemeral sessions fail as stalled
status: done
priority: P1
created: "2026-09-16T20:42:55.195112567Z"
updated: "2026-09-20T05:02:31.082979485Z"
tags:
  - orchestrator
  - sessions
depends_on:
  - xjaah
parent: cxmar
attempts: 1
---

## Summary
Extend the `SessionOwner` stop path with a reason so the idle reaper can reuse the owner's single-writer SIGINT-then-SIGTERM sequence instead of transitioning sessions behind the owner's back. A stop with reason `Idle` ends in `parked` with `state_change.reason = "idle timeout"`; a stop with reason `Stalled` (ephemeral sessions only) ends in `failed` with `sessions.error = "stalled"` and fires the `on_session_ended` hook so held leases are released. The user-initiated stop keeps its existing behaviour and wording.

## Documents
- `ARCHITECTURE.md` "Session owner task" step 3 and step 4 ("the idle reaper (a cron job, not the owner) parks conversational sessions idle longer than the profile's `idle_timeout_secs` and fails ephemeral ones as stalled")
- `ARCHITECTURE.md` "Session lifecycle" (state table: `failed` with `sessions.error` `stalled` "for an ephemeral session the idle reaper gave up on"; `running → parked: conversational: idle reaper / stop / ...`; `running → failed: ... ephemeral: stalled`; ephemeral sessions are never parked, resumed or retried; "Held tasks are released" for `done` and `failed`)
- `ARCHITECTURE.md` "Stop semantics" (`SIGINT`, `STOP_GRACE_SECS`, `SIGTERM`, signal recorded in the `state_change` event)
- `ARCHITECTURE.md` "Task tracker" -> "Liveness comes from the session" ("for an ephemeral session the same silence means stalled, and the reaper stops the container and marks the session `failed` with error `stalled`")
- `SPEC.md` "AgentEvent" (`state_change { from, to, reason, signal? }`), "Sessions" (`Session.error`)
- `docs/data-model.md` `sessions` (`error`: reason for `failed`; `parked_at`, `ended_at`, `container_id` NULL once removed)

## Acceptance criteria
- [ ] `orchestrator/src/session/owner.rs` gains `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum StopReason { User, Idle, Stalled }`; `OwnerCommand::Stop` carries `reason: StopReason`; `SessionRegistry::stop(id, reason)` forwards it and returns `StopOutcome::Sent | StopOutcome::AlreadyStopping { since: Instant } | StopOutcome::NoOwner`; the registry records `stopping_since` when the first stop is accepted and clears it when the owner leaves `running`.
- [ ] Signal sequence is unchanged for every reason: `engine.kill(cid, "SIGINT")`, then `SIGTERM` after `STOP_GRACE_SECS`; the last signal sent is recorded in `state_change.signal`.
- [ ] On container exit after a stop, the owner's transition depends on the reason: `User` -> `transition(running -> parked, reason = "stopped by user", signal)` (existing behaviour); `Idle` -> `transition(running -> parked, reason = "idle timeout", signal)`; `Stalled` -> `transition(running -> failed, reason = "stalled", signal, error = "stalled")`, then `on_session_ended(sid)` is invoked exactly once.
- [ ] Kind guard: `Idle` received by an ephemeral owner is treated as `Stalled` with a `warn!`; `Stalled` received by a conversational owner is treated as `Idle` with a `warn!`, so an ephemeral session can never end `parked` and a conversational one is never failed by idleness.
- [ ] After the transition the existing cleanup runs unchanged: container removed (force, NotFound ignored), `set_container_id(None)`, `registry.mark_parked` for `parked` or `registry.remove` for `failed`, stdin closed, loop returns.
- [ ] A stalled ephemeral session performs no fetch-back and no `git` event (nothing in the documents asks for one; its work clone stays on disk and `POST /sessions/{id}/sync` remains available).
- [ ] The routes and `SessionService::stop`/`end` pass `StopReason::User`; nothing else in the crate changes behaviour.
- [ ] `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Implementation notes
- Files: `orchestrator/src/session/owner.rs`, `orchestrator/src/session/registry.rs`, `orchestrator/src/session/service.rs` (call sites), `orchestrator/tests/session_owner.rs`.
- The transition uses `SessionRepository::transition(tx, id, Running, Failed, "stalled", signal, Some("stalled"))`; `ended_at` is set by the repository for `failed`.
- The `on_session_ended` hook (`AppState`, installed by the tracker epic) is what releases the leases; the owner never touches `tasks`.
- `StopOutcome::AlreadyStopping { since }` is what the idle reaper task uses to decide on a `SIGKILL` escalation; store `since` as `tokio::time::Instant` in the registry entry.

## Edge cases
- Stop of any reason while the session is `creating` (no container yet): ignored with a log line, as today.
- Container exits with a non-zero code during an `Idle` stop (the CLI crashed on SIGINT): still `parked` with the reason above, because a stop was requested; the exit code goes into the log, not the state.
- A `Stalled` stop racing the ephemeral `result` line: the owner drains the transcript to EOF before deciding; if `result` was seen, the ephemeral end-of-run (`done`) wins and the stop reason is discarded.
- Second `Stop` while one is pending is ignored regardless of reason (existing rule); the reason of the first stop stands.

## Testing
- Extend `orchestrator/tests/session_owner.rs` (mock engine with scriptable exit, capturing stdin, hook counter): `Stop { Idle }` then exit 0 -> `parked`, `state_change { from: running, to: parked, reason: "idle timeout", signal: "SIGINT" }`, `parked_at` set, container removed; `Stop { Idle }` with no exit inside the 1 s test grace -> `SIGTERM` sent, exit 143 -> `parked` with `signal: "SIGTERM"`; ephemeral `Stop { Stalled }` then exit 130 -> `failed`, `error = "stalled"`, `state_change.reason = "stalled"`, `ended_at` set, hook invoked once, no `git` event; ephemeral `Stop { Idle }` -> same as `Stalled` plus a `warn` line; conversational `Stop { Stalled }` -> `parked`; `Stop { User }` unchanged (`reason: "stopped by user"`); registry reports `AlreadyStopping` on a second stop and `NoOwner` for an unknown session.
- Command: `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- `ARCHITECTURE.md` "Stop semantics": add one sentence listing the recorded `state_change` reasons for a stop: `stopped by user`, `idle timeout` (conversational, from the idle reaper) and `stalled` (ephemeral, from the idle reaper, ending in `failed` with `error = stalled`).

## Assumes from other epics
- "Session lifecycle: launcher, owner, recovery and sessions API": `SessionOwner` with `OwnerCommand::Stop`, the SIGINT/SIGTERM sequence, the exit rules, `SessionRegistry::stop`, `SessionRepository::transition`, and the `on_session_ended` hook on `AppState`.
- "Task tracker: states, tasks, leases, dependencies and events": `release_leases_for_session` installed behind `on_session_ended`.
- "Container engine adapter": `kill(id, signal)`, mock engine.