---
id: kunxh
title: Build the orchestrator and nginx images and validate compose files in CI
status: open
priority: P2
created: "2026-09-16T20:44:36.994925400Z"
updated: "2026-09-16T20:44:36.994925400Z"
tags:
  - infra
  - tests
depends_on:
  - c9u6c
  - h7479
  - sgg2e
parent: "5czwa"
---

## Summary
Keep the deployment artefacts from rotting: add a `Deploy` workflow that builds `orchestrator/Dockerfile` and `nginx/Dockerfile` on both Docker and Podman, runs `nginx -t` on the rendered configuration, and validates `compose.yml` with each override through `docker compose config` and `podman-compose config`. It runs on changes to the Dockerfiles, `nginx/`, the compose files, `.env.example` and the workflow itself, and the `README.md` "CI" table gets its row.

## Documents
- `README.md` "CI" table (currently four rows: Orchestrator, Frontend, E2E, Images) and "Development" (build commands added by the image tasks).
- `CLAUDE.md` "Git workflow" (CI is described in `README.md` "Development"), rule 1.
- The Orchestrator/Frontend/Images workflows from the scaffolding epic for style (`permissions: contents: read`, `concurrency`, `timeout-minutes`, action majors).

## Acceptance criteria
- [ ] `.github/workflows/deploy.yml` named `Deploy`, triggers `push` to `main` and `pull_request` with `paths: ["orchestrator/Dockerfile", "orchestrator/.dockerignore", "nginx/**", "compose.yml", "compose.podman.yml", "compose.docker.yml", ".env.example", ".dockerignore", ".github/workflows/deploy.yml"]`, `concurrency` group `deploy-${{ github.ref }}`, `permissions: contents: read`, `timeout-minutes: 30`.
- [ ] Job `images` with `strategy.matrix.engine: [docker, podman]` on `ubuntu-latest`: installs Podman for the podman leg (`sudo apt-get install -y podman`; ubuntu-latest ships Docker), builds `${{ matrix.engine }} build -t mars-orchestrator:ci orchestrator` and `${{ matrix.engine }} build -f nginx/Dockerfile -t mars-nginx:ci .`, then runs `${{ matrix.engine }} run --rm mars-orchestrator:ci git --version`, `${{ matrix.engine }} run --rm --read-only --tmpfs /tmp mars-orchestrator:ci healthcheck || test $? -eq 1` (exit 1 = connection refused is the expected outcome; any other code fails), and `${{ matrix.engine }} run --rm -e ORCHESTRATOR_HOST=127.0.0.1 -e API_PORT=7000 mars-nginx:ci nginx -t`.
- [ ] The orchestrator image build in CI uses `SQLX_OFFLINE=true` implicitly through the Dockerfile; the job must not need `DATABASE_URL` or a Postgres service.
- [ ] Job `compose` on `ubuntu-latest`: `cp .env.example .env`, `docker compose -f compose.yml -f compose.docker.yml config -q`, `pip install podman-compose` then `podman-compose -f compose.yml -f compose.podman.yml config > /dev/null`, and `grep -q 'userns_mode: keep-id'` on the rendered Podman config and `grep -q 'user: 1000:1000'` (or `"1000:1000"`) on the Docker one; a `ports:` entry for 7000 or 7001 anywhere in either rendered config fails the job (`! grep -E '(^|[^0-9])700[01]:' `).
- [ ] Rust dependency caching for the Docker image build is enabled where cheap (`docker/setup-buildx-action` + `cache-from/cache-to: type=gha` for the docker leg only); the podman leg builds cold and its `timeout-minutes` allows it.
- [ ] `README.md` "CI" gains the row `Deploy | Dockerfiles, nginx/, compose files | Build orchestrator and nginx images on Docker and Podman; nginx -t; compose config for both overrides`.

## Implementation notes
- Files: `.github/workflows/deploy.yml`, `README.md`.
- The orchestrator image build compiles the whole crate in release mode; on a cold runner this is the long pole. Keep it in the matrix anyway (both engines must build the Dockerfile) but do not run it on every orchestrator source change: the `paths` filter above deliberately excludes `orchestrator/src/**`; the Orchestrator CI workflow covers compilation, and this workflow covers packaging. If the image should be built on every source change later, that is a separate decision.
- Use `actions/checkout@v4`; no secrets; no registry push (v1 builds locally on the host, `README.md` "Start").
- Podman on `ubuntu-latest` is rootful in CI; that is fine for `build`/`run` smoke checks and is not a substitute for the rootless walkthrough task.

## Edge cases
- `podman-compose config` prints the merged YAML to stdout; older versions print warnings to stderr about unsupported keys — treat only a non-zero exit as failure.
- `docker compose config` interpolates `${DATA_DIR_HOST}` and `${ENGINE_SOCKET_HOST}` from `.env`; `.env.example` must therefore carry syntactically valid placeholder paths (they need not exist for `config`).
- If `apt-get install podman` on `ubuntu-latest` yields a Podman too old for the compat features used in the Dockerfile smoke run, pin the ubuntu runner version or install from the OpenSUSE Kubic/upstream repository, and record the version in the workflow.

## Testing
- Open a PR touching `compose.yml` and `nginx/` and confirm the workflow runs both jobs green; force a failure once by publishing port 7001 in a scratch branch to see the `grep` guard trip (record in the PR, do not merge it).

## Documentation
- `README.md` "CI" row, same commit.

## Assumes from other epics
- "Repository scaffolding, tooling and CI": existing workflow style and the `.github/workflows/` directory.