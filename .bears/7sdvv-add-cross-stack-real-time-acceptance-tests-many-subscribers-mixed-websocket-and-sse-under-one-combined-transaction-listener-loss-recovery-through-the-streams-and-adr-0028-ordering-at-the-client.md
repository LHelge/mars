---
id: "7sdvv"
title: "Add cross-stack real-time acceptance tests: many subscribers, mixed WebSocket and SSE under one combined transaction, listener loss recovery through the streams, and ADR 0028 ordering at the client"
status: open
priority: P2
created: "2026-09-16T20:47:49.728820951Z"
updated: "2026-09-16T20:47:49.728820951Z"
tags:
  - orchestrator
  - realtime
  - tests
depends_on:
  - wty2g
  - wdqcz
  - bkchb
parent: h8kw9
---

## Summary
Add the one test suite that exercises the whole delivery path (repository transaction → `pg_notify` → listener → fanout → WebSocket and SSE handlers → client) in scenarios no single module test covers: many concurrent subscribers, a combined tracker/session transaction observed on both stream kinds at once, listener loss recovered through the streams themselves, and the client-visible ordering guarantees of ADR 0028. It closes the epic's acceptance criteria for "no gap or duplicate" under load rather than in isolation.

## Documents
- `ARCHITECTURE.md` "Event delivery" (the whole section: subscribe-before-replay, safety read, project-keyed SSE, single-writer input), "Task tracker" ("Combined tracker/session writes acquire the project row before session row locks"; "Nothing is broadcast before commit; rollback exposes none of the changes or their events").
- `docs/data-model.md` "Notifications", `events`, `task_events`.
- ADR 0005 ("At-least-once delivery to clients with dedupe on `seq`, at any scale of subscribers"), ADR 0028 (acceptance list: "no notification before commit, delivery after commit, rollback without delivery, batch notification followed by reading every new event, and recovery through the safety read after a lost notification"), ADR 0022.
- `CLAUDE.md` "Testing expectations" (one `#[tokio::test]` per scenario; `TestApp::spawn()`).

## Acceptance criteria
- [ ] `orchestrator/tests/realtime_acceptance.rs` exists, is gated with `#![cfg(feature = "integration-tests")]`, uses `TestApp` and the `TestApp::ws` / `TestApp::sse` helpers, and contains one test per scenario below; all pass reliably ten times in a row locally (`cargo test --test realtime_acceptance -- --test-threads=1` in a loop) and in CI.
- [ ] Scenarios:
  1. `twenty_sockets_receive_every_event_once`: 20 WebSocket clients on one session with different `after` cursors (0, 5, 10, ...), then 100 events appended in batches of 1 to 7 from two concurrent writer tasks; every client receives exactly the sequences greater than its cursor, each once, in ascending order.
  2. `sse_and_ws_observe_one_combined_transaction`: a session WebSocket and a project SSE stream are open; a single transaction that locks the project row, appends a `task_events` row and appends a session event (the launch-for-task shape) commits; the SSE stream receives the task frame and the socket receives the event frame; before the commit (transaction held open for 1 s after both inserts) neither stream receives anything.
  3. `rollback_delivers_nothing_anywhere`: same shape, rolled back; after `2 × safety_read` neither stream has received a frame and `max_seq` / `max_task_event_seq` are unchanged.
  4. `listener_loss_is_recovered_by_streams`: with a socket and an SSE stream open, terminate the listener's backend (`pg_terminate_backend`) and immediately append one session event and one task event; both frames arrive within `max(5 s, 2 × safety_read)` (through `Resync` or the safety read), and a later append after reconnection arrives within 2 s.
  5. `state_change_delivers_event_and_session_frames`: `transition(parked → running)` in one transaction yields both the `state_change` `event` frame and a `session` frame with `state: "running"` on every open socket for that session (order between the two is not asserted).
  6. `replay_page_boundary_has_no_gap`: exactly 500, 501 and 1000 seeded events with `after = 0` and `after = 499` on WebSocket and `Last-Event-ID` equivalents on SSE (with `task_events`) deliver contiguous sequences with no gap or duplicate at the page boundary.
  7. `slow_client_does_not_block_others`: one socket that never reads (its receive side is left unpolled) and one that does; 200 appended events reach the reading socket within 5 s and the non-reading socket is closed by the server (send timeout) without affecting the other.
- [ ] Helpers added to `tests/common/mod.rs` are reusable: `collect_ws_events(ws, until_seq, timeout) -> Vec<i64>`, `collect_sse_ids(stream, until_seq, timeout) -> Vec<i64>`, `terminate_listener_backend(pool)`.
- [ ] `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Implementation notes
- Files: `orchestrator/tests/realtime_acceptance.rs` (new), `orchestrator/tests/common/mod.rs` (helpers), `orchestrator/tests/common/sse.rs` (frame parser from the SSE task).
- Seed data through the repositories (`ProjectRepository`, `AgentProfileRepository`, `SessionRepository`, `TaskRepository`) and never through raw SQL, except `pg_terminate_backend` and the deliberately held-open transaction in scenarios 2 and 3 (`sqlx::Transaction` kept in scope, `commit`/`rollback` explicitly).
- Use the test `StreamTimings` (200 / 300 / 100 ms) so the suite finishes in well under a minute.
- Scenario 2 must build the combined transaction in the documented lock order: `TaskRepository::begin_mutation(project_id)` (project row) → `SessionRepository::append_events(tx, ...)` (session row) → `append_task_events(tx, ...)` → commit.

## Edge cases
- Broadcast `Lagged` can occur in scenario 1 with two writers; the handlers must recover by cursor read, so the assertion is still "exactly once, ascending".
- Scenario 4 may see either `Resync` or the safety read deliver the frames; assert the outcome, not the mechanism.
- `pg_terminate_backend` needs the test role to own the connection or be superuser; the testcontainers `postgres` user is superuser.
- Keep every wait bounded with `tokio::time::timeout`; a hung test is a failure, never a hang.

## Testing
- This task is the test suite. Commands: `cd orchestrator && cargo test --features integration-tests --test realtime_acceptance` and the full `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- none: verifies the documented contract as written. If a scenario exposes a contradiction between `ARCHITECTURE.md` "Event delivery" and the implementation, fix the implementation; if the document is wrong, correct the document in the same commit and note it in the PR.

## Assumes from other epics
- "Database schema, models, repositories and test harness": `TaskRepository::begin_mutation` / `append_task_events` and `SessionRepository::append_events` accept the caller's transaction so scenario 2 can combine them.
- "Session lifecycle": `SessionRepository::transition`.