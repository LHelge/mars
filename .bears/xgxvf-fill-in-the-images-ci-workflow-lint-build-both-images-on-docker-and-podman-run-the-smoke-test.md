---
id: xgxvf
title: "Fill in the Images CI workflow: lint, build both images on Docker and Podman, run the smoke test"
status: done
priority: P1
created: "2026-09-16T20:31:13.773117983Z"
updated: "2026-09-17T22:45:35.815214621Z"
tags:
  - images
  - infra
  - tests
depends_on:
  - yuk96
parent: deex5
assignee: claude-opus-subagent
attempts: 1
---

## Summary
Replace the scaffolding epic's Images workflow placeholder with the real `.github/workflows/images.yml`: a lint job (shellcheck, hadolint, stub unit tests, fixture validity, entrypoint-copy identity) and a build-and-smoke job that builds both images and runs `images/smoke-test.sh` on Docker and on rootless Podman, triggered by changes under `images/**`. This is the epic's "Images CI is green" acceptance item.

## Documents
- `README.md` "CI" (Images row: triggers on `images/**`; build session images; smoke-run the entrypoint), "Prerequisites" (rootless Podman 5+ or Docker 24+)
- `ARCHITECTURE.md` "Engine adapter" (both engines are exercised in CI), "Session image"
- `CLAUDE.md` "Git workflow", rule 3 (no credentials in CI configuration)
- ADR 0004

## Acceptance criteria
- [ ] `.github/workflows/images.yml` runs on `push` to `main` and on `pull_request` when paths match `images/**` or `.github/workflows/images.yml`, plus `workflow_dispatch`; concurrency group per ref with `cancel-in-progress: true`; `permissions: contents: read` only.
- [ ] Job `lint` (ubuntu-latest): `shellcheck images/claude/mars-entrypoint images/stub/mars-entrypoint images/smoke-test.sh`; `cmp images/claude/mars-entrypoint images/stub/mars-entrypoint` (fails with "entrypoint copies differ" if not identical); `hadolint` on both Dockerfiles (ignore `DL3008`); `python3 -m unittest discover -s images/stub/tests`; JSONL validity of every `images/stub/fixtures/*.jsonl`; `! grep -rEn 'sk-ant-|ghp_|Bearer [A-Za-z0-9]{20,}' images/` as the credential guard.
- [ ] Job `build-and-smoke` with `strategy.matrix.engine: [docker, podman]`, `needs: lint`, ubuntu-latest: builds `mars-session-claude:ci` from `images/claude` and `mars-session-stub:ci` from `images/stub` with `${{ matrix.engine }} build`, then runs `ENGINE=${{ matrix.engine }} CLAUDE_IMAGE=mars-session-claude:ci STUB_IMAGE=mars-session-stub:ci images/smoke-test.sh`.
- [ ] The Podman leg runs rootless as the runner user: `systemctl --user enable --now podman.socket` is not needed (the smoke script uses the CLI), but `podman info` is printed first and `XDG_RUNTIME_DIR` is set; if the preinstalled Podman is older than 4.x, the job installs a current one (document the source in a comment).
- [ ] Docker layer cache for the claude image via `docker/build-push-action` with `cache-from/cache-to: type=gha` on the Docker leg (optional on Podman); no image is pushed to any registry in v1.
- [ ] Build logs never print environment variables; no secrets are referenced by the workflow.
- [ ] The workflow is green on `main` on both legs, and the E2E workflow placeholder (owned by the Playwright epic) can reuse the stub build step by calling the same `podman build`/`docker build` command (no reusable-workflow plumbing required now).

## Implementation notes
- File: `.github/workflows/images.yml` (replace the placeholder created by the scaffolding epic; keep its name `Images` so branch-protection rules match).
- Pin actions by major version (`actions/checkout@v4`, `docker/setup-buildx-action@v3`, `docker/build-push-action@v6`, `hadolint/hadolint-action@v3`, `ludeeus/action-shellcheck@2`), consistent with the other workflows the scaffolding epic wrote.
- Run the smoke script with `bash -x` off; its own `ok:/FAIL:` lines are the report. On failure upload `<tmp>/log/*` from the smoke run as an artifact (`actions/upload-artifact@v4`, `if: failure()`); the smoke script must therefore accept `SMOKE_KEEP_DIR=<path>` to write its temp files under a known directory.
- Version drift guard: a step compares `ARG CLAUDE_CODE_VERSION` in the Dockerfile with the version string in `README.md` "Session image" only if the README hard-codes one; with the `sed`-based README command there is nothing to compare, so skip it.

## Edge cases
- GitHub runners have `docker` and `podman` preinstalled; Podman's rootless storage under `/home/runner` is fine for bind mounts; the smoke script's temp dir must be under `$HOME` on the Podman leg.
- `keep-id` user namespaces need `newuidmap` subordinate ids for the runner user; verify with `podman unshare cat /proc/self/uid_map` in a diagnostic step and fail early with a clear message.
- The claude image build downloads the npm package; a transient registry failure should be retried once (`nick-fields/retry@v3` or a shell loop), not marked as a contract failure.
- Fork PRs get read-only tokens; nothing in this workflow needs more.

## Testing
- Open a PR touching `images/` and confirm both matrix legs and the lint job pass; deliberately break `images/stub/mars-entrypoint` (one byte) in a scratch branch to confirm the `cmp` guard fails.
- No cargo or frontend chain applies.

## Documentation
- `README.md` "CI" Images row is verified by the documentation task; if this workflow adds the Podman leg, that row's "Checks" cell says "Build session images on Docker and Podman; smoke-run the entrypoint" (coordinate with the docs task, or make the one-word change here in the same commit).

## Assumes from other epics
- "Repository scaffolding, tooling and CI" has created the `.github/workflows/` directory with an `Images` placeholder workflow and the action-pinning convention.