---
id: ev67v
title: Make the startup probe directory writable by uid 1000 so a uid mismatch is reported as such
status: done
priority: P1
created: "2026-09-17T19:21:13.359721837Z"
updated: "2026-09-17T19:58:55.311644888Z"
tags:
  - orchestrator
  - engine
depends_on:
  - "9ecwn"
parent: naqhy
assignee: claude-opus-subagent
attempts: 1
---

## Summary
`run_startup_probe` (`orchestrator/src/engine/probe.rs`) creates `DATA_DIR/tmp/probe-<random>/{work,home,log}` with mode 0o755, owned by the orchestrator's own uid. Under Docker with the orchestrator running as a uid other than 1000 (GitHub-hosted runners: 1001) the container's uid 1000 cannot write into `work/`, so the probe fails with `probe container exited with code 1` instead of the documented `probe file is owned by uid 1000, orchestrator runs as uid 1001: ...` line. The epic's criterion is "the probe fails startup with a clear log line when uid mapping is wrong"; the exit-code message does not name the cause. Found while writing `tests/engine.rs` (task 9ecwn), whose scenarios 15 and 16 currently accept the three possible messages on the Docker branch.

## Documents
- `ARCHITECTURE.md` "Engine adapter" → "Startup probe" (add one sentence: the probe directory is world-writable so the check measures ownership, not permissions)
- `ARCHITECTURE.md` "Session container specification" → "Uid contract" (unchanged; the fix makes the documented Docker message reachable)

## Acceptance criteria
- [ ] The three probe subdirectories (`work`, `home`, `log`) are mode 0o777 after creation regardless of the process umask (`set_permissions` after `create_dir_all`; `DirBuilder::mode` alone is subject to umask). `DATA_DIR/tmp` and the `probe-<random>` directory itself keep 0o755, because the uid check reads the orchestrator's uid off `probe-<random>`.
- [ ] A unit test in `probe.rs` asserts the mode of the created subdirectories.
- [ ] `tests/engine.rs` scenarios 15 (`startup_probe_end_to_end`) and 16 (`bootstrap_engine_end_to_end`) assert, on Docker with a process uid other than 1000, only the exact prefix `probe file is owned by uid 1000, orchestrator runs as uid`; the alternative "exited with code" and "was not written" acceptances are removed.
- [ ] `ARCHITECTURE.md` "Startup probe" gains the sentence in the same commit.
- [ ] `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo clippy --all-targets --features integration-tests -- -D warnings && cargo test --features integration-tests` pass, with `DOCKER_HOST` set to rootless Podman (the engine suite runs) and unset.

## Implementation notes
- File: `orchestrator/src/engine/probe.rs` (`create_probe_dirs`), `orchestrator/tests/engine.rs` (the two probe scenarios), `ARCHITECTURE.md`.
- The directories are throwaway: created under `DATA_DIR/tmp`, removed after the probe, and swept by orphan cleanup if the process dies; a world-writable mode on them is not a weakening of anything.
- Do not change the probe's failure strings.

## Edge cases
- Rootless Podman with `keep-id` maps the container's uid 1000 to the orchestrator's uid, so the mode change is invisible there; the engine suite on Podman must stay green unchanged.
- Docker with the orchestrator as uid 1000: unchanged outcome (pass).

## Testing
- Unit: mode assertion in `probe.rs`.
- Live: the Engine CI workflow's Docker job (runner uid 1001) shows the uid-mismatch line in its log and the two scenarios pass on that branch; the Podman job passes as before.

## Documentation
- `ARCHITECTURE.md` "Startup probe" paragraph, one sentence, same commit (rule 1).