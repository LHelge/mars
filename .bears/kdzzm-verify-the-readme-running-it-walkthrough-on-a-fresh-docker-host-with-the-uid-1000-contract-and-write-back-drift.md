---
id: kdzzm
title: Verify the README "Running it" walkthrough on a fresh Docker host with the uid-1000 contract and write back drift
status: open
priority: P1
created: "2026-09-16T20:46:30.244077570Z"
updated: "2026-09-16T20:46:30.244077570Z"
tags:
  - infra
  - docs
  - tests
depends_on:
  - sgg2e
  - t93cj
  - rmu9n
parent: "5czwa"
---

## Summary
Repeat the "Running it" walkthrough on a fresh Linux host with Docker 24+ and `docker compose`, using the Docker override (`user: "1000:1000"`, `/var/run/docker.sock`), and prove the same three epic acceptance criteria on the second engine: login page through nginx, forced password change, a stub-image session replaying live, query-free stream logs, MCP unreachable from the host. Docker has no user-namespace mapping, so this run specifically validates the uid contract (orchestrator as uid 1000, `DATA_DIR_HOST` owned by uid 1000, session containers writing files the orchestrator can read) and the `docker` group requirement. It runs after the Podman walkthrough so README fixes land once.

## Documents
- `README.md` "Running it": "Prerequisites" (Docker 24+), the Docker paragraph under "Podman setup" ("use the daemon's socket `unix:///var/run/docker.sock` and a user in the `docker` group ... the orchestrator service runs as uid 1000 (`user: "1000:1000"`) and the data directory must be owned by uid 1000"), "Configuration", "Start".
- `ARCHITECTURE.md` "Uid contract" (Docker: "the orchestrator itself must run as uid 1000 ... `DATA_DIR_HOST` must be owned by uid 1000. The startup probe verifies the outcome"), "Engine adapter" table (`UsernsMode` ignored on Docker; `ExtraHosts` `host-gateway`), "Networks".
- ADR 0004 ("Docker must keep working; the only deployment difference is `DOCKER_HOST`" — now also the override file and `ENGINE_SOCKET_HOST`; if the walkthrough shows more differences, the ADR's consequence is condensed, not the decision changed).
- Epic acceptance criteria 1 to 3.

## Acceptance criteria
- [ ] Host state recorded in the PR: distribution, `docker --version`, `docker compose version`, the service user's uid and `docker` group membership, `DATA_DIR_HOST` ownership (`stat -c '%u:%g'` = `1000:1000`).
- [ ] `.env` with `COMPOSE_FILE=compose.yml:compose.docker.yml`, `ENGINE_SOCKET_HOST=/var/run/docker.sock`, `DOCKER_HOST=unix:///var/run/docker.sock`: `docker compose up -d` builds both images, all services healthy, `compose logs orchestrator` shows `engine connected` with `engine_kind = docker` and a passing probe. `docker inspect` of the orchestrator shows `"User": "1000:1000"` and no `UsernsMode`.
- [ ] Negative check, recorded: with `DATA_DIR_HOST` deliberately owned by another uid, the orchestrator logs `startup probe failed; refusing to start` and compose shows it restarting; the README "Start" or Docker paragraph names this log line as the thing to look for (add it if the Podman walkthrough did not).
- [ ] The Docker socket is root-equivalent (ADR 0004 context); the README Docker paragraph states plainly that on Docker the orchestrator container holds a root-equivalent socket and the "unprivileged" claim is weaker than under rootless Podman — add this sentence if missing (it is the honest consequence and belongs next to the `docker` group requirement).
- [ ] Browser flow identical to the Podman task: login, forced password change, project `ready`, stub profile, conversational session with live transcript, message, stop, `down`/`up` persistence check.
- [ ] `scripts/verify-deployment.sh` with `ENGINE=docker` passes; output in the PR. Check 4 (alias `orchestrator` on `mars-sessions`) confirms Docker's embedded DNS resolves the compose alias for containers the orchestrator created outside compose; if not, `MCP_URL` guidance for Docker is added to the README `MCP_URL` row.
- [ ] `read_only: true` under Docker: the orchestrator runs with a read-only root and `/tmp` tmpfs without errors; if Docker rejects the bind of the socket under a read-only root, the fix goes to `compose.yml` with a comment.
- [ ] Session containers created by the orchestrator on Docker: files under `DATA_DIR_HOST/sessions/<sid>/` are owned by `1000:1000` on the host after the session ran (the `agent` user), and the orchestrator reads the transcript without ownership errors.
- [ ] README deviations fixed in the same commit; PR lists each command and outcome.

## Implementation notes
- Files: `README.md`, possibly `compose.yml`/`compose.docker.yml` (small fixes), `ARCHITECTURE.md` "Uid contract" (only if the observed behaviour differs), ADR 0004 consequence line (condense only).
- Use `mars-session-stub:latest` with `SESSION_IMAGE_DEFAULT` pointed at it, as in the Podman task; build it with `docker build -t mars-session-stub:latest images/stub`.
- `docker compose` reads `DOCKER_HOST` from `.env` for its own connection; confirm that with the default local socket nothing surprising happens (it is the same socket either way). Note the behaviour in the `DOCKER_HOST` README row if it surprised you.
- Do not run Docker in rootless mode for this task (that is a third configuration; README supports rootful Docker with the `docker` group). If time allows, a one-paragraph note on what breaks under rootless Docker (uid mapping like Podman without `keep-id`) can go into "Operating notes", but it is not required.

## Edge cases
- Docker Desktop on macOS/Windows is out of scope for the walkthrough (README says Linux host); the `ARCHITECTURE.md` macOS note about shared paths stays as is.
- `docker compose` treats `userns_mode` in the Podman override as an error; the README must make clear that the Docker override is the one to select, or `COMPOSE_FILE` defaults confuse Docker users: consider whether `.env.example` should default to the Docker pair instead of the Podman pair; decide based on which failure mode is clearer and record it in ADR 0032's consequences (Podman remains the target engine per ADR 0004, so the default stays Podman unless the Docker error is truly opaque).
- Host firewall (ufw) interplay with Docker's iptables rules can expose `HTTP_PORT` on all interfaces; note in "Operating notes" that `HTTP_PORT` binds `0.0.0.0` unless `HTTP_PORT=127.0.0.1:8080` style binding is used (compose accepts `ip:port:port`; verify and document the form).

## Testing
- The walkthrough plus `ENGINE=docker scripts/verify-deployment.sh`; the negative probe check; the ownership check with `stat`.
- Any code fix requires the corresponding `cargo`/`npm` chain to pass.

## Documentation
- This task is the write-back: `README.md` Docker paragraph, "Configuration" rows touched, "Operating notes" if a sentence was added; ADR 0004/0032 consequence lines only if the facts changed.

## Assumes from other epics
- Same as the Podman walkthrough task: stub image, full session and frontend flows, engine adapter probe and logging.
- "Container engine adapter" engine tests already passed on Docker in CI, so any failure here is packaging (compose, uid, socket) rather than adapter behaviour; file adapter bugs against that epic.