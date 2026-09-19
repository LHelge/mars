---
id: bkchb
title: "Add the sse module and GET /api/projects/{pid}/tasks/stream: token auth, subscribe-before-open, Last-Event-ID and ?after replay, id/event/data framing, 15-second keepalive with re-authorization and the safety read"
status: in_progress
priority: P1
created: "2026-09-16T20:46:05.224290171Z"
updated: "2026-09-19T21:42:54.127736490Z"
tags:
  - orchestrator
  - realtime
  - tracker
depends_on:
  - nq4su
  - "22tk6"
parent: h8kw9
attempts: 1
---

## Summary
Create `orchestrator/src/sse/` with the project task-event stream: validate `?token=` and the project before opening the response, subscribe to the project's fanout channel first, replay `task_events` rows with `seq` greater than `Last-Event-ID` or `?after`, then follow `TaskEvents` notices and the 30-second safety read, framing each row as `id: <seq>` / `event: task` / `data: <TaskEvent JSON>`. A `: keepalive` comment every 15 seconds is also the re-authorization tick; a failed check ends the response.

## Documents
- `SPEC.md` "SSE: task stream" (`GET /api/projects/{pid}/tasks/stream?token=<jwt>` with optional `Last-Event-ID: <seq>` "(or `?after=<seq>` for the first connection)"; "Each SSE message has `id: <seq>`, `event: task`, and `data: <TaskEvent JSON>`. A `: keepalive` comment is sent every 15 seconds."; "The server subscribes to project notifications before opening the SSE response, then replays events and follows live changes, with the existing periodic safety read for missed notifications"; "Historical event `task_id` values, including `deleted` events, survive deletion of the task row"), "TaskEvent" (JSON shape), "Authentication" (`?token=`; re-check at SSE keepalive ticks; "A failed authorization check closes the stream"), "Frontend architecture" (nginx: `proxy_buffering off`, `proxy_cache off`, `Connection ''` for this location).
- `ARCHITECTURE.md` "Event delivery" ("The same pattern, keyed by project, serves `TaskEvent`s over SSE with `Last-Event-ID` as the cursor. The SSE handler establishes its notification subscription before opening the response."), "Orchestrator internals" (`sse/` module).
- `docs/data-model.md` `task_events` (`seq` "used as the SSE `id`"; project FK cascades history on project deletion), "Notifications".
- ADRs 0005, 0022, 0025, 0028.

## Acceptance criteria
- [ ] `sse::routes() -> Router<AppState>` with `.route("/projects/{pid}/tasks/stream", get(task_stream))`, merged into `routes::routes()` so it sits under `/api` with the other resource routers.
- [ ] Before the response opens: `StreamToken` + `authenticate_stream` (401 / 403 JSON bodies), `ProjectRepository::find(pid)` (404 `project not found`), cursor = `Last-Event-ID` header when present, else `?after=`, else 0; a header or parameter that is not a non-negative integer → 400 `invalid cursor`. Then `rx = fanout.subscribe_project(pid)`, and only then the `axum::response::Sse` response is built.
- [ ] Response headers: `Content-Type: text/event-stream`, `Cache-Control: no-cache`, `X-Accel-Buffering: no`; no `retry:` field is ever sent (the client disables automatic reconnect).
- [ ] The body is a stream of `axum::response::sse::Event`: replay with `TaskRepository::list_task_events_after(pid, cursor, 500)` in pages until short, each row → `Event::default().id(seq.to_string()).event("task").data(serde_json::to_string(&task_event)?)`, cursor advanced per row; then a `select!` loop: `rx.recv()` `TaskEvents` / `Resync` / `Err(Lagged)` → cursor read (other notice kinds ignored; `Err(Closed)` → end); safety interval (`realtime_timings.safety_read`) → cursor read plus `ProjectRepository::find(pid)`, ending the stream when the project is gone; keepalive interval (`realtime_timings.sse_keepalive`) → `reauthorize(state, principal)` then `Event::default().comment("keepalive")` (wire form `: keepalive`); `Revoked` / `Unavailable` → the stream ends with a `debug!` log (SSE defines no error frame).
- [ ] `TaskEvent` JSON is exactly the `SPEC.md` shape as produced by the tracker epic's `TaskEvent::from_row`; the SSE module never assembles payloads itself.
- [ ] `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Implementation notes
- Files: `orchestrator/src/sse/mod.rs` (new), `orchestrator/src/routes/mod.rs` (merge `sse::routes()`), `orchestrator/tests/common/mod.rs` (`TestApp::sse(pid, token, after: Option<i64>, last_event_id: Option<i64>) -> impl Stream<Item = Bytes>` over the server's random-port HTTP transport: build the `TestServer` with `TestServerConfig { transport: Some(Transport::HttpRandomPort), .. }` and connect with `reqwest` `bytes_stream()`; `reqwest` is already a dependency, add its `stream` feature with `cargo add reqwest --features stream` if missing), plus a small `SseFrame` parser in `tests/common/sse.rs` splitting on blank lines into `{ id, event, data, comment }`.
- Do not use `Sse::keep_alive(KeepAlive)`; it cannot run the re-authorization check. The keepalive tick lives in the handler's own `select!` (`async_stream::stream!` or a hand-written `futures::stream::unfold`), so the tick and the check are the same event.
- `Last-Event-ID` takes precedence over `?after=` when both are present.
- The stream future owns `rx` (subscribed before the response); hyper drops the body future when the client disconnects, which ends the subscription.
- Log `project_id = %pid, user_id = %uid` at `debug`; never the `data` payloads.

## Edge cases
- Replay of zero rows still opens the response (the frontend waits for `open` before its first REST load); the first frame may be a keepalive.
- A row whose payload fails `TaskEvent::from_row` → `tracing::error!(project_id = %pid, seq)` and skip it (advance the cursor); never end the stream for one bad row.
- Client disconnects during replay: the body future is dropped; assert `fanout.project_subscribers(pid) == 0` afterwards.
- Cursor beyond `max_task_event_seq`: replay sends nothing; live follows from the given cursor.
- Two streams on one project both receive every event (fanout is broadcast).

## Testing
- `orchestrator/tests/sse_task_stream.rs` with `TestApp` (seed a project; append `task_events` through `TaskRepository::begin_mutation` + `append_task_events`): no token → 401 JSON; gated user → 403; unknown project → 404; `after=abc` → 400; `Last-Event-ID: x` → 400; replay: 5 seeded events with `Last-Event-ID: 2` → frames 3, 4, 5 in order, each with `id: <seq>`, `event: task` and `data` parsing as `TaskEvent` with the same `seq`; `?after=2` behaves identically; header wins over `?after=`; live: append after the replay → frame within 2 s; keepalive: with test timings a `: keepalive` comment arrives within `2 × sse_keepalive` and the connection stays open; a rolled-back mutation produces no frame; revocation: bump `auth_version` → the body ends within `2 × sse_keepalive`; `deleted` replay frame carries the original `task_id`; commit-during-replay (600 events, append during the first page) yields 1..601 exactly once; safety read: cancel `TestApp.listener`, append, assert delivery within `2 × safety_read`; project deleted mid-stream → body ends within `2 × safety_read`.
- Command: `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Task tracker": `events::TaskEvent` with `from_row(seq, ts, task_id, kind, payload)` and the documented serialisation. Until it lands, a `TaskEventRow` passthrough serialising the stored columns (`seq`, `ts`, `task_id`, `kind`, flattened `payload`) is acceptable for this module's tests and must be swapped before the epic closes.
- "Database schema, models, repositories and test harness": `TaskRepository::{begin_mutation, append_task_events, list_task_events_after, max_task_event_seq}` and `ProjectRepository::find`.
- "Authentication": as in the stream auth task.