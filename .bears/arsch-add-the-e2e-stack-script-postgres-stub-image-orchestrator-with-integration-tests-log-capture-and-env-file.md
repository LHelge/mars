---
id: arsch
title: "Add the E2E stack script: Postgres, stub image, orchestrator with integration-tests, log capture and env file"
status: open
priority: P0
created: "2026-09-16T20:40:46.823541657Z"
updated: "2026-09-16T20:40:46.823541657Z"
tags:
  - frontend
  - infra
  - tests
parent: "6s8j7"
---

## Summary
Provide one script, `frontend/tests/e2e-stack.sh`, that brings up everything the Playwright scenarios need on a developer machine and in CI in exactly the same way: a fresh Postgres container, the stub session image, and the orchestrator built with `--features integration-tests` running on the host against a real engine, with its log captured to a file. The script writes the resulting connection facts to `frontend/.e2e/env`, which `playwright.config.ts` loads, so `npm run test:e2e` needs no hand-set variables. Every later task in this epic runs against this stack; the CI task calls the same script.

## Documents
- `CLAUDE.md` "Testing expectations", Frontend E2E (real orchestrator with `--features integration-tests`, `workers: 1`, stub image, fresh users through `/api/test/users`).
- `README.md` "Development", "Running locally" (Postgres command, `DOCKER_HOST`, `DATA_DIR`/`DATA_DIR_HOST` equal on the host, `MCP_URL=http://host.containers.internal:7001/mcp`, `SESSION_EXTRA_HOSTS`), "Configuration" (every variable the orchestrator requires), "CI" (E2E row), "Start" (seeded `admin`/`changeme`).
- `ARCHITECTURE.md` "Session image" (stub paragraph), "Session container specification" ("Development on the host", "Uid contract"), "Storage" (`DATA_DIR` layout).
- `SPEC.md` "Test-only routes"; "Health" (`GET /api/health` → 200 only when database and engine are up).
- `SPEC.md` "Users", paragraph "When `RESEND_API_KEY` is unset" (invite/reset links are logged at `info`; ADR 0026).

## Acceptance criteria
- [ ] `frontend/tests/e2e-stack.sh up|down|status` exists, is `bash`, passes `shellcheck`, and is wired as `npm run test:e2e:up`, `npm run test:e2e:down` in `frontend/package.json`.
- [ ] `up` performs, in order: (1) pick the engine from `E2E_ENGINE` (`podman` default, `docker` accepted) and export `DOCKER_HOST` accordingly (`unix://$XDG_RUNTIME_DIR/podman/podman.sock` or `unix:///var/run/docker.sock`) unless already set; (2) start a fresh Postgres 18 container named `mars-e2e-pg` on port `E2E_PG_PORT` (default `5433`) with user/password/db `mars`, removing any previous one so every run starts from a clean database (the seeded `admin`/`changeme` exists again); (3) build `images/stub` as `E2E_STUB_IMAGE` (default `localhost/mars-session-stub:dev`) unless `E2E_SKIP_IMAGE_BUILD=1`; (4) `cargo build --features integration-tests` in `orchestrator/` unless `E2E_SKIP_CARGO_BUILD=1`; (5) create `frontend/.e2e/data` and start the orchestrator binary in the background with stdout+stderr appended to `frontend/.e2e/orchestrator.log` and its pid in `frontend/.e2e/orchestrator.pid`; (6) poll `GET http://localhost:<API_PORT>/api/health` until it answers 200 (up to 180 s), then write `frontend/.e2e/env`.
- [ ] The orchestrator environment set by the script: `PUBLIC_URL=http://localhost:5173` (so logged invite links open in the test browser), `JWT_SECRET=e2e-not-a-real-secret`, `DATABASE_URL=postgres://mars:mars@localhost:<pg port>/mars`, `DOCKER_HOST`, `DATA_DIR` and `DATA_DIR_HOST` both the absolute path of `frontend/.e2e/data`, `MCP_URL=http://host.containers.internal:7001/mcp`, `SESSION_EXTRA_HOSTS=host.containers.internal:host-gateway`, `SECRETS_MASTER_KEYS=1=<base64 of 32 bytes generated with openssl rand at each up>`, `GIT_BOT_NAME=Mars E2E Bot`, `GIT_BOT_EMAIL=bot@example.test`, `API_PORT` (`E2E_API_PORT`, default `7000`), `MCP_PORT` (default `7001`), `SESSION_IMAGE_DEFAULT=<E2E_STUB_IMAGE>`, `STOP_GRACE_SECS=5`, `MIRROR_FETCH_INTERVAL_SECS=600`, `RUST_LOG=info`, and no `RESEND_API_KEY` (email goes to the log). Docker engine: also `MCP_URL=http://host.docker.internal:7001/mcp` and `SESSION_EXTRA_HOSTS=host.docker.internal:host-gateway`.
- [ ] `frontend/.e2e/env` contains `PLAYWRIGHT_BASE_URL=http://localhost:5173`, `PLAYWRIGHT_API_URL=http://localhost:<API_PORT>`, `PLAYWRIGHT_ORCHESTRATOR_LOG=<abs path>`, `PLAYWRIGHT_DATA_DIR=<abs path of frontend/.e2e/data>`, `PLAYWRIGHT_REPOS_DIR=<abs path of frontend/.e2e/repos>`, `PLAYWRIGHT_STUB_IMAGE=<image>`, `PLAYWRIGHT_ENGINE=<podman|docker>`; `playwright.config.ts` reads this file when present and copies each key into `process.env` only when that key is not already set.
- [ ] `down` stops the orchestrator (SIGTERM, then SIGKILL after 10 s), removes `mars-e2e-pg`, removes any container labelled `mars.session_id` left by the run, and deletes `frontend/.e2e/` except the log when `E2E_KEEP_LOG=1`; `status` prints whether each part is up.
- [ ] `frontend/.e2e/` is git-ignored; `frontend/.gitignore` is updated.
- [ ] The Vite dev proxy target for `/api` and `/ws` honours `VITE_API_PROXY_TARGET` (default `http://localhost:7000`) so a non-default `E2E_API_PORT` works; if the scaffolding task already made it configurable, reuse that variable name and do not add a second.
- [ ] Preconditions are checked with clear one-line failures: engine socket reachable (`<engine> info`), `cargo`, `git`, `openssl`, `curl` present, the port free, and on Podman that `podman unshare cat /proc/self/uid_map` shows more than one line (keep-id needs subordinate ids).
- [ ] `cd frontend && npm run test:e2e:up && npm run test:e2e && npm run test:e2e:down` passes with the existing smoke test on a machine with rootless Podman.

## Implementation notes
- Files: `frontend/tests/e2e-stack.sh` (new), `frontend/package.json` (scripts), `frontend/playwright.config.ts` (env-file loading, a 15-line synchronous parse of `KEY=VALUE` lines, no dotenv dependency), `frontend/.gitignore`, `frontend/vite.config.ts` (proxy target env, only if needed), `README.md`.
- The orchestrator runs on the host, not in a container, exactly as `README.md` "Running locally" describes: `DATA_DIR == DATA_DIR_HOST`, session containers bind-mount that path, the mirror is at `DATA_DIR/projects/<pid>/repo.git`. Tests therefore can read and write the session work clones and mirrors directly (used by the helpers task); keep the data directory under the user's home so rootless Podman and macOS VMs can mount it.
- Use `podman run` / `docker run` for Postgres through the same `$ENGINE` binary; wait for `pg_isready` inside the container before starting the orchestrator.
- The image name default `localhost/mars-session-stub:dev` avoids Podman short-name resolution prompts; pass the same name into `SESSION_IMAGE_DEFAULT` so a new project's `default` profile uses the stub without any profile edit.
- Log capture must use `RUST_LOG=info`, because the invite-acceptance scenario reads the `LogEmailClient` record (`to=`, `subject=`, `text=` with the `/invite/<token>` link) from `frontend/.e2e/orchestrator.log`.
- Never print `SECRETS_MASTER_KEYS` or `JWT_SECRET` in the script output; the values are obviously fake but rule 3 of `CLAUDE.md` still applies to logs.

## Edge cases
- A previous run left `mars-e2e-pg` or the orchestrator pid file behind: `up` removes and restarts both, never reuses a database.
- `cargo build` failing or health never turning 200: print the last 50 lines of `orchestrator.log` and exit non-zero; `up` is idempotent enough to rerun.
- `GET /api/health` returns 503 while the engine probe fails (for example the socket path is wrong): the script reports the health JSON so the user sees `engine: false`.
- Docker engine on a host whose uid is not 1000: the uid contract (`ARCHITECTURE.md` "Uid contract") is not met; print a warning that session scenarios will fail and continue (the smoke test still works).
- The script must not depend on the repository's `.env`; it sets every variable explicitly.

## Testing
- Manual: `npm run test:e2e:up`, check `status`, `curl $PLAYWRIGHT_API_URL/api/health` → `{"orchestrator":true,"database":true,"engine":true}`, run the smoke test, `down` leaves no container (`podman ps -a --filter label=mars.session_id` empty).
- `shellcheck frontend/tests/e2e-stack.sh` clean.
- Command: `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:e2e`.

## Documentation
- `README.md` "Development": add an "End-to-end tests" subsection after "Frontend" listing `npm run test:e2e:up`, `npm run test:e2e`, `npm run test:e2e:down`, the `E2E_ENGINE`, `E2E_PG_PORT`, `E2E_API_PORT`, `E2E_STUB_IMAGE`, `E2E_SKIP_*` knobs and the `PLAYWRIGHT_*` variables the config reads; same commit.

## Assumes from other epics
- "Repository scaffolding, tooling and CI": Playwright skeleton (`playwright.config.ts` with `PLAYWRIGHT_BASE_URL`/`PLAYWRIGHT_API_URL`, `tests/utils/test-helpers.ts`, `npm run test:e2e`), `Config::from_env()` with the README variables, `GET /api/health`, Vite dev proxy for `/api` and `/ws`.
- "Session container images: claude and stub": `images/stub/Dockerfile` builds with `images/stub` as the context; the stub honours `MARS_STUB_*` environment knobs and reports `mars-orchestrator` from `/session/mcp.json`.
- "Authentication, users, invites and email": `POST /api/test/users` under `integration-tests`; `LogEmailClient` selected when `RESEND_API_KEY` is unset.
- "Container engine adapter": startup probe and health flag on rootless Podman and Docker.