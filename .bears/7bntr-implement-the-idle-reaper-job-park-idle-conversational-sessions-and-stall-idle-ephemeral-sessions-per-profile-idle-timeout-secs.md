---
id: "7bntr"
title: "Implement the idle reaper job: park idle conversational sessions and stall idle ephemeral sessions per profile idle_timeout_secs"
status: open
priority: P1
created: "2026-09-16T20:45:57.853060106Z"
updated: "2026-09-16T20:45:57.853060106Z"
tags:
  - orchestrator
  - cron
  - sessions
depends_on:
  - yb2ny
  - "9pvaj"
parent: cxmar
---

## Summary
Fill in `CronService::idle_reaper` through `session/idle_reaper.rs`: every minute select `running` sessions whose `last_activity_at` is older than their profile's `idle_timeout_secs`, send the owner an `Idle` stop for conversational sessions (ending in `parked`) or a `Stalled` stop for ephemeral sessions (ending in `failed` with `error = stalled`), escalate to `SIGKILL` when a CLI ignores both signals, and fall back to a direct transition when a `running` session has no owner. The `now` parameter gives integration tests time control without sleeping.

## Documents
- `ARCHITECTURE.md` "Background jobs" (idle reaper row: `1 min`, "Park `running` conversational sessions idle beyond their profile's timeout; stop and fail `running` ephemeral sessions idle beyond it (`stalled`)")
- `ARCHITECTURE.md` "Session owner task" step 4 (`last_activity_at`; "Idle is measured from the last event, so one long tool call with no output counts as idle; parking only between turns is a post-v1 tuning"), "Session lifecycle" (transitions and the `stalled` error), "Stop semantics", "Task tracker" -> "Liveness comes from the session" (parked counts as alive; ephemeral silence means stalled: "the reaper stops the container and marks the session `failed` with error `stalled`")
- `docs/data-model.md` `agent_profiles.idle_timeout_secs` (default 1800; "Time without any event after which a running conversational session is parked, or a running ephemeral session is treated as stalled and failed"), `sessions.last_activity_at` ("Advanced on every event; the idle reaper reads it"), `sessions.kind`, `sessions_state_idx`
- `SPEC.md` "Agent profiles" (users edit the idle timeout), "AgentEvent" `state_change`

## Acceptance criteria
- [ ] `SessionRepository::list_idle_running(now: DateTime<Utc>) -> Result<Vec<IdleSession { session: Session, idle_timeout_secs: i32 }>>` runs `SELECT s.*, p.idle_timeout_secs FROM sessions s JOIN agent_profiles p ON p.id = s.profile_id WHERE s.state = 'running' AND s.last_activity_at < $1 - make_interval(secs => p.idle_timeout_secs) ORDER BY s.last_activity_at` (bound `now`, never `NOW()`).
- [ ] `session/idle_reaper.rs`: `pub async fn reap_idle(state: &AppState, now: DateTime<Utc>) -> Result<JobReport>`; `cron/idle_reaper.rs` delegates `CronService::idle_reaper(now)` to it. For each idle session: reason = `StopReason::Idle` for `kind = conversational`, `StopReason::Stalled` for `kind = ephemeral`; `registry.stop(sid, reason)`; `Sent` -> `items += 1` and `info!(session_id = %sid, kind = ?kind, idle_secs, timeout_secs, "idle session: <parking|stalling>")`; `AlreadyStopping { since }` -> if `now - since > 2 × STOP_GRACE_SECS + 60 s` and `container_id` is set, `engine.kill(cid, "SIGKILL")` and `warn!(session_id = %sid, "CLI ignored SIGINT and SIGTERM; sent SIGKILL")` (`items += 1`), otherwise `skipped += 1`; `NoOwner` -> fallback below.
- [ ] Fallback for a `running` session without an owner: in one transaction `transition(running -> parked, reason = "idle timeout", signal = None)` for conversational or `transition(running -> failed, reason = "stalled", error = "stalled")` for ephemeral; then, if `container_id` is set, `engine.kill(cid, "SIGTERM")` followed by `engine.remove(cid, force = true)` (NotFound ignored) and `set_container_id(None)`; for `failed` invoke `on_session_ended(sid)`. A conflict from `transition` (the state changed meanwhile) is `skipped`, not a failure.
- [ ] Any per-session error is logged `error!(session_id = %sid, error = %e)` and counted as `failures += 1`; the loop continues.
- [ ] `parked`, `creating`, `done` and `failed` sessions are never selected; a session whose `last_activity_at` is within its own profile's timeout is never selected even if another profile's timeout is shorter.
- [ ] `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes; `.sqlx/` refreshed.

## Implementation notes
- Files: `orchestrator/src/session/idle_reaper.rs`, `orchestrator/src/session/mod.rs`, `orchestrator/src/cron/idle_reaper.rs`, `orchestrator/src/cron/mod.rs`, `orchestrator/src/repositories/session.rs`, `orchestrator/.sqlx/`.
- The normal path writes nothing to the database itself: the owner is the single writer for its session and performs the transition when the container exits. Lock order in the fallback: session row only (through `transition`); no project lock, no git lock.
- `SessionRegistry::stop` returning `AlreadyStopping { since }` comes from the stop-reason task; `since` is a `tokio::time::Instant`, compare against `Instant::now()` rather than `now` for that one check.
- Idle is measured from `last_activity_at`, which every appended event advances (including `user_message`); a user message to a running session therefore resets the clock.

## Edge cases
- Profile timeout edited while sessions run: the join reads the current value, so a shorter timeout applies at the next tick.
- Session stopped by the user at the same moment: the registry ignores the second stop (`AlreadyStopping`); whoever's stop was first decides the recorded reason.
- Ephemeral session whose `result` arrives during the stop: the owner's drain-to-EOF rule lets `done` win; the reaper's report already counted it as an item, which is acceptable.
- Container gone but session still `running` (engine lost it without the owner noticing): the owner's `wait` path handles it; the reaper only sends the stop.
- `STOP_GRACE_SECS = 0`: the SIGKILL threshold is 60 s.

## Testing
- Integration test `orchestrator/tests/cron_idle_reaper.rs` via `TestApp` and the mock engine: launch a conversational session and an ephemeral session (profiles with `idle_timeout_secs = 60` and `= 3600`), drive them to `running` with an `init` line, then call `app.cron().idle_reaper(last_activity + 61 s)`: the 60 s conversational session's mock container received `SIGINT`; simulate exit 0 -> `parked`, `state_change.reason = "idle timeout"`; the 3600 s session untouched; call with `+ 3601 s` on an ephemeral session -> `SIGINT`, simulate exit 130 -> `failed`, `error = "stalled"`, hook invoked; a parked session with an old `last_activity_at` is never selected; a user message appended before the tick prevents the stop; a second tick while the first stop is pending returns `skipped = 1` and sends no second `SIGINT`; with `TestApp` grace 1 s, a tick more than 62 s after the first stop (advance the registry's `since` through a test helper or `tokio::time::pause`) sends `SIGKILL`; a `running` row with no registry entry (insert directly) is parked by the fallback with the container removed and `container_id` null, and an ephemeral one is failed with `error = "stalled"` and the hook invoked.
- Command: `cd orchestrator && cargo sqlx prepare && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- `ARCHITECTURE.md` "Background jobs" idle reaper row: append "a CLI that survives both signals receives `SIGKILL` on a later tick" so the escalation is part of the contract.

## Assumes from other epics
- "Session lifecycle: launcher, owner, recovery and sessions API": `SessionRegistry`, `SessionOwner` stop handling, `SessionRepository::transition`/`set_container_id`, the `on_session_ended` hook, mock-engine-driven session tests reaching `running`.
- "Projects, agent profiles and shared directories": `agent_profiles.idle_timeout_secs` editable per profile.
- "Container engine adapter": `kill`, `remove`, mock engine signal capture.