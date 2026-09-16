---
id: sgg2e
title: Write compose.yml with engine override files, networks, volumes, health checks and the compose-only variables
status: open
priority: P1
created: "2026-09-16T20:43:28.463856293Z"
updated: "2026-09-16T20:43:28.463856293Z"
tags:
  - infra
  - docs
depends_on:
  - vbpey
  - c9u6c
  - h7479
parent: "5czwa"
---

## Summary
Write the compose deployment: `compose.yml` with the `postgres`, `orchestrator` and `nginx` services, the `mars-frontend` and `mars-sessions` (`internal: true`) networks, a Postgres 18 volume, the `DATA_DIR_HOST` bind at `/data`, the engine socket mount, health checks on `/api/health`, and two small engine override files carrying the only lines that differ between rootless Podman (`userns_mode: keep-id`) and Docker (`user: "1000:1000"`). The task also adds the compose-only variables to `.env.example` and the `README.md` "Configuration" table, and rewrites "Start" so `podman-compose up -d` / `docker compose up -d` work as documented.

## Documents
- `README.md` "Deployment shape" (orchestrator, postgres, nginx; session containers created on demand), "Running it" → "Prerequisites", "Podman setup" (`userns_mode: keep-id`, socket mount, "If the compatibility API cannot apply `keep-id` ... plain user systemd service"), the Docker paragraph (`user: "1000:1000"`, data directory owned by uid 1000), "Configuration" (every row; note "`DATABASE_URL` ... compose sets it for the orchestrator", `DATA_DIR` "/data in compose", `MCP_URL` default `http://orchestrator:7001/mcp`, `SESSION_NETWORK_INTERNAL`/`SESSION_NETWORK_EGRESS` defaults), "Start" (`podman-compose up -d  # or: docker compose up -d`; migrations on startup; `admin`/`changeme`).
- `ARCHITECTURE.md` "Components" (postgres "Not reachable from session containers"; orchestrator "Attached to both networks"; "Session containers are not in the compose file"), "Networks" (three networks; `mars-sessions` `internal: true`; `mars-egress` connects session containers and nothing else; MCP listener "binds on all interfaces but is only reachable through `mars-sessions` because nginx never forwards to it and the host does not publish its port"; the orchestrator creates both session networks at startup if missing), "Storage" (`DATA_DIR` vs `DATA_DIR_HOST`), "Uid contract", "Trust boundaries" (read-only root filesystem where the engine allows), "Session lifecycle" → "Stop semantics" (`STOP_GRACE_SECS`).
- `SPEC.md` "Health" (compose health checks), "Authentication" (`Secure` cookie when `PUBLIC_URL` is https).
- ADR 0004 (Docker keeps working; the only deployment difference is `DOCKER_HOST`), ADR 0012.
- `CLAUDE.md` rule 1 (README "Configuration" and `.env.example` carry the same variables; new ADR when a real alternative was rejected); "Backend conventions" (`.env.example` is the variable contract).

## Acceptance criteria
- [ ] `/compose.yml` declares `services.postgres`: `image: postgres:18`, `environment: POSTGRES_USER, POSTGRES_PASSWORD, POSTGRES_DB` from `.env`, `volumes: [pgdata:/var/lib/postgresql/data]` (check the Postgres 18 image's data directory convention and use what it documents), `healthcheck: pg_isready -U $$POSTGRES_USER -d $$POSTGRES_DB` every 5 s, `networks: [mars-frontend]` only, `restart: unless-stopped`, no published port.
- [ ] `services.orchestrator`: `build: ./orchestrator`, `image: mars-orchestrator:latest`, `env_file: .env`, `environment` overriding `DATABASE_URL=postgres://${POSTGRES_USER}:${POSTGRES_PASSWORD}@postgres:5432/${POSTGRES_DB}`, `DATA_DIR=/data`, `DOCKER_HOST=unix:///run/engine.sock`; `volumes: ["${DATA_DIR_HOST}:/data", "${ENGINE_SOCKET_HOST}:/run/engine.sock"]`; `read_only: true` with `tmpfs: ["/tmp:mode=1777"]`; `healthcheck: ["CMD", "mars-orchestrator", "healthcheck"]` interval 10 s, timeout 5 s, retries 6, start_period 30 s (migrations and the startup probe run first); `depends_on: postgres: condition: service_healthy`; `networks: mars-frontend: {}`, `mars-sessions: { aliases: [orchestrator] }`; `stop_grace_period: 30s` (≥ `STOP_GRACE_SECS` default 20 plus margin); `restart: unless-stopped`; **no** `ports` (neither 7000 nor 7001 is published).
- [ ] `services.nginx`: `build: { context: ., dockerfile: nginx/Dockerfile }`, `image: mars-nginx:latest`, `environment: ORCHESTRATOR_HOST=orchestrator, API_PORT=${API_PORT:-7000}`, `ports: ["${HTTP_PORT:-8080}:80"]`, `depends_on: orchestrator: condition: service_healthy`, `networks: [mars-frontend]`, `restart: unless-stopped`.
- [ ] `networks`: `mars-frontend: { name: mars-frontend }`, `mars-sessions: { name: mars-sessions, internal: true }`. `mars-egress` is **not** declared (the orchestrator creates it at startup; declaring it would make `compose down` try to remove a network that running session containers use). A comment explains this and that `SESSION_NETWORK_INTERNAL` must stay `mars-sessions` if the compose network name is kept. `volumes: pgdata: {}`.
- [ ] `/compose.podman.yml` contains only `services.orchestrator.userns_mode: keep-id`; `/compose.docker.yml` contains only `services.orchestrator.user: "1000:1000"`. Neither file declares anything else.
- [ ] `.env.example` gains, with README-quoting comments: `ENGINE_SOCKET_HOST=/run/user/1000/podman/podman.sock` (host path of the engine socket, bind-mounted into the orchestrator; Docker: `/var/run/docker.sock`), `HTTP_PORT=8080` (host port nginx publishes), `COMPOSE_FILE=compose.yml:compose.podman.yml` (with a comment giving the Docker value `compose.yml:compose.docker.yml`). The `.env.example` round-trip unit test from the config task (its hard-coded expected key list) is updated to include the three names, and `Config` does **not** read them (same treatment as `POSTGRES_*`).
- [ ] `README.md` "Configuration" table gains rows for `ENGINE_SOCKET_HOST`, `HTTP_PORT` and `COMPOSE_FILE` marked "compose only", and the `DOCKER_HOST` row says compose overrides it inside the container to the mounted socket path while the `.env` value is what `docker compose` itself and a host-run orchestrator use.
- [ ] `README.md` "Start" states: choose the engine by `COMPOSE_FILE` in `.env` (or pass `-f compose.yml -f compose.podman.yml`), then `podman-compose up -d` / `docker compose up -d`; first start builds both images; nginx listens on `HTTP_PORT`; TLS is terminated in front of nginx by the operator (host reverse proxy or load balancer) and `PUBLIC_URL` must be the https URL users open so cookies are `Secure`. "Running it" → "Prerequisites" adds `git` is not needed on the host (the image ships it) and that `DATA_DIR_HOST` must exist before first start.
- [ ] `podman-compose -f compose.yml -f compose.podman.yml config` and `docker compose -f compose.yml -f compose.docker.yml config` both succeed with `.env.example` copied to `.env` (fake values); `docker compose config` with the Podman override fails only on `userns_mode` (expected, documented).
- [ ] New ADR `docs/decisions/0032-compose-engine-variants-as-override-files.md` (under 300 words): options considered — one compose file with commented-out lines, two complete compose files, override files selected by `COMPOSE_FILE`; decision — override files; consequences — one source of truth for services, `COMPOSE_FILE` in `.env`, the `-f` fallback if a compose implementation ignores `COMPOSE_FILE` from `.env`. Add the row to `docs/decisions/README.md`.

## Implementation notes
- Files: `/compose.yml`, `/compose.podman.yml`, `/compose.docker.yml`, `/.env.example`, `README.md`, `docs/decisions/0032-...md`, `docs/decisions/README.md`, `orchestrator/src/prelude/config.rs` (only the `.env.example` key-list test).
- Socket path inside the container is fixed (`/run/engine.sock`) so `DOCKER_HOST` inside compose is engine-independent; the host side varies through `ENGINE_SOCKET_HOST`. Under `read_only: true`, `/run` must still accept the mount point: bind mounts over read-only root work because the mount is created by the engine, not by the process; verify on both engines.
- Under Podman `keep-id`, the orchestrator runs as the host user's uid inside the container (Podman sets the container user to the host uid unless `--user` is given); the Dockerfile task made the image usable by any uid. Under Docker, `user: "1000:1000"` matches the uid contract.
- DNS for `MCP_URL`'s default `http://orchestrator:7001/mcp`: session containers are created by the orchestrator on `mars-sessions` (not by compose), so they resolve `orchestrator` through the network's DNS from the alias set above; both Docker's embedded DNS and Podman's netavark/aardvark resolve network aliases. If a compose implementation drops aliases on `internal` networks, `MCP_URL=http://mars-orchestrator:7001/mcp` with `container_name: mars-orchestrator` is the fallback; record which one worked in the walkthrough tasks.
- `env_file: .env` passes `COMPOSE_FILE`, `ENGINE_SOCKET_HOST`, `HTTP_PORT` and `POSTGRES_*` into the orchestrator environment as well; they are harmless (`Config` ignores unknown variables). Do not try to filter them.
- SELinux hosts (Fedora): the `/data` bind may need `:z`; do not add it by default (Arch/Debian have no SELinux and `z` relabels the whole tree); document as a note in "Podman setup" only if the walkthrough hits it.
- `stop_grace_period` applies to compose stopping the orchestrator; the orchestrator itself does not stop session containers on shutdown ("Restarting the orchestrator does not intentionally stop running containers").

## Edge cases
- `compose down` while sessions run: `mars-sessions` cannot be removed while session containers are attached; `compose down` reports an error for the network but the services are gone. Document one line in "Operating notes": stop or delete sessions first, or ignore the network warning.
- `DATA_DIR_HOST` missing or not owned by the service user (Podman) / uid 1000 (Docker): the orchestrator's startup probe fails and the container restarts in a loop; the README "Start" paragraph tells the operator to check `compose logs orchestrator` for `startup probe failed; refusing to start`.
- `HTTP_PORT` below 1024 under rootless Podman fails to bind unless `net.ipv4.ip_unprivileged_port_start` is lowered; keep 8080 as the default and mention the sysctl in one sentence.
- `POSTGRES_PASSWORD` containing URL-reserved characters breaks the interpolated `DATABASE_URL`; the README row says to use URL-safe characters (or set `DATABASE_URL` explicitly in `.env`, which compose's `environment:` would override — so the rule is: characters only).
- First `up` builds two images; a build failure leaves nothing running, which is fine; `compose build` is documented as the explicit rebuild command after pulling changes.

## Testing
- `compose config` on both tools as above; `podman-compose up -d` on a Podman host with the stub image present reaches `GET http://localhost:8080/api/health` → 200 and `compose ps` shows all three services healthy (the full walkthrough is a separate task).
- From the host: `curl -s localhost:7000/api/health` and `localhost:7001/mcp` both fail to connect (no published ports).
- `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes after the `.env.example` key-list test update.

## Documentation
- `README.md` "Configuration", "Start", "Prerequisites", "Operating notes" (one line); `.env.example`; ADR 0032 and the ADR index; all in the same commit.

## Assumes from other epics
- "Repository scaffolding, tooling and CI": `.env.example`, `Config::from_env()` with its `.env.example` round-trip test, `GET /api/health`.
- "Container engine adapter": startup creates `mars-sessions`/`mars-egress` if missing, tolerates a pre-existing `mars-sessions` declared `internal: true`, runs the startup probe from `SESSION_IMAGE_DEFAULT`, and the probe failure log line `startup probe failed; refusing to start`.
- "Session container images": the default session image exists locally or is pullable before first start.