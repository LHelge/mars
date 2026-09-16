---
id: m8qqn
title: Implement BollardEngine attach-stdin (TTY off) and exec PTY with resize
status: open
priority: P1
created: "2026-09-16T20:29:18.916075238Z"
updated: "2026-09-16T20:29:18.916075238Z"
tags:
  - orchestrator
  - engine
  - realtime
depends_on:
  - d9h6f
parent: naqhy
---

## Summary
Implement the two streaming operations of `BollardEngine`: `attach_stdin`, which hijacks the container's attach endpoint for stdin only (TTY off) and returns the writer the session owner uses to deliver `stream-json` input to the CLI, and `exec_pty`, which creates an exec with a PTY (used by the WebSocket terminal to run `/bin/bash -l` as the `agent` user), exposes bidirectional bytes, `resize`, and the exit code on close. The attach stream carries only stdin: stdout and stderr are files on the session volume (ADR 0010).

## Documents
- `ARCHITECTURE.md` "Engine adapter" (attach stdin TTY off "used for stdin only"; exec + resize TTY on "used by the optional terminal view")
- `ARCHITECTURE.md` "Session container specification" (Stdin row: `OpenStdin: true`, `StdinOnce: false`, `Tty: false`, `AttachStdin: true`; stdout/stderr not attached), "Session owner task" step 2, "Durability and recovery" ("the attach stream is only a pipe for stdin")
- `SPEC.md` "WebSocket: session stream" (terminal is an exec with a PTY running `/bin/bash -l` as `agent`; `terminal_open {cols, rows}`, `terminal_resize`, binary frames, `terminal_closed { exit_code }`)
- ADR 0010

## Acceptance criteria
- [ ] `attach_stdin(id)` calls `attach_container(id, AttachContainerOptions { stdin: true, stdout: false, stderr: false, stream: true, logs: false })` and returns `Box<dyn StdinWriter>` wrapping the `input` half; the `output` half is drained in a spawned task (or kept alive inside the writer) so the hijacked connection is not closed when the caller ignores output. The engine test suite verifies that bytes written after a 2-second idle still arrive (the connection is alive).
- [ ] Writing to the returned writer after the container exited returns an `std::io::Error` (broken pipe), not a hang; the owner records the failed input.
- [ ] `exec_pty(id, cmd, user, cols, rows)`: `create_exec(CreateExecOptions { cmd, user: Some(user), attach_stdin: true, attach_stdout: true, attach_stderr: true, tty: true, env: None, working_dir: None })`, then `start_exec(exec_id, StartExecOptions { detach: false, tty: true })`, then `resize_exec(exec_id, ResizeExecOptions { height: rows, width: cols })`; returns `Box<dyn ExecSession>`.
- [ ] `ExecSession::read` yields raw bytes from the PTY (`LogOutput` variants concatenated, no stream-type framing, because `tty: true` multiplexes nothing); `write` forwards to the exec input; `resize` calls `resize_exec`; `close` drops input, waits for output to end (bounded by 5 s), calls `inspect_exec` and returns `exit_code` (default `-1` when the engine reports none).
- [ ] `exec_pty` on a container that is not running returns `EngineError::Conflict` (the engine's 409) so the WebSocket handler can answer `terminal_open` with an `error` message.
- [ ] `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` pass.

## Implementation notes
- File: `orchestrator/src/engine/bollard.rs` plus a private `BollardStdin` and `BollardExec` type (may live in `orchestrator/src/engine/streams.rs`).
- bollard types: `AttachContainerResults { output: Stream<LogOutput>, input: Pin<Box<dyn AsyncWrite + Send>> }`; `StartExecResults::Attached { output, input }` versus `Detached` (treat `Detached` as `EngineError::Unsupported`).
- `BollardStdin` implements `AsyncWrite` by delegating to `input`; hold a `JoinHandle` for the output-drain task and abort it in `Drop`.
- `BollardExec` holds `exec_id`, `docker` clone, `input`, `output` stream, and a `closed` flag; `read` returns `Ok(None)` when the stream ends.
- User for the terminal is passed by the caller (`1000:1000`, the image's `agent` uid, matching the container's `User`); do not hard-code a user name because the engine test image is not the session image.
- No new `HostConfig` fields are involved; exec options are not part of the table but `tty`, `user`, `cmd`, attach flags are the compatible subset on both engines (ADR 0004).

## Edge cases
- Podman's compat API closes the attach connection when the container stops; the writer must then return an error on the next write rather than buffering forever. Test on both engines.
- `resize_exec` before the process has a PTY can return 409/500 on Docker in a race directly after `start_exec`; retry once after 50 ms, then propagate.
- Zero `cols`/`rows` from a client: clamp to 1 before calling the engine.
- Large writes: `AsyncWrite::poll_write` may accept partial writes; the owner writes whole lines followed by `\n` and calls `flush`.
- Never log terminal bytes or stdin payloads (they can contain user messages and secrets pasted by users); log only `container = %id`, byte counts at `debug`.

## Testing
- Unit tests: `BollardStdin` delegates `poll_write`/`poll_flush`/`poll_shutdown` (test with an in-memory `tokio::io::duplex` as `input`); `BollardExec::read` concatenation from a fake `LogOutput` stream.
- Live scenarios go in `tests/engine.rs` (engine test suite task): attach stdin to `cat > /mnt/out/echo.txt` then read the bind-mounted file; exec PTY `sh -c "stty size; cat"` with `cols=100, rows=40` shows `40 100`, echo of typed bytes, `resize` reflected by a second `stty size`, `close` returns 0; exec on a stopped container → `Conflict`; writes after container exit error out.
- `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` must pass.

## Documentation
- None: implements the documented contract as written.

## Assumes from other epics
- "Real-time delivery": the WebSocket handler builds `cmd = ["/bin/bash", "-l"]`, `user = "1000:1000"` and maps `ExecSession` to `terminal`/`terminal_closed` frames.
- "Session lifecycle": the owner keeps the `StdinWriter` and reattaches after restart.
