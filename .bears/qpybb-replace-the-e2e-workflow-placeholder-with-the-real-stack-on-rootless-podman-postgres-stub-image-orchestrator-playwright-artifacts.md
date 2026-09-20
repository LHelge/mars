---
id: qpybb
title: "Replace the E2E workflow placeholder with the real stack on rootless Podman: Postgres, stub image, orchestrator, Playwright, artifacts"
status: done
priority: P1
created: "2026-09-16T20:42:10.894509572Z"
updated: "2026-09-20T17:25:49.144195964Z"
tags:
  - infra
  - tests
depends_on:
  - arsch
parent: "6s8j7"
attempts: 1
---

## Summary
Fill the commented slots the scaffolding epic left in `.github/workflows/e2e.yml` so CI runs the whole Playwright suite against the real stack: rootless Podman on the runner, Postgres, the stub image built from `images/stub`, the orchestrator built with `--features integration-tests`, and the Vite dev server started by Playwright's `webServer`. The workflow calls `frontend/tests/e2e-stack.sh` so CI and local runs cannot drift, and uploads the Playwright report and the orchestrator log on failure.

## Documents
- `README.md` "CI" (E2E row: triggers on `orchestrator/**` or `frontend/**`, checks "Playwright against a real orchestrator, Postgres and the stub session image"), "Prerequisites" (rootless Podman 5+ or Docker 24+).
- `CLAUDE.md` "Testing expectations", Frontend E2E; "Git workflow"; rule 3 (no credentials in CI configuration).
- `ARCHITECTURE.md` "Session container specification", "Uid contract" (Docker requires the orchestrator to run as uid 1000; Podman `keep-id` maps any host uid), "Engine adapter".

## Acceptance criteria
- [ ] `.github/workflows/e2e.yml` keeps its name `E2E`, triggers (`push` to `main`, `pull_request` on `orchestrator/**`, `frontend/**`, `images/**`, the workflow file; plus `workflow_dispatch`), `concurrency` group and `permissions: contents: read`; job `playwright` on `ubuntu-latest` with `timeout-minutes: 45`.
- [ ] Steps, in order: checkout; `actions/setup-node@v4` (node 22, npm cache on `frontend/package-lock.json`); Rust toolchain from `rust-toolchain.toml` with `Swatinem/rust-cache@v2` keyed on `orchestrator/Cargo.lock` and the feature flag; `npm ci` in `frontend/`; `npx playwright install --with-deps chromium`; a diagnostic step printing `podman info --format '{{.Host.Security.Rootless}} {{.Version.Version}}'` and `podman unshare cat /proc/self/uid_map` and failing early with "rootless Podman with subordinate ids required" when the map has one line; `systemctl --user enable --now podman.socket` (with `XDG_RUNTIME_DIR` exported); `E2E_ENGINE=podman npm run test:e2e:up` in `frontend/`; `npm run test:e2e` with `CI=true` (retries 1, fresh Vite server per config); `npm run test:e2e:down` with `if: always()`.
- [ ] Artifacts on failure (`if: failure()`, `actions/upload-artifact@v4`): `frontend/playwright-report`, `frontend/test-results` (traces, `retain-on-failure`), and `frontend/.e2e/orchestrator.log` (set `E2E_KEEP_LOG=1` for the `down` step). The orchestrator log contains logged invite links for throwaway test users and fake secrets only; no real credential can appear.
- [ ] The Postgres container and the stub image are produced by the stack script, not by workflow `services:`; the workflow sets `E2E_STUB_IMAGE=localhost/mars-session-stub:ci`.
- [ ] `SQLX_OFFLINE=true` is set for the cargo build so the workflow does not need a database at compile time; the `.sqlx/` cache must be current (the orchestrator CI catches staleness first).
- [ ] The workflow is green on `main` once every spec task of this epic is merged; before that it must at least be green with the smoke and helper specs.
- [ ] `actionlint` passes.

## Implementation notes
- File: `.github/workflows/e2e.yml` (replace the placeholder steps; keep the header the scaffolding task wrote).
- Podman is the CI engine because the runner user (`runner`, uid 1001) cannot satisfy the Docker uid contract without a container-side `user: 1000` and chown gymnastics; `keep-id:uid=1000,gid=1000` maps the runner to uid 1000 inside session containers. Document this choice in a comment at the top of the job.
- If the preinstalled Podman is older than 4.x, install a current one from the distribution's backports or the upstream OBS repository in a step with a comment naming the source (mirror the Images workflow's approach).
- Pin actions by major version consistent with `orchestrator.yml`, `frontend.yml` and `images.yml`.
- Cache the stub image build with `podman build --layers` (default) only; no registry push.
- Total budget: the suite is serial (`workers: 1`) and each session scenario starts a container; keep the job under 30 minutes by building the orchestrator in release-lite profile only if the debug build proves too slow (`CARGO_PROFILE_DEV_OPT_LEVEL=1` is acceptable).

## Edge cases
- Fork pull requests get a read-only token; nothing in this workflow needs more.
- `host.containers.internal` resolution: Podman 4+ adds it automatically; `SESSION_EXTRA_HOSTS=host.containers.internal:host-gateway` (set by the stack script) covers engines that do not.
- A flaky container start must not be masked by `retries: 1` silently: the coverage task adds a check that the report shows no retried tests on `main`; here, upload the report on retry as well (`if: failure() || steps.e2e.outcome == 'failure'` is unnecessary; a passed-on-retry run is still a pass).
- The `down` step must run even when `up` fails halfway (`if: always()`), so leftover session containers do not break the next job on a persistent runner.

## Testing
- Open a PR touching `frontend/**` and confirm the `E2E` workflow runs the full stack and passes; temporarily break `images/stub/claude --version` output in a scratch branch to confirm session scenarios fail and the orchestrator log artifact is uploaded.
- No cargo or frontend chain applies beyond the workflow's own run.

## Documentation
- `README.md` "CI": the E2E row's "Checks" cell becomes "Playwright against a real orchestrator, Postgres and the stub session image on rootless Podman"; same commit.

## Assumes from other epics
- "Repository scaffolding, tooling and CI": the placeholder `e2e.yml` with named slots, the action-pinning convention, `.sqlx/` committed by the orchestrator CI task.
- "Session container images: claude and stub": `images/stub` builds with `podman build`.
- "Container engine adapter": the startup probe passes on rootless Podman on `ubuntu-latest`.