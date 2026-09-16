---
id: "2f25v"
title: Implement the startup probe container and uid-ownership check
status: open
priority: P1
created: "2026-09-16T20:29:53.809182220Z"
updated: "2026-09-16T20:29:53.809182220Z"
tags:
  - orchestrator
  - engine
depends_on:
  - d9h6f
parent: naqhy
---

## Summary
Implement `orchestrator/src/engine/probe.rs`: `run_startup_probe(engine, image, data_dir, data_dir_host, ...)` runs a short-lived container from the default session image with the same `HostConfig` a session would get (via `build_probe_spec`), has it write a file into `DATA_DIR/tmp/probe-<random>/work/`, and verifies on the orchestrator side that the file is owned by the orchestrator's own uid and is writable. On Podman this proves `keep-id:uid=1000,gid=1000` is honoured (open question 8); on Docker it proves the bind-mount and uid-1000 layout is sane; on macOS it catches an unshared `DATA_DIR_HOST`. A failed probe is a fatal startup error with the reason in one clear log line.

## Documents
- `ARCHITECTURE.md` "Engine adapter" → "Startup probe" (whole paragraph), "Session container specification" → "Uid contract", "Development on the host" (macOS shared path note)
- `ARCHITECTURE.md` "Storage" (`/data/tmp/`: temporary clones and the startup probe; emptied by orphan cleanup)
- `README.md` "Podman setup" ("the orchestrator verifies this at startup with a probe container and refuses to start if Podman does not honour it"), "Configuration" (`SESSION_IMAGE_DEFAULT`, `DATA_DIR`, `DATA_DIR_HOST`)
- ADR 0004 ("a hard requirement verified with a probe container at startup")

## Acceptance criteria
- [ ] `pub async fn run_startup_probe(engine: &dyn ContainerEngine, input: ProbeInput) -> Result<ProbeReport, EngineError>` where `ProbeInput { image, data_dir, data_dir_host, network_internal, network_egress, extra_hosts }` and `ProbeReport { engine_kind, file_uid, own_uid, duration }`.
- [ ] Steps, in order: create `DATA_DIR/tmp/probe-<16 hex random>/{work,home,log}` (mode 0o755); `image_exists` → `pull_image` if absent; `create(build_probe_spec(...))`; `connect_network(egress)`; `start`; `wait` with a 60 s timeout; on timeout `kill(Sigkill)`; `remove(force = true)` always (also on error paths); check `work/probe-ok` exists, `metadata.uid()` equals the uid of the `probe-<random>` directory the orchestrator itself created (that directory's owner is the orchestrator's uid without needing `libc`), and `OpenOptions::new().append(true).open(file)` succeeds; then `remove_dir_all` the probe directory.
- [ ] Failure returns `EngineError::Probe(reason)` where `reason` is one of the exact strings: `probe container exited with code <n>` (non-zero exit), `probe container did not exit within 60s`, `probe file was not written: <path>` (missing file; typical for an unshared path on macOS), `probe file is owned by uid <file_uid>, orchestrator runs as uid <own_uid>: on Podman check that keep-id:uid=1000,gid=1000 is supported, on Docker run the orchestrator as uid 1000 with DATA_DIR_HOST owned by uid 1000` (uid mismatch), `probe file is not writable by the orchestrator: <io error>`, and `probe image pull failed: <message>`.
- [ ] Success logs one `info!` line `startup probe passed` with fields `engine_kind`, `image`, `uid`, `duration_ms`; failure is logged by the caller (`main.rs`) at `error!` with `reason = %e` and stops startup (wiring task).
- [ ] The probe never runs in `TestApp` (the mock has no filesystem); it is exercised live in `tests/engine.rs`.
- [ ] `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` pass.

## Implementation notes
- File: `orchestrator/src/engine/probe.rs`. Uses `build_probe_spec` from `spec.rs` (name `mars-probe-<random>`, label `mars.probe=true`, cmd `["sh", "-c", "touch /session/work/probe-ok"]`, user `1000:1000`, working dir `/session/work`, binds `<DATA_DIR_HOST>/tmp/probe-<random>/{work,home,log}` → `/session/{work,home,log}` rw, `open_stdin = false`, same security/userns/runtime-less `HostConfig`).
- The session image's entrypoint (`mars-entrypoint`, Images epic) `cd`s to `/session/work` and redirects stdout to `/session/log/stream.jsonl` before `exec`ing the command, which is why `log/` and `home/` are mounted too; with a plain image (engine tests use `alpine`) the command runs directly. Both work with the same spec.
- Uid comparison: `std::os::unix::fs::MetadataExt::uid()` on the probe file and on the `probe-<random>` directory; no new crate. Gid is not checked (the group may legitimately differ under keep-id).
- Random suffix: `rand::random::<u64>()` formatted as `{:016x}` (`rand` is already a crate for secrets).
- Ensure `DATA_DIR/tmp` exists (`create_dir_all`) before creating the probe dir; the orphan-cleanup job (Background jobs epic) removes leftovers if the process dies mid-probe, so also label the container `mars.probe=true` and make cleanup-by-label possible.
- Timeout via `tokio::time::timeout(Duration::from_secs(60), engine.wait(&id))`.

## Edge cases
- The default image may be absent on first start: pull it (logging at `info` `pulling default session image`), and surface a pull failure as `probe image pull failed: <engine message>`.
- Under rootless Podman without keep-id the file would be owned by a sub-uid (e.g. 100999): the uid-mismatch message must include both numbers.
- Under Docker with the orchestrator running as a uid other than 1000 the file is owned by 1000 and the check fails with the same message — this is the documented Docker contract, not a bug; the engine test on a CI runner with uid 1001 asserts this failure path.
- If `remove` fails after a successful check, log `warn!` and still return success (the container is `Exited`; orphan cleanup will remove it by label).
- Directory removal failure after success is a `warn!`, not an error.
- Never include env or secrets: the probe spec has no secrets and no `MARS_*` variables.

## Testing
- Unit tests in `probe.rs` with `MockEngine` for the control flow: pull when missing; `Conflict`/`NotFound` propagation; timeout path calls `kill` then `remove`; `remove` called on every error path (assert through the mock's records). The filesystem check is unit-tested by writing the file yourself in a `tempfile` dir and asserting the uid comparison logic in a `check_probe_file(dir, file) -> Result<(), EngineError>` helper (owned by self → Ok; a read-only file → "not writable" error; missing → "was not written").
- Live probe success (Podman) and the Docker uid-contract path are in `tests/engine.rs` (engine test suite task).
- `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` must pass.

## Documentation
- None: implements the documented contract as written. The exact failure strings above are not in the documents; add the uid-mismatch sentence to `README.md` "Podman setup" only if the implementer changes its wording.

## Assumes from other epics
- "Session container images: claude and stub": the default image's entrypoint honours `cd /session/work` and `exec`s the given command; the probe does not depend on the CLI being present.
- "Background jobs": orphan cleanup removes `/data/tmp` leftovers and stray labelled containers.
