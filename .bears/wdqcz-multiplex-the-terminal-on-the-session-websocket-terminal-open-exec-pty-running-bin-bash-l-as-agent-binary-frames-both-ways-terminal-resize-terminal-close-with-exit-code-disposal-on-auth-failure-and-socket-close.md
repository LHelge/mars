---
id: wdqcz
title: "Multiplex the terminal on the session WebSocket: terminal_open exec PTY running /bin/bash -l as agent, binary frames both ways, terminal_resize, terminal_close with exit code, disposal on auth failure and socket close"
status: open
priority: P2
created: "2026-09-16T20:47:12.486453581Z"
updated: "2026-09-16T20:47:12.486453581Z"
tags:
  - orchestrator
  - realtime
  - sessions
  - engine
depends_on:
  - wty2g
parent: h8kw9
---

## Summary
Add the optional terminal to the session socket: `terminal_open` (only while the session is `running`) creates an exec PTY in the session container running `/bin/bash -l` as the `agent` user through `ContainerEngine::exec_pty`, pumps PTY output to the client as binary frames and client binary frames to the PTY, applies `terminal_resize`, and reports `terminal_closed { exit_code }` when the shell exits or `terminal_close` arrives. The attachment is disposed on authorization failure and on socket close; nothing it does is recorded as events. The `Terminal` adapter is socket-independent so the engine test task can drive it against a real engine.

## Documents
- `SPEC.md` "WebSocket: session stream" (`terminal_open { cols, rows }` "(session must be `running`)", `terminal_resize { cols, rows }`, terminal data as binary frames both ways, `terminal_close {}`, `terminal_closed { exit_code }`; "The terminal is an `exec` with a PTY into the session container running `/bin/bash -l` as the `agent` user, multiplexed onto the same socket with binary frames. It is an escape hatch for inspection; nothing it does is recorded as events."), "Authentication" (re-check before terminal bytes; "Dispose of that socket's terminal attachment and stop forwarding its input/output").
- `ARCHITECTURE.md` "Engine adapter" ("exec + resize (TTY on) ... Used by the optional terminal view"), "Session image" (`agent`, uid 1000), "User authentication and revocation" ("Invalid authorization closes the connection and its terminal attachment, without stopping agent sessions").
- ADR 0025.

## Acceptance criteria
- [ ] `ws::terminal` exports `pub const TERMINAL_CMD: &[&str] = &["/bin/bash", "-l"];` and `pub const TERMINAL_USER: &str = "1000:1000";` (numeric uid:gid of the image's `agent` user, matching the container's `User`; the engine test image has no `agent` name).
- [ ] `pub struct Terminal` wraps a `Box<dyn ExecSession>` and an `mpsc::Sender<TerminalOutput>` where `pub enum TerminalOutput { Data(Bytes), Closed { exit_code: i64 } }`: `Terminal::open(engine: &dyn ContainerEngine, container: &ContainerId, cmd: &[String], user: &str, cols: u16, rows: u16, out: mpsc::Sender<TerminalOutput>) -> Result<Terminal, EngineError>` starts a reader task forwarding each `read()` chunk as `Data` and, on `Ok(None)` or `Err`, calls `close()` and sends `Closed { exit_code }`; `write(&self, &[u8]) -> Result<(), EngineError>`; `resize(&self, cols, rows)`; `close(self) -> i64` (aborts the reader, awaits `ExecSession::close` with a 5 s bound, `-1` on timeout). The WebSocket maps `Data` to `Message::Binary` and `Closed` to `ServerMessage::TerminalClosed`.
- [ ] `TerminalOpen { cols, rows }` in the socket: if a terminal is already open → ignore with `debug!`; reload the session row; state not `running` or `container_id` null → `TerminalClosed { exit_code: -1 }` (no `error`, since `error` implies close); otherwise `Terminal::open(engine, ContainerId(container_id), TERMINAL_CMD, TERMINAL_USER, cols.max(1), rows.max(1), tx)`; `EngineError::Conflict` / `NotFound` (container stopped meanwhile) → `TerminalClosed { exit_code: -1 }`; other engine errors → the same frame plus `tracing::error!`.
- [ ] Client `Message::Binary(bytes)` → `reauthorize` first (it is an application message), then `terminal.write(&bytes)`; with no open terminal the frame is dropped with `debug!`. A write error closes the terminal and emits `terminal_closed`.
- [ ] `TerminalResize { cols, rows }` → `terminal.resize(cols.max(1), rows.max(1))`; ignored when no terminal is open; a resize error is logged at `debug` and closes nothing.
- [ ] `TerminalClose` → `terminal.close()` → `TerminalClosed { exit_code }`; the socket stays open and a later `terminal_open` starts a fresh shell.
- [ ] Disposal: on authorization failure (before the 1008 close), on socket close, on send failure and on any task exit path, `Terminal::close` is awaited so the exec does not outlive the socket. A container stop while a terminal is open ends the PTY on its own and produces the normal `terminal_closed`.
- [ ] `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Implementation notes
- Files: `orchestrator/src/ws/terminal.rs` (new), `orchestrator/src/ws/mod.rs` (dispatch of `TerminalOpen` / `TerminalResize` / `TerminalClose` / `Binary`, disposal in the exit path, `TerminalOutput` as a fifth `select!` arm).
- `ExecSession` is `Send` but not `Sync`: keep it behind `Arc<tokio::sync::Mutex<Box<dyn ExecSession>>>`, lock per `read`/`write`/`resize` call, and never hold the lock across the socket send. If the engine epic offers a `split()` into read and write halves, prefer that.
- Output chunks are forwarded verbatim; never log terminal bytes (they can contain pasted secrets, CLAUDE.md rule 3); at most `bytes = len` at `debug`.
- Add after the terminal paragraph of `SPEC.md` "WebSocket: session stream": "A `terminal_open` that cannot be honoured (session not `running`, container gone) answers `terminal_closed` with `exit_code: -1`." because the table defines no other non-fatal reply.

## Edge cases
- `cols` / `rows` of 0 → clamped to 1 before the engine call (the engine adapter clamps too; do it here so the mock records sane values).
- Binary frames arriving after `terminal_closed` (client race) → dropped.
- `terminal_open` on a `parked` session → `terminal_closed { exit_code: -1 }`, socket stays open.
- Exec exit without an engine-reported code → `-1`, as the engine adapter documents.
- Client closes the socket mid-output: the writer task's send fails → dispose the terminal within 5 s.

## Testing
- `orchestrator/tests/ws_terminal.rs` with `TestApp`, a `running` session with a mock container (`container_id` set to a `MockEngine` container in `Running` state): `terminal_open {cols: 100, rows: 40}` → `exec_requests()` records `cmd == ["/bin/bash", "-l"]`, `user == "1000:1000"`, `cols 100`, `rows 40`; `script_exec_output` bytes arrive as one or more binary frames; a client binary frame is echoed back by the mock exec (write path); `terminal_resize {cols: 0, rows: 50}` → recorded resize `(1, 50)`; `terminal_close` → `terminal_closed { exit_code: 0 }` and a second `terminal_open` creates a second exec request; `terminal_open` on a `parked` session → `terminal_closed { exit_code: -1 }` and no exec request; revocation (bump `auth_version`) then a binary frame → `error { authentication required }`, close 1008 and the mock exec closed; client closes the socket with a terminal open → the mock exec closed within 1 s; unit tests for `Terminal` against a hand-written fake `ExecSession` (reader forwards chunks, `Closed` after `Ok(None)`, `close` bounded).
- Command: `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- `SPEC.md` "WebSocket: session stream": the `exit_code: -1` sentence (see notes). Everything else implements the documented contract as written.

## Assumes from other epics
- "Container engine adapter": `ContainerEngine::exec_pty(id, cmd, user, cols, rows) -> Box<dyn ExecSession>` with `read` / `write` / `resize` / `close`, `EngineError::Conflict` on a non-running container, and `MockEngine::{exec_requests, script_exec_output}` with an echoing `MockExecSession` that records `resize` calls; add a `closed_execs() -> usize` counter to the mock if it lacks one.
- "Session container images": both session images provide `/bin/bash` and the `agent` user with uid 1000.