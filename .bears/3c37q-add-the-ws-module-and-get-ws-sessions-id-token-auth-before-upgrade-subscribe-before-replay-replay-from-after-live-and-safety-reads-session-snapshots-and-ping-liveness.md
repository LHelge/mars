---
id: "3c37q"
title: "Add the ws module and GET /ws/sessions/{id}: token auth before upgrade, subscribe-before-replay, replay from ?after, live and safety reads, session snapshots and ping liveness"
status: done
priority: P1
created: "2026-09-16T20:45:35.510083978Z"
updated: "2026-09-19T21:42:50.452345187Z"
tags:
  - orchestrator
  - realtime
  - sessions
depends_on:
  - nq4su
  - "22tk6"
parent: h8kw9
attempts: 1
---

## Summary
Create `orchestrator/src/ws/` with the JSON protocol types and the session WebSocket's read side: authenticate `?token=` before upgrading, subscribe to the session's fanout channel before replaying `seq > after`, forward live events on `SessionEvents` notices, run the 30-second safety read, send a `session` message on every `SessionState` notice, ping every 30 s and close after two missed pongs. Client messages are parsed here and dispatched to hooks the input and terminal tasks fill in; the loop structure (`select!` over socket, fanout receiver and timers) is fixed by this task.

## Documents
- `SPEC.md` "WebSocket: session stream" (`GET /ws/sessions/{id}?after=<seq>&token=<jwt>`; "The server replays every event with `seq > after` from the database, then streams live events. The client keeps the highest `seq` seen and dedupes on it. Messages are JSON text frames except terminal data"; server messages `event { event: AgentEvent }`, `session { session: Session }` "on every state change", `error { message }` "followed by close"; "The orchestrator sends WebSocket pings every 30 seconds and closes after two missed pongs"), "AgentEvent" (`_`-prefixed payload fields are stripped before an event leaves the orchestrator), "Sessions" (`Session` DTO), "Authentication" (`?token=` rules; 401/403 at open).
- `ARCHITECTURE.md` "Event delivery" (sequence diagram; "The handler subscribes before it replays, so a row committed during the replay is either included in the replay or triggers a read afterwards; the client dedupes on `seq`. Older history (before `after`) is fetched over paginated REST"; "A periodic safety read (every 30 seconds) covers a lost notification"), "Frontend architecture" (nginx `location /ws/`, 30-second pings), "Orchestrator internals" (`ws/` module).
- ADRs 0005, 0025, 0028.

## Acceptance criteria
- [ ] `ws::protocol`: `#[serde(tag = "type", rename_all = "snake_case")] pub enum ServerMessage { Event { event: SessionEvent }, Session { session: Session }, InputAccepted { client_id: String, seq: i64 }, InputRejected { client_id: String, reason: String }, TerminalClosed { exit_code: i64 }, Error { message: String } }` and `pub enum ClientMessage { Input { client_id: String, input: SessionInput }, Stop, TerminalOpen { cols: u16, rows: u16 }, TerminalResize { cols: u16, rows: u16 }, TerminalClose }`, field for field as in `SPEC.md`, with round-trip unit tests against literal JSON.
- [ ] `ws::routes() -> Router<AppState>` with `.route("/ws/sessions/{id}", get(session_socket))`, merged at the root of the app router in `lib.rs` (`Router::new().nest("/api", routes::routes()).merge(ws::routes())`), still under the `TraceLayer` that excludes query strings.
- [ ] Before the upgrade: `StreamToken` + `authenticate_stream` (401 `authentication required` / 403 `password change required` as JSON bodies), `Path<Uuid>` and `SessionRepository::get` (404 `session not found`), `after` from the query (default 0; negative → 400 `after must be >= 0`; non-numeric → 400). Only then `WebSocketUpgrade::on_upgrade`.
- [ ] Inside the socket task, in this order: `rx = fanout.subscribe_session(id)`; `cursor = after`; send one `session` message with the current row (initial snapshot); replay with `SessionRepository::events_after(id, cursor, 500)` in pages until a page is short, one `event` frame per row, `cursor` advanced to each `seq`; then the live loop.
- [ ] Live loop `select!` arms: (a) `rx.recv()`: `SessionEvents`, `Resync` or `Err(Lagged)` → `events_after(id, cursor, 500)` until short, sending each; `SessionState` → reload the row and send `session`; `TaskEvents` → ignore; `Err(Closed)` → end; (b) safety interval → the same cursor read; (c) ping interval → run the re-authorization hook (a no-op closure in this task, replaced by the input task), send `Message::Ping(b"mars".into())`, increment `missed_pongs`; when `missed_pongs == 2` at a tick with no pong in between → close with code 1001 and end; (d) socket `recv()`: `Pong` → `missed_pongs = 0`; `Close`/`None`/`Err` → end; `Text` → parse `ClientMessage`; a parse failure sends `error { message: "malformed message" }` and closes with 1003; parsed messages go to `handle_client_message` (returns `Ok(())` in this task); `Binary` → the terminal hook (ignored in this task).
- [ ] Every outgoing event is a `SessionEvent` produced by the repository's `from_row`, so `_offset` never appears on the wire; any `Err` from `socket.send` ends the task without further writes.
- [ ] If the session row disappears during the stream (`get` → `NotFound` on a reload) → `error { message: "session not found" }` then close 1000.
- [ ] `pub struct StreamTimings { pub ping: Duration, pub safety_read: Duration, pub sse_keepalive: Duration }` on `AppState` as `realtime_timings` (`Default` = 30 s / 30 s / 15 s); `TestApp::spawn()` sets 200 ms / 300 ms / 100 ms. This is not an environment variable; `README.md` "Configuration" is unchanged.
- [ ] `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Implementation notes
- Files: `orchestrator/src/ws/mod.rs` (routes, handler, `SocketContext { state, session_id, principal: StreamPrincipal, cursor: i64, missed_pongs: u8, terminal: Option<Terminal> }`), `orchestrator/src/ws/protocol.rs`, `orchestrator/src/lib.rs` (merge), `orchestrator/src/prelude/state.rs` (`StreamTimings`), `orchestrator/tests/common/mod.rs` (timings; `TestApp::ws(session_id, token, after) -> axum_test::TestWebSocket`; `cargo add --dev axum-test --features ws`).
- Use `axum::extract::ws::{WebSocketUpgrade, WebSocket, Message, CloseFrame}`; split the socket with `futures_util::StreamExt::split` and route all outgoing frames through one `tokio::sync::mpsc::Sender<Message>` and a writer task, so the terminal reader (later task) and the main loop can both send without sharing the sink.
- If `events_after` has no `limit` parameter, add one (`... AND seq > $2 ORDER BY seq LIMIT $3`), run `cargo sqlx prepare` and commit `.sqlx/`.
- The `session` snapshot on connect is an addition to the spec table; add to `SPEC.md` "WebSocket: session stream", after the server-to-client table: "The server also sends one `session` message immediately after the upgrade, before replay, so the client has the current state." in the same commit.
- Log `session_id = %id, user_id = %uid` at `debug`; never log frames.

## Edge cases
- Event committed during replay: the subscription precedes the replay, so it is either read by a later page or its notice triggers a cursor read; duplicates are impossible because the cursor advances per row, and the client dedupes anyway. Prove it with a test that appends while the first page is being sent.
- `after` beyond the current `max_seq`: replay sends nothing; live reads continue from the client's cursor.
- `Lagged` → one cursor read, never close.
- Slow client: wrap each `socket.send` in `tokio::time::timeout(Duration::from_secs(10), ..)` and close on timeout.
- Drop `rx` on task exit so the fanout can clean up; assert `session_subscribers == 0` after the socket closes in a test.

## Testing
- `orchestrator/tests/ws_session_stream.rs` with `TestApp` (mock engine; seed a project, profile and a `parked` session; append events through `SessionRepository`): no token, garbage token and expired token → 401 before upgrade; gated user → 403; unknown session → 404; `after=-1` → 400; replay: 7 seeded events with `after=4` yields one `session` frame, then events 5, 6, 7 in order with no `_offset` key; live: append an event after the replay → `event` with `seq 8` within 2 s; commit-during-replay: seed 600 events, open with `after=0`, append event 601 from another task while the first page is in flight, assert the client receives 1..601 exactly once each; state change: `transition(parked → running)` produces an `event` frame (`state_change`) and a `session` frame with `state: "running"`; pings: with test timings the client receives pings and, when it stops answering, the server closes with 1001 after two intervals; safety read: cancel `TestApp.listener`, append an event, assert it arrives within `2 × safety_read`; after the client closes, `fanout.session_subscribers(id) == 0`.
- Command: `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- `SPEC.md` "WebSocket: session stream": the initial `session` snapshot sentence (see notes). Everything else implements the documented contract as written.

## Assumes from other epics
- "Session lifecycle": `SessionRepository::{get, events_after, transition}` and the `Session` DTO without `mcp_token_hash`.
- "Claude Code agent backend": `events::SessionEvent` (`seq`, `ts`, flattened `AgentEvent`) and `events::SessionInput`.
- "Authentication": `Error::Unauthorized` / `Error::Forbidden` `IntoResponse` mapping used before the upgrade.