---
id: "6f23c"
title: Exercise the terminal exec contract in tests/engine.rs on both engines through the ws Terminal adapter
status: open
priority: P2
created: "2026-09-16T20:48:18.817672676Z"
updated: "2026-09-16T20:48:18.817672676Z"
tags:
  - orchestrator
  - realtime
  - engine
  - tests
depends_on:
  - wdqcz
parent: h8kw9
---

## Summary
Extend the live engine suite so the terminal path the browser uses is proven on Podman and Docker, not only against the mock: drive `ws::terminal::Terminal` (the socket-independent adapter) against a real container through `BollardEngine`, checking the login-shell exec as uid 1000, binary output framing, input echo, resize and the exit code on close. When a session image is available the real `/bin/bash -l` as `agent` command is run; otherwise the same adapter runs `/bin/sh -l` on the alpine test image so both engines are always covered in CI.

## Documents
- `SPEC.md` "WebSocket: session stream" (terminal paragraph: exec with a PTY running `/bin/bash -l` as `agent`; `terminal_closed { exit_code }`).
- `ARCHITECTURE.md` "Engine adapter" ("exec + resize (TTY on) | yes | yes | Used by the optional terminal view"; "Anything outside this table is not used without being verified on both engines first"), "Session image" (`agent`, uid 1000, `/bin/bash` in the image contract via the CLI image base).
- `CLAUDE.md` "Testing expectations" ("Engine tests (`tests/engine.rs`) run only when `DOCKER_HOST` is set; CI runs them on both Podman and Docker").
- The epic's acceptance criterion "Terminal exec is exercised in the engine tests on both engines".

## Acceptance criteria
- [ ] `orchestrator/tests/engine.rs` gains, using the suite's `connect_or_skip`, `ensure_test_image`, unique names and the cleanup wrapper:
  1. `terminal_adapter_login_shell_as_uid_1000`: container `sleep 300` from `TEST_IMAGE`; `Terminal::open(engine, id, ["/bin/sh", "-l"], "1000:1000", 100, 40, tx)`; write `id -u; stty size\n`; collected `Data` chunks contain `1000` and `40 100` within 5 s; write `exit\n`; a `Closed { exit_code: 0 }` arrives within 5 s.
  2. `terminal_adapter_resize_and_close_while_alive`: open with `80x24`, `resize(120, 50)` returns `Ok`, write `stty size\n` and observe `50 120`; then `close()` while the shell is alive returns within 5 s with an exit code `>= -1`, and `inspect_exec` (through a bollard call in the test) shows the exec is no longer running.
  3. `terminal_adapter_on_stopped_container_is_conflict`: after the container exits, `Terminal::open` returns `Err(EngineError::Conflict(_))` (or `NotFound`, both accepted) and no reader task is left (the `tx` sender's receiver sees the channel closed).
  4. `terminal_real_session_image_bash_as_agent`: only when `MARS_TEST_SESSION_IMAGE` is set (the stub or claude image tag): run that image with `cmd = ["sleep", "300"]` and `user = "1000:1000"`, open with `TERMINAL_CMD` and `TERMINAL_USER`, write `echo $0; whoami\n`, expect `bash` and `agent` in the output, `exit\n` → `Closed { exit_code: 0 }`. Without the variable the test prints `MARS_TEST_SESSION_IMAGE not set; skipping` and passes.
- [ ] `Terminal` and `TerminalOutput` are `pub` in `mars_orchestrator::ws::terminal` so the integration test binary can use them; no test-only API is added to the engine.
- [ ] The Engine CI workflow's matrix runs the new tests unchanged (no workflow edit is required for tests 1 to 3); a note in the test file header says test 4 runs in CI once the Images workflow publishes a tag into `MARS_TEST_SESSION_IMAGE`.
- [ ] `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes with and without `DOCKER_HOST`.

## Implementation notes
- Files: `orchestrator/tests/engine.rs` (append the scenarios), `orchestrator/tests/common/engine.rs` (a `collect_terminal_output(rx, needle, timeout) -> Vec<u8>` helper), `orchestrator/src/ws/terminal.rs` (visibility only).
- Alpine's `sh -l` under uid 1000 has no home directory; set `HOME=/tmp` through the container spec `env` so the login shell does not print a warning that confuses the assertions, and strip ANSI/CR before searching output.
- PTY output echoes the input line; search for the expected values after the echoed command, or disable echo with `stty -echo` first.
- Timeouts: every wait bounded by `tokio::time::timeout(Duration::from_secs(10), ..)`.

## Edge cases
- Docker on GitHub-hosted runners runs as uid 1001 and the alpine image has no user 1000; numeric `1000:1000` still works for `exec` because no passwd lookup is required. If a login shell fails to start under that uid on one engine, record the difference in the test output and fall back to `0:0` for tests 1 and 2 with an `eprintln!`, keeping the `1000:1000` assertion for test 4.
- Podman's compat API may report no exit code for an exec killed by `close`; `-1` is accepted there (test 2 asserts `>= -1`).
- Leftover exec sessions do not block container removal with `force = true`; the cleanup wrapper already forces.

## Testing
- This task is the test addition. Commands: `cd orchestrator && DOCKER_HOST=unix://$XDG_RUNTIME_DIR/podman/podman.sock cargo test --features integration-tests --test engine terminal_` and the same with `DOCKER_HOST=unix:///var/run/docker.sock`; plus the full chain without `DOCKER_HOST`.

## Documentation
- none: the "Engine adapter" table already lists exec + resize as verified on both engines; if a test reveals an engine difference (for example a missing exit code), add the observation to that table row's Notes column in the same commit.

## Assumes from other epics
- "Container engine adapter": `tests/engine.rs` with `connect_or_skip`, `ensure_test_image`, `TEST_IMAGE`, the cleanup wrapper and `BollardEngine::exec_pty`.
- "Session container images" and "Repository scaffolding, tooling and CI": the Images and Engine CI workflows; publishing `MARS_TEST_SESSION_IMAGE` to the engine job is a CI follow-up, not part of this task.