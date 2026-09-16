---
id: rmu9n
title: Verify the README "Running it" walkthrough on a fresh rootless Podman host and write back drift
status: open
priority: P1
created: "2026-09-16T20:45:51.896648547Z"
updated: "2026-09-16T20:45:51.896648547Z"
tags:
  - infra
  - docs
  - tests
depends_on:
  - sgg2e
  - t93cj
parent: "5czwa"
---

## Summary
Execute `README.md` "Running it" verbatim on a fresh Linux host (or a clean VM/user) with rootless Podman 5+, from "Prerequisites" through "Start", and prove the epic's first acceptance criterion: the login page is reached through nginx, the forced first-login password change completes, and a session on the stub image runs and replays its transcript. Every sentence in the README that was wrong, missing or out of order is corrected in the same commit; findings about `keep-id` through `podman-compose` and the `orchestrator` alias on the internal network are written back into the documents that state them.

## Documents
- `README.md` "Running it" in full: "Prerequisites", "Podman setup", "Configuration", "Start" (incl. `admin`/`changeme`, first-login password change, invite, create project, launch session), "Operating notes" (only the lines touched by findings).
- `ARCHITECTURE.md` "Networks", "Uid contract", "Engine adapter" → "Startup probe" (probe from the default session image; "A failed probe is a fatal startup error with the reason in the log").
- `ARCHITECTURE.md` "Session image" (stub: "emits a `system`/`init` line, then replays a fixture transcript"); `SPEC.md` "Authentication" (must_change_password gate), "Test-only routes" are **not** used (release build, no `integration-tests` feature).
- ADR 0004 (rootless Podman is the target), ADR 0024 (fixed bootstrap credentials).
- Epic acceptance criteria 1 to 3.

## Acceptance criteria
- [ ] Host state recorded in the PR: distribution, kernel, `podman --version`, `podman-compose --version` (or `podman compose` provider), service user uid (deliberately **not** 1000 for at least one run, to exercise `keep-id` with a non-1000 uid), `DATA_DIR_HOST` path and ownership.
- [ ] Following "Podman setup": `loginctl enable-linger`, `systemctl --user enable --now podman.socket`, the printed socket path equals what `.env` gets as `ENGINE_SOCKET_HOST` and `DOCKER_HOST`; if the README's `DOCKER_HOST` row example (`unix:///run/user/1000/podman/podman.sock`) does not match the printed path for a non-1000 uid, the row is reworded to use `$XDG_RUNTIME_DIR`.
- [ ] Following "Configuration" and "Start" with `COMPOSE_FILE=compose.yml:compose.podman.yml` in `.env`: `podman-compose up -d` builds both images, all three services become healthy (`podman-compose ps`), `compose logs orchestrator` shows `engine connected` with `engine_kind = podman`, `session networks ready`, and no `startup probe failed`. If `podman-compose` ignores `COMPOSE_FILE` from `.env`, the README "Start" command is changed to the explicit `-f compose.yml -f compose.podman.yml` form and ADR 0032's consequence line is updated.
- [ ] `podman inspect` of the orchestrator container shows the user namespace mapping (`keep-id`) and the process uid equals the service user's uid; files created under `DATA_DIR_HOST/tmp` by the probe were owned by the service user. If `userns_mode: keep-id` was **not** applied by `podman-compose` (README already anticipates this), the walkthrough switches to the host-run fallback and the finding is recorded in "Podman setup" with the exact `podman-compose` version; the P3 systemd task becomes P1 in that case (update its priority and say why in the PR).
- [ ] Browser flow at `PUBLIC_URL` (through nginx on `HTTP_PORT`): login `admin`/`changeme`, forced password change succeeds and the dashboard loads; create a project from a local bare repository or a public GitHub URL, wait for `ready`; create a profile (or edit the default) using `mars-session-stub:latest`; launch a conversational session; the replayed transcript appears live in the session view; send a message; stop the session (`parked`).
- [ ] `scripts/verify-deployment.sh` passes (health, host and network isolation, query-free logs); its output is pasted in the PR with the canary token visible as proof.
- [ ] `podman-compose down` then `up -d` again: the admin's new password still works (Postgres volume persisted), the project is still `ready` (mirror on `DATA_DIR_HOST`), the parked session can be resumed.
- [ ] Every README deviation found is fixed in the same commit; the PR description lists each command run with its outcome as a checklist. `docs/open-questions.md` is unchanged unless item 8 (`keep-id` through the compat API) is confirmed here before the engine epic writes it back; if so, coordinate with that epic's write-back task rather than deleting the entry twice.

## Implementation notes
- Files: `README.md`, possibly `ARCHITECTURE.md` "Networks"/"Uid contract", `compose*.yml` only for fixes discovered (a fix to compose belongs here if small; a redesign goes back to the compose task as a new linked task), `docs/decisions/0032-*.md` if the `COMPOSE_FILE` consequence changes.
- Use `mars-session-stub:latest` built from `images/stub` per the README "Session image" instructions on the same host before `up -d`; set `SESSION_IMAGE_DEFAULT=mars-session-stub:latest` in `.env` for the walkthrough so the startup probe does not need the claude image or credentials (record this substitution; the README default stays the claude image).
- Record the exact `MCP_URL` that worked: default `http://orchestrator:7001/mcp` (network alias) or the `mars-orchestrator` fallback from the compose task's notes, and make `.env.example`/README match.
- For a non-1000 service uid, watch `git` inside the orchestrator: a `dubious ownership` error on the mirror means the uid contract is broken somewhere; that is a real bug to file against the Git or Engine epic, not to paper over with `safe.directory`.
- Keep the host reproducible: a throwaway VM (`podman machine` on Linux is not equivalent; use a real Linux VM or a fresh user) so "fresh host" is honest.

## Edge cases
- SELinux-enforcing hosts (Fedora): the `/data` bind may fail with permission denied inside the container; if hit, add `:z` to the compose bind with a comment, and a sentence in "Podman setup".
- `podman.socket` under `systemctl --user` requires the `XDG_RUNTIME_DIR` of the service user; `sudo -u` shells without a login session do not have it; the README "Podman setup" comment about linger already exists, but add the `machinectl shell` / `ssh` hint if the walkthrough trips on it.
- `HTTP_PORT` 8080 already used: document `HTTP_PORT` override in one sentence if not already there.
- A stub session that starts but never shows events points at the `mcp.json`/network or the transcript tail, not at packaging; file the bug against the right epic and note it here.

## Testing
- The walkthrough is the test; `scripts/verify-deployment.sh` is the automated part. No cargo/npm chains unless a code fix was needed, in which case the relevant chain must pass.

## Documentation
- This task is the write-back: `README.md` "Running it" and any `ARCHITECTURE.md` sentence proven wrong, same commit.

## Assumes from other epics
- "Session container images": `images/stub` builds and replays a fixture.
- "Session lifecycle", "Real-time delivery", "Frontend foundation", "Frontend project and session views", "Projects, agent profiles and shared directories", "Authentication": the login, password-change, project, profile and session flows exist end to end (the epic depends on the sessions and frontend-foundation epics; the session view is from the frontend sessions epic and is required here for the transcript check — if it is not merged yet, verify the session through `GET /api/sessions/{id}/events` instead and say so).
- "Container engine adapter": startup probe, network creation, `engine connected` log line.