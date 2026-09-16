---
id: nq4su
title: Implement the shared Postgres LISTEN task forwarding session_events, task_events and session_state to the fanout, with reconnect, Resync and startup wiring
status: open
priority: P0
created: "2026-09-16T20:44:55.417253048Z"
updated: "2026-09-16T20:44:55.417253048Z"
tags:
  - orchestrator
  - realtime
  - core
depends_on:
  - "4sptg"
parent: h8kw9
---

## Summary
Add the single Postgres listener connection the orchestrator holds for real-time delivery: `events::listener` connects a `sqlx::postgres::PgListener` from the pool, `LISTEN`s on the three channels, parses each payload and publishes the notice on `EventFanout`. When the connection drops it reconnects with backoff and publishes `Resync` so open streams read from their cursors at once instead of waiting for the safety read. `run()` and `TestApp::spawn()` start it.

## Documents
- `ARCHITECTURE.md` "Event delivery" (sequence diagram; "A shared Postgres listener forwards delivered notifications to the in-process broadcast channels; writers do not broadcast before commit or send a second notification afterwards"), "Orchestrator internals" (`main.rs`: "config, pool, migrations, listeners, recovery, spawn services").
- `docs/data-model.md` "Notifications (LISTEN/NOTIFY channels)" ("The shared listener fans out delivered notifications after commit. Cursor replay, deduplication and periodic safety reads remain required for lost notifications or disconnected listeners").
- ADR 0005 ("notifications are dropped if the listener is disconnected ... their loss is harmless"), ADR 0028 (acceptance: "no notification before commit, delivery after commit, rollback without delivery, batch notification followed by reading every new event").

## Acceptance criteria
- [ ] `pub fn spawn_listener(pool: PgPool, fanout: EventFanout, shutdown: CancellationToken) -> tokio::task::JoinHandle<()>` in `orchestrator/src/events/listener.rs`; the task never panics and only returns after `shutdown` is cancelled.
- [ ] On start: `PgListener::connect_with(&pool)`, `listen_all(Channel::ALL.iter().map(Channel::name))`, `info!("postgres listener connected")`. Loop on `listener.try_recv().await`: `Ok(Some(n))` → `Channel::from_name(n.channel())`, `parse_payload(channel, n.payload())`, then `fanout.publish_session(id, notice)` for `session_events` and `session_state`, `fanout.publish_project(id, notice)` for `task_events`; unknown channel or `PayloadError` → `warn!(channel = %c, error = %e, "ignoring malformed notification")` and continue (the raw payload only at `debug`).
- [ ] `Ok(None)` (sqlx reports the connection was lost and re-established) → `warn!("postgres listener reconnected; broadcasting resync")` and `fanout.publish_resync()`. `Err(e)` → `error!(error = %e, "postgres listener failed")`, sleep with exponential backoff (500 ms doubling to a 10 s cap, reset after a successful `listen_all`), reconnect (`connect_with` + `listen_all`), then `publish_resync()`.
- [ ] Shutdown: every await in the loop (including backoff sleeps) is inside a `select!` with `shutdown.cancelled()`; on cancellation the task drops the listener connection and returns.
- [ ] `run()` in `lib.rs` spawns the listener after migrations and before serving, cancels it on graceful shutdown and awaits the handle; `TestApp` stores the `JoinHandle` and `CancellationToken` in `pub listener: ListenerHandle` and cancels in `Drop`, and `TestApp::spawn()` starts it by default so stream tests get live delivery.
- [ ] Through the stack, ADR 0028 holds: a batch committed through `SessionRepository::append_events` produces exactly one `Notice::SessionEvents { seq: <highest> }` on a `subscribe_session` receiver; a rolled-back batch produces nothing; a `set_state`/`transition` produces `Notice::SessionState { state }`; a `TaskRepository::append_task_events` batch produces one `Notice::TaskEvents { seq }` on `subscribe_project`.
- [ ] `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Implementation notes
- Files: `orchestrator/src/events/listener.rs` (new), `orchestrator/src/events/mod.rs`, `orchestrator/src/lib.rs` (`run`), `orchestrator/tests/common/mod.rs`.
- `cargo add tokio-util --features rt` for `CancellationToken` and add the row `Cancellation | tokio-util` to the crate table in `ARCHITECTURE.md` "Orchestrator internals" in the same commit (skip if the startup task already added it). If the startup task's graceful shutdown uses a different primitive, adapt to it rather than having two.
- `PgListener` takes one pooled connection permanently; the pool size of 20 accounts for it. Use `try_recv()` and not `recv()`, because `recv()` hides the reconnect that must trigger `Resync`.
- This is the only Postgres `LISTEN` in the process. WebSocket and SSE handlers subscribe to the fanout only; tests observe notifications through the fanout, not through their own `PgListener`.
- Do not add a listener flag to `GET /api/health` (its contract has exactly three fields).

## Edge cases
- A notification for a session or project with no subscribers is dropped by the fanout; that is correct (rows are the truth).
- Pool exhaustion or an unreachable database at startup: `connect_with` errors → retry with backoff, log at `error` per attempt; do not abort startup from this task.
- Shutdown while sleeping in backoff must exit promptly.
- The `listen_all` call must complete before `run()` starts serving, so a stream opened immediately after startup is not missing notices: `spawn_listener` returns after the first successful `listen_all` (use a `oneshot` for readiness with a 5 s bound; on timeout log `warn!` and continue serving, the safety read covers the gap).

## Testing
- `orchestrator/tests/events_listener.rs` with `TestApp::spawn()` (seed user, project, profile and session through the repositories): the four acceptance scenarios above with `tokio::time::timeout(Duration::from_secs(5), rx.recv())`; a batch of three events yields exactly one notice with `seq == 3`; a notice for session A is not delivered to a subscriber of session B; after `SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE pid <> pg_backend_pid() AND query ILIKE 'LISTEN%'` every live subscriber receives `Notice::Resync` within 5 s and a subsequent committed batch is delivered again.
- Unit test for the pure `next_backoff(prev: Duration) -> Duration` schedule (500 ms, 1 s, 2 s, ..., capped at 10 s).
- Command: `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- `ARCHITECTURE.md` "Orchestrator internals" crate table gains `tokio-util` if this task adds it; otherwise none: implements the documented contract as written.

## Assumes from other epics
- "Database schema, models, repositories and test harness": `SessionRepository::append_events` / `set_state` and `TaskRepository::begin_mutation` / `append_task_events` issue `pg_notify` inside their transactions with the documented payloads; `TestApp` with a pool.
- "Repository scaffolding, tooling and CI": `run(state, shutdown)` with a graceful-shutdown signal the listener can share.