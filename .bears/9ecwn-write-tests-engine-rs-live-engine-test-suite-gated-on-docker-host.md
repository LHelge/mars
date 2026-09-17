---
id: "9ecwn"
title: "Write tests/engine.rs: live engine test suite gated on DOCKER_HOST"
status: done
priority: P1
created: "2026-09-16T20:31:08.748480338Z"
updated: "2026-09-17T19:20:21.064641842Z"
tags:
  - orchestrator
  - engine
  - tests
depends_on:
  - m8qqn
  - "2f25v"
  - "3zfgh"
parent: naqhy
assignee: claude-opus-subagent
attempts: 1
---

## Summary
Add `orchestrator/tests/engine.rs`, the live test suite that runs `BollardEngine` against whatever engine `DOCKER_HOST` names and is skipped entirely when the variable is unset. It covers every row of the "Engine adapter" table the adapter uses, answers open questions 7 (does `SIGINT` via `kill` reach PID 1 without `Init: true`) and 8 (is `UsernsMode: keep-id:uid=1000,gid=1000` accepted through Podman's compat API) with assertions whose outcome the write-back task records, and runs the startup probe end to end.

## Documents
- `ARCHITECTURE.md` "Engine adapter" (operation table; "nested bind mounts ... Exercised by the engine tests"; `UsernsMode` "Verified at startup"; "Anything outside this table is not used without being verified on both engines first, and the verification is recorded in this table"), "Startup probe"
- `ARCHITECTURE.md` "Session image" ("If the engine tests show a signal not reaching PID 1 on either engine, the container is created with `Init: true`")
- `ARCHITECTURE.md` "Storage" (shared directories: "the engine mounts parents before children, and the engine tests verify this on both engines")
- `CLAUDE.md` "Testing expectations" ("Engine tests (`tests/engine.rs`) run only when `DOCKER_HOST` is set; CI runs them on both Podman and Docker")
- `docs/open-questions.md` items 7, 8
- ADR 0004

## Acceptance criteria
- [ ] Every test starts with `let Some(engine) = common::engine::connect_or_skip().await else { return };` which returns `None` (after `eprintln!("DOCKER_HOST not set; skipping engine test")`) when the variable is unset and otherwise `BollardEngine::connect(&docker_host)`; with `DOCKER_HOST` unset `cargo test --features integration-tests` still passes with these tests reported as passing (skipped inside).
- [ ] Test image is `docker.io/library/alpine:3.20` (constant `TEST_IMAGE`); a `pull_absent_image` test removes it first through bollard's `remove_image` if present and asserts `image_exists` false → `pull_image` ok → true; the other tests call `ensure_test_image` (pull if absent).
- [ ] Every test uses a unique container name `mars-test-<test>-<random>` and label `mars.test=<run id>`, and removes its containers with `remove(force = true)` in a guard that runs even when an assertion fails.
- [ ] Scenarios (one `#[tokio::test]` each):
  1. `create_start_wait_exit_code`: cmd `sh -c "exit 7"` → `wait` gives `code == 7`; `inspect` after gives `Exited { code: 7 }`.
  2. `kill_sigint_reaches_pid1_without_init` (open question 7): cmd `sh -c 'trap "exit 42" INT; while true; do sleep 1; done'` as PID 1, no `Init`; after `start` and a 1 s settle, `kill(Sigint)`; `wait` within 10 s must return `code == 42`. If this fails on an engine, do not mark the test ignored: the write-back task changes the spec to `Init: true` and the "Session image" paragraph; the test then asserts the new contract.
  3. `kill_sigterm_exit_code`: same loop without a trap; `kill(Sigterm)` → `wait` returns 143.
  4. `kill_on_exited_container_is_conflict`: after exit, `kill(Sigint)` → `Err(EngineError::Conflict(_))`.
  5. `remove_missing_is_ok`: `remove(ContainerId("does-not-exist"), true)` → `Ok(())`.
  6. `list_by_label_filters`: create two containers with label `mars.session_id=<uuid>` and one without; `list_by_label("mars.session_id")` contains exactly the two, `running` flags correct.
  7. `attach_stdin_delivers_bytes`: cmd `sh -c "cat > /mnt/out/echo.txt"` with a `tempfile` dir bind-mounted rw at `/mnt/out` (dir chmod 0o777 so the container user can write on Docker); `attach_stdin`, write `hello\n`, sleep 2 s, write `world\n`, `shutdown` the writer; `wait` → 0; file content equals `hello\nworld\n`.
  8. `attach_stdin_write_after_exit_fails`: attach, let the container exit (`sh -c "exit 0"`), then a write + flush returns `Err`.
  9. `exec_pty_echo_and_resize`: container `sleep 300`; `exec_pty(["sh", "-c", "stty size; cat"], "0:0", 100, 40)`; first read contains `40 100`; write `ping\n` → a read contains `ping`; `resize(120, 50)` then a second exec with `stty size` reports the size of its own PTY (each exec has its own PTY; assert `resize` returns `Ok`); `close` returns an exit code (0 after writing EOF or `-1` if the engine reports none, assert `>= -1`).
  10. `exec_on_stopped_container_is_conflict`.
  11. `nested_bind_mounts_parent_before_child`: tempdirs `parent` and `child`; binds `parent → /work` rw and `child → /work/target` rw (built through `order_binds` from a deliberately reversed list); cmd `sh -c "echo p > /work/p.txt && echo c > /work/target/c.txt"`; `wait` → 0; assert `parent/p.txt` exists, `child/c.txt` exists and `parent/target/` does not contain `c.txt`.
  12. `second_network_connected_before_start`: `ensure_network("mars-test-int-<rand>", true)`, `ensure_network("mars-test-egress-<rand>", false)`; create on the internal one, `connect_network(egress)`, `start`; `inspect().networks` contains both; cleanup removes the networks via bollard `remove_network`.
  13. `ensure_network_is_idempotent`: calling twice succeeds; the second call does not error.
  14. `userns_keep_id_accepted` (open question 8): only when `engine.kind() == Podman`: create with the session spec's `HostConfig` (via `to_bollard`) and cmd `id -u`; container output is not attached, so instead run `sh -c "id -u > /mnt/out/uid && touch /mnt/out/f"` and assert the file `f` on the host is owned by the current process's uid (compare with the tempdir's uid) and `uid` contains `1000`. On Docker the test asserts that `to_bollard` produced no `userns_mode` and skips the ownership assertion.
  15. `startup_probe_end_to_end`: `run_startup_probe` with `image = TEST_IMAGE`, `data_dir = data_dir_host = tempdir`: on Podman assert `Ok`; on Docker assert `Ok` when the process uid is 1000 and otherwise assert `Err(EngineError::Probe(msg))` with `msg` starting with `probe file is owned by uid 1000, orchestrator runs as uid`; in both cases assert the probe container is gone (`list_by_label("mars.probe")` empty) and the probe directory is removed on success.
  16. `bootstrap_engine_end_to_end`: with a `Config` built from a temporary `.env`-like map pointing `SESSION_IMAGE_DEFAULT` at `TEST_IMAGE` and unique network names; expect the same outcome rule as test 15 and clean the networks afterwards.
- [ ] The suite runs green on rootless Podman locally; the CI task makes it run on both engines.
- [ ] `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` pass with and without `DOCKER_HOST`.

## Implementation notes
- Files: `orchestrator/tests/engine.rs`, `orchestrator/tests/common/engine.rs` (`connect_or_skip`, `ensure_test_image`, `ContainerGuard` with `Drop`-spawned removal, `unique_name`, `writable_tempdir()` that chmods 0o777).
- Guard: `Drop` cannot await; keep a `tokio::runtime::Handle` and `spawn` the `remove`, or structure each test as `async fn body(engine, guard)` wrapped by a helper that awaits cleanup after the body regardless of its result (`let result = body().await; cleanup().await; result.unwrap()`). Prefer the wrapper.
- Use `ContainerSpec` literals with `user: "0:0"` for generic tests (alpine has no uid 1000 user but `1000:1000` still works numerically; use `1000:1000` where ownership matters).
- Do not hard-code `Init`; the point of test 2 is to observe the default.
- Timeouts: wrap every `wait` in `tokio::time::timeout(Duration::from_secs(30), ...)`.
- Run tests serially where they share the image removal (`pull_absent_image`): give it its own image tag (`alpine:3.19`) so it cannot race with the others.

## Edge cases
- GitHub-hosted Docker runners execute as uid 1001: tests 7, 11 and 14 rely on world-writable tempdirs, test 15 exercises the failure branch there; document this in a comment at the top of the file.
- Rootless Podman without `keep-id` maps root to the host user, so a `0:0` container writing to a tempdir produces files owned by the host user: tests 7 and 11 use `0:0` on purpose and do not assert uid; only tests 14 and 15 assert ownership.
- Leftover containers from a crashed run: a `cleanup_stale_test_containers` helper at the start of the suite removes anything labelled `mars.test`.
- `remove_network` fails while a container is still attached: remove containers first, then networks.

## Testing
- This task is the test suite. Commands: `cargo test --features integration-tests --test engine` with `DOCKER_HOST=unix://$XDG_RUNTIME_DIR/podman/podman.sock` (Podman) and `DOCKER_HOST=unix:///var/run/docker.sock` (Docker); and the full `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` without `DOCKER_HOST`.

## Documentation
- None in this task; the observed answers to open questions 7 and 8 are written back by the write-back task after CI has run on both engines.

## Assumes from other epics
- "Repository scaffolding, tooling and CI": `tests/common/mod.rs` exists (this task adds `tests/common/engine.rs` beside it).
