---
id: qwesr
title: Add session and event models and SessionRepository with the locked event-append primitive
status: in_progress
priority: P1
created: "2026-09-16T20:31:14.957852491Z"
updated: "2026-09-17T07:17:22.163689832Z"
tags:
  - orchestrator
  - core
  - sessions
  - realtime
depends_on:
  - k42gy
parent: p5tsd
assignee: claude-opus-subagent
attempts: 1
---

## Summary
Add the `Session` domain type (with `SessionState`, its transition table and `SessionError`), the `events` row types, and `SessionRepository<'a>` with basic CRUD, the session-row lock, the state-change write that issues `pg_notify('session_state', ...)`, and the event-append primitive: lock the session row, insert each event with `COALESCE(MAX(seq), 0) + 1`, update `last_seq` and `last_activity_at`, and `pg_notify('session_events', '<session_id>:<seq>')` in the same transaction. The session owner, launcher, recovery and git handlers all call this one path.

## Documents
- `docs/data-model.md` "Sessions and events" (`sessions` columns, `events` columns, the SQL for lock + insert, "a primary-key collision indicates a writer bypassed the locking contract ... roll back and surface an error"), "Notifications (LISTEN/NOTIFY channels)" (`session_events` payload `<session_id>:<seq>`, `session_state` payload `<session_id>:<state>`).
- `ARCHITECTURE.md` "Session owner task" (no in-memory counter; every writer locks the row first), "Session lifecycle" (state diagram and table), "Event delivery" (notify in the writing transaction), ADR 0021, ADR 0028.
- `SPEC.md` "Sessions" (`Session` DTO fields; `GET /sessions/{id}/events` `?before=&limit≤500`, newest-last), "AgentEvent" (`seq`, `ts` and `kind` are columns; payload fields starting with `_` are internal).

## Acceptance criteria
- [ ] `SessionState { Creating, Running, Parked, Done, Failed }` derives `sqlx::Type` (`session_state`) and serde snake_case; `SessionState::can_transition_to(self, to) -> bool` encodes exactly the diagram: `creating→running|failed`, `running→parked|failed|done`, `parked→running|done|failed`, `failed→parked`, `done→` nothing; `SessionError::InvalidTransition { from, to }` otherwise.
- [ ] `Session` row struct has every column including `task_id` and `handoff_id`; `mcp_token_hash` is `#[serde(skip)]`. `NewSession { id, project_id, profile_id, kind, created_by, title, base_ref, branch, mcp_token_hash, task_id, handoff_id }` with `branch` validated to equal `format!("session/{id}")` (`SessionError::InvalidBranch`) and `title`, when given, trimmed and non-empty.
- [ ] `models/event.rs`: `EventRow { session_id, seq: i64, ts, kind: String, payload: serde_json::Value }` and `NewEvent { ts, kind, payload }`.
- [ ] `SessionRepository<'a>`: `insert(tx, &NewSession)` (unique `mcp_token_hash` → `Error::Internal`, it is a random collision or a bug), `find(id)`, `find_in_project(project_id, id)`, `list_by_project(project_id, state: Option<SessionState>)` ordered by `created_at DESC`, `list_all(state: Option<SessionState>)`, `update_title(tx, id, title)`, `set_container_id`, `set_cli_session_id`, `set_mcp_token_hash(tx, id, hash)`, `delete(tx, id) -> bool`.
- [ ] `lock_session(tx, id) -> Result<Session>` runs `SELECT ... FROM sessions WHERE id = $1 FOR UPDATE`, `NotFound` when missing.
- [ ] `set_state(tx, id, to, StateChange { error?, ... })` reads the current state under the lock, validates the transition, updates `state`, sets `parked_at = NOW()` on entering `parked` and `ended_at = NOW()` on entering `done`/`failed`, stores `error` on `failed`, and executes `SELECT pg_notify('session_state', $1)` with `format!("{id}:{state}")` in the same transaction. It does not insert the `state_change` event itself; callers append it with `append_events` in the same transaction.
- [ ] `append_events(tx, session_id, events: &[NewEvent]) -> Result<Vec<i64>>`: locks the row, then for each event `INSERT INTO events (session_id, seq, ts, kind, payload) SELECT $1, COALESCE(MAX(seq), 0) + 1, $2, $3, $4 FROM events WHERE session_id = $1 RETURNING seq`, then `UPDATE sessions SET last_seq = $2, last_activity_at = NOW() WHERE id = $1`, then one `SELECT pg_notify('session_events', $1)` with `<session_id>:<highest seq>`; an empty slice is a no-op returning `Ok(vec![])`. A unique violation on `events_pkey` is returned as `Error::Internal("event sequence collision")` and logged with `tracing::error!(session_id = %id)`.
- [ ] `add_usage(tx, id, cost_usd: f64, input_tokens: i64, output_tokens: i64)` increments the three counters (the owner calls it in the same transaction as the `result` event).
- [ ] `list_events(session_id, before: Option<i64>, limit: u32) -> Result<(Vec<EventRow>, bool)>` returns the `limit` events with `seq < before` (or the newest when `before` is `None`) ordered ascending, plus `has_more`; `limit` is clamped to 500. `list_events_after(session_id, after: i64) -> Vec<EventRow>` for replay. `max_seq(session_id) -> i64`. `max_offset(session_id) -> Option<i64>` reading `MAX((payload->>'_offset')::bigint)`.
- [ ] `cargo sqlx prepare` run and `.sqlx/` committed.

## Implementation notes
- Files: `orchestrator/src/models/session.rs`, `models/event.rs`, `src/repositories/sessions.rs`, `src/prelude/error.rs` (`#[from] SessionError` → 409 for `InvalidTransition`, 400 otherwise).
- Every helper takes `&mut PgConnection`; `append_events` documents: "the caller's transaction must not already hold a session row lock for another session and, if it also mutates tracker rows, must have locked the project row first (ADR 0021)".
- Payloads are stored as given; stripping `_`-prefixed fields on read is the realtime/sessions epics' concern at the DTO boundary, but `EventRow::public_payload()` may be provided here as a pure helper.
- Never log payloads at `info` or above.

## Edge cases
- Two writers appending concurrently for the same session must serialize on the row lock and produce consecutive sequences with no gaps; a writer for a different session must not wait.
- `set_state` to the current state is `InvalidTransition` (no self-loops in the diagram).
- `list_events` with `before = Some(1)` returns empty and `has_more = false`.
- `add_usage` with negative values is rejected with `SessionError::InvalidUsage`.

## Testing
- Unit tests: transition table (every allowed pair and a sample of forbidden ones), `branch` validation, usage validation.
- Integration tests in `tests/repositories_sessions.rs` (seed a user, project and profile directly through the repositories or SQL): insert/find/list; `append_events` returns 1..n and `last_seq` matches; 8 tasks appending 25 events each concurrently yield seq 1..200 with no gaps or duplicates; a `PgListener` on `session_events` receives exactly one notification `<id>:<max seq>` per committed batch and nothing when the transaction is rolled back (ADR 0028 acceptance); `session_state` notification on `set_state`; `InvalidTransition` for `done → running`; `list_events` pagination with `before`/`limit` and `has_more`; `max_offset` reads `_offset` from payloads.
- `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` must pass.

## Documentation
- None: implements the documented contract as written.

## Assumes from other epics
- "Repository scaffolding, tooling and CI": `prelude::Error`; the `unique_violation` helper introduced by the UserRepository task (add it if that task is not yet merged).
- `AgentEvent`/`TaskEvent` typed payloads are defined by the agent and realtime epics; this repository stores `serde_json::Value`.