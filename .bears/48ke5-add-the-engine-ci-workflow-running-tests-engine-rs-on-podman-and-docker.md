---
id: "48ke5"
title: Add the Engine CI workflow running tests/engine.rs on Podman and Docker
status: done
priority: P1
created: "2026-09-16T20:31:39.405256184Z"
updated: "2026-09-17T19:58:56.564963968Z"
tags:
  - infra
  - engine
  - tests
depends_on:
  - "9ecwn"
parent: naqhy
assignee: claude-opus-subagent
attempts: 1
---

## Summary
Add a GitHub Actions workflow that runs the live engine suite on both engines, which is the epic's first acceptance criterion and the evidence the write-back task needs for open questions 7 and 8. One job runs against the runner's Docker daemon, the other installs rootless Podman, starts its Docker-compatible socket and points `DOCKER_HOST` at it. The `README.md` "CI" table gains the new row.

## Documents
- `README.md` "CI" (workflow table), "Podman setup" (`systemctl --user enable --now podman.socket`, socket path), "Running locally" (`DOCKER_HOST` values for both engines)
- `CLAUDE.md` "Testing expectations" ("CI runs them on both Podman and Docker"), "Running locally" ("The CI workflows are described there too")
- ADR 0004 ("`testcontainers` tests run against whatever engine `DOCKER_HOST` names, so CI exercises both")

## Acceptance criteria
- [ ] `.github/workflows/engine.yml` named `Engine` triggers on pushes and pull requests touching `orchestrator/**` and `.github/workflows/engine.yml`, with a matrix `engine: [docker, podman]`.
- [ ] `docker` job: `ubuntu-latest`, `DOCKER_HOST=unix:///var/run/docker.sock`, runs `cargo test --features integration-tests --test engine -- --nocapture` in `orchestrator/` with `SQLX_OFFLINE=true`.
- [ ] `podman` job: `ubuntu-latest`, installs `podman` (apt, version 4.9+ from the distribution or the OpenSUSE `devel:kubic:libcontainers:unstable` repo if the distro version is too old for `host-gateway`), enables lingering is not needed on CI; starts `podman system service --time=0 unix:///tmp/podman.sock &` as the runner user (rootless), waits for the socket, exports `DOCKER_HOST=unix:///tmp/podman.sock`, verifies `curl --unix-socket /tmp/podman.sock http://d/v1.41/version` reports `Podman`, then runs the same `cargo test` command.
- [ ] Both jobs print `id -u` and the engine version at the start so the uid contract branch of the probe test is explainable from the log; the Docker job's log shows the expected probe failure branch when the runner uid is not 1000, and the workflow is green in that case (the test asserts the branch).
- [ ] The workflow uses `Swatinem/rust-cache` (or the same caching the Orchestrator CI workflow uses) and the toolchain from `rust-toolchain.toml`.
- [ ] `README.md` "CI" table gains the row `| Engine | \`orchestrator/**\` | \`tests/engine.rs\` against the runner's Docker daemon and against rootless Podman via its compatible socket |` in the same commit.
- [ ] The workflow is green on `main` for both jobs.

## Implementation notes
- Files: `.github/workflows/engine.yml`, `README.md` "CI".
- Mirror the structure of the Orchestrator CI workflow from the scaffolding epic (checkout, toolchain, cache, `SQLX_OFFLINE=true`); the engine tests need no Postgres, so do not start one.
- Rootless Podman on GitHub runners: `podman info` must show `rootless: true`; if the apt package fails to start the compat service rootless, fall back to `sudo podman system service` with `DOCKER_HOST` at the root socket and mark that clearly in the job name (`podman (rootful fallback)`), because ADR 0004 targets rootless. Prefer rootless.
- Keep the job timeout at 20 minutes and add `concurrency: { group: engine-${{ github.ref }}, cancel-in-progress: true }`.
- Optionally run with `RUST_LOG=mars=debug` so the engine detection and probe log lines are visible.

## Edge cases
- `host-gateway` in `ExtraHosts` requires Podman 4.x+: if the distro package is older, the `bootstrap_engine_end_to_end` test must not set `SESSION_EXTRA_HOSTS` on that job; record the Podman version used in the README row if it constrains the supported minimum (README says Podman 5+).
- The Docker job on a runner where uid 1000 exists but is not the runner user: unchanged, the probe test's Docker branch keys on the process uid.
- Cleanup: both jobs run `docker ps -a` / `podman ps -a` filtered by `label=mars.test` after tests as a diagnostic step (`if: always()`).

## Testing
- The workflow itself: push a branch and confirm both matrix jobs pass; attach the run URL to the PR.
- Locally: `act` is not required; the local commands are the ones in `README.md` "Running locally".

## Documentation
- `README.md` "CI" table row added in the same commit (rule 1).

## Assumes from other epics
- "Repository scaffolding, tooling and CI": the Orchestrator CI workflow whose steps this one mirrors; `rust-toolchain.toml`.
