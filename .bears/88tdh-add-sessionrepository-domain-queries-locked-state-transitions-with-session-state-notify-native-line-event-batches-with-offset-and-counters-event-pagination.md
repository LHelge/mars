---
id: "88tdh"
title: "Add SessionRepository domain queries: locked state transitions with session_state notify, native-line event batches with _offset and counters, event pagination"
status: done
priority: P0
created: "2026-09-16T20:29:04.674000401Z"
updated: "2026-09-18T20:35:52.881514830Z"
tags:
  - orchestrator
  - sessions
  - core
depends_on:
  - cksdv
parent: s52qg
attempts: 1
---

## Summary
Extend `SessionRepository<'a>` (and the event-append primitive from the schema epic) with every query the session lifecycle needs: insert with the MCP token hash, list/get/update/delete, a locked state-transition helper that writes the `state_change` event and `pg_notify('session_state', ...)` in the same transaction, the per-native-line batch append that commits events, `_offset`, `last_seq`, `last_activity_at` and cost counters together, `MAX(_offset)` for recovery, and `GET /sessions/{id}/events` pagination. All SQL for sessions lives here; no other module in the epic writes SQL.

## Documents
- `docs/data-model.md` `sessions` (columns, indexes, `parked_at`/`ended_at` rules, `last_seq` cache, counters), `events` (lock statement, `MAX(seq)+1` insert, batch-commit rule, unique violation is an invariant failure), "Notifications" (`session_events` `<session_id>:<seq>`, `session_state` `<session_id>:<state>`, issued within the writing transaction).
- `ARCHITECTURE.md` "Session owner task" (no in-memory counter; lock session row then derive `seq`; offset rechecked under the lock; a line's events, offset and counters commit together), "Durability and recovery" (`_offset` is the byte offset just past the native line, advanced only on the last event of a line), "Cost accounting", "Event delivery".
- `SPEC.md` "Sessions" (`GET /sessions/{id}/events?before=<seq>&limit=<n≤500>` → `{events, has_more}` newest-last, ending just before `before`; `GET /projects/{pid}/sessions?state=`; `GET /sessions?state=`), "AgentEvent" (`_`-prefixed payload fields are stripped before an event leaves the orchestrator).
- ADRs 0021, 0028.

## Acceptance criteria
- [ ] `insert(&self, tx: &mut Transaction, NewSessionRow { id, project_id, profile_id, kind, created_by, title, base_ref, branch, mcp_token_hash, task_id, handoff_id }) -> Result<Session>` inserts with `state = creating` and accepts the caller's transaction (launch-for-task shares it).
- [ ] `get(id) -> Result<Session>` (`Error::NotFound` when absent), `get_in_project(id, project_id)`, `list_by_project(project_id, state: Option<SessionState>)`, `list_all(state: Option<SessionState>)` ordered `created_at DESC`, `list_by_state(state)`, `update_title(id, title) -> Session`, `set_container_id(id, Option<String>)`, `set_cli_session_id(id, &str)`, `set_mcp_token_hash(tx, id, hash)`, `delete(id)`.
- [ ] `transition(&self, tx, id, from: SessionState, to: SessionState, reason: &str, signal: Option<StopSignal>, error: Option<&str>) -> Result<Session>` does, in the caller's transaction: `SELECT ... FROM sessions WHERE id = $1 FOR UPDATE`; returns `Error::Conflict("session is <current state>")` if the current state is not `from`; returns `SessionError::InvalidTransition` if the model forbids `from → to`; `UPDATE sessions SET state = $to, parked_at = (NOW() when to = parked), ended_at = (NOW() when to ∈ {done, failed}), error = ($error when to = failed, NULL when to = parked from failed)`; appends one `state_change` event `{from, to, reason, signal?}` through the event primitive; executes `SELECT pg_notify('session_state', $1 || ':' || $2)` with bound parameters. One row lock, one transaction.
- [ ] `append_native_line(&self, tx, session_id, events: &[AgentEvent], line_end_offset: u64, expected_prev_offset: u64, cost: Option<CostDelta { cost_usd: f64, input_tokens: i64, output_tokens: i64 }>) -> Result<AppendedRange { first_seq, last_seq }>`: locks the session row; reads `COALESCE(MAX((payload->>'_offset')::bigint), 0)` and returns `Error::Conflict("transcript offset moved")` if it differs from `expected_prev_offset`; inserts each event with `seq = COALESCE(MAX(seq),0)+1` in order; only the last event's payload carries `_offset = line_end_offset`; updates `last_seq`, `last_activity_at = NOW()`, and adds `cost` to `cost_usd`, `input_tokens`, `output_tokens` when given; issues exactly one `SELECT pg_notify('session_events', $1 || ':' || $2)` with the highest seq. A unique violation is returned as `Error::Internal` after rollback, never retried partially.
- [ ] `append_event(&self, tx, session_id, event: &AgentEvent) -> Result<i64>` (single event, no offset, notify) is the primitive used by git handlers, the launcher, recovery and the reaper; if the schema epic already provides it under another name, re-export rather than duplicate.
- [ ] `max_offset(session_id) -> Result<u64>` returns the highest `_offset` or 0.
- [ ] `events_page(session_id, before: Option<i64>, limit: u32) -> Result<(Vec<AgentEvent>, bool)>`: `WHERE session_id = $1 AND ($2 IS NULL OR seq < $2) ORDER BY seq DESC LIMIT $3 + 1`, reversed to newest-last, `has_more` true when the extra row existed; `_`-prefixed payload fields stripped on read.
- [ ] `events_after(session_id, after: i64) -> Result<Vec<AgentEvent>>` for the real-time epic's replay.
- [ ] `fail_all_creating(&self, reason: &str) -> Result<Vec<Session>>` transitions every `creating` session to `failed` with `error = reason` using `transition` per row (each in its own transaction so one failure does not block the others).
- [ ] `.sqlx/` regenerated with `cargo sqlx prepare` and committed.

## Implementation notes
- Files: `orchestrator/src/repositories/session.rs`, `orchestrator/src/repositories/event.rs` (extend), `orchestrator/src/repositories/mod.rs`.
- Every query uses `sqlx::query!`/`query_as!` with the scope in the `WHERE` clause. Helpers take `&mut Transaction<'_, Postgres>`; convenience wrappers without a transaction open and commit one.
- `AgentEvent` (from the agent epic) serialises to `kind` + `payload`; on insert, set `ts = NOW()` (orchestrator observation time) unless the event already carries one.
- Lock order inside this module: only the session row. Never acquire a project row lock here (combined tracker/session operations lock the project first in the caller).
- Cost accumulation rule: expose `CostDelta` as the input and let the owner compute the delta (per-turn sum or increase over the previous `result`, per the agent epic's verified answer to open question 5).

## Edge cases
- `transition` with `to = failed` and no `error` sets `error = reason`.
- `transition(failed → parked)` (retry) clears `error` and `ended_at`.
- `append_native_line` with an empty `events` slice is a programming error: return `Error::Internal` rather than committing an offset without a row.
- `events_page`: `limit` outside 1..=500 is rejected by the route (400); the repository clamps defensively to 500.
- `before = Some(1)` returns an empty page with `has_more = false`.

## Testing
- Integration tests in `orchestrator/tests/sessions_repository.rs` using `TestApp::spawn()` and its pool: insert then get; list filters; `transition` happy path writes the `state_change` event and sets `parked_at`/`ended_at`/`error` exactly; wrong `from` yields 409 semantics and leaves the row unchanged; a second connection that `LISTEN`s on `session_state` and `session_events` receives the payload `<id>:<state>` / `<id>:<seq>` only after commit and nothing after a rollback (ADR 0028 acceptance); `append_native_line` with three events gives contiguous `seq`, `_offset` only on the last, counters and `last_seq` updated, and a wrong `expected_prev_offset` returns the conflict with no rows written; `max_offset` after two batches; `events_page` boundaries (`has_more` true/false, `before` cursor, stripped `_offset`); `fail_all_creating` touches only `creating` rows.
- `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes; CI builds with `SQLX_OFFLINE=true`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Database schema, models, repositories and test harness" delivers the `sessions`/`events` tables, the basic `SessionRepository` skeleton and the lock-then-`MAX(seq)+1` event-append primitive; this task extends them.
- "Claude Code agent backend and event translation" delivers `AgentEvent` in `events/` with `_`-field stripping on read; until it lands, use the type from `events/` as scaffolded by the schema epic.