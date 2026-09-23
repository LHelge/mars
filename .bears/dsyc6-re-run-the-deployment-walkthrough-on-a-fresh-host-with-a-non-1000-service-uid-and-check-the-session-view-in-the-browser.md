---
id: dsyc6
title: Re-run the deployment walkthrough on a fresh host with a non-1000 service uid and check the session view in the browser
status: open
priority: P2
created: "2026-09-19T17:06:22.486307016Z"
updated: "2026-09-23T13:49:50.112757115Z"
tags:
  - infra
  - docs
  - tests
---

## Summary
The Podman walkthrough of the deployment packaging epic (5czwa, task rmu9n) ran on a warm Arch developer host as uid 1000, because that machine has one user, no passwordless sudo and no VM. Three parts of its acceptance criteria are therefore unverified and are collected here.

## Acceptance criteria
- [ ] `README.md` "Running it" is followed verbatim on a genuinely fresh Linux host or VM (no pre-existing images, networks or Podman configuration) with rootless Podman 5+.
- [ ] At least one run uses a service user whose uid is **not** 1000: `userns_mode: keep-id` maps it, `podman exec <orchestrator> id` equals the service uid, files under `DATA_DIR_HOST` are owned by it, `git` reports no `dubious ownership`, and the README's `$XDG_RUNTIME_DIR` wording for `DOCKER_HOST`/`ENGINE_SOCKET_HOST` (changed on inspection in rmu9n, not by measurement) is confirmed. `loginctl enable-linger` without `sudo` is checked on a stock server (polkit-dependent).
- [ ] Once the frontend project and session views exist (on `main` at the time of rmu9n, `/projects`, `/projects/:id` and `/sessions/:id` were `PlaceholderPage`), the browser flow creates the project, launches a stub session and sees the replayed transcript live in the session view; rmu9n verified these steps through the REST API and `GET /api/sessions/{id}/events` instead.
- [ ] On an SELinux-enforcing host (Fedora), whether the `/data` bind needs `:z`; if so, the compose bind and "Podman setup" say so.
- [ ] Every README deviation found is fixed in the same commit.

## Documents
`README.md` "Running it"; `ARCHITECTURE.md` "Uid contract", "Networks"; ADR 0004, ADR 0035.

Follow-up of rmu9n (epic 5czwa); not a child of that epic because it cannot be done on the machines that implement it.

## Cross-reference from 2v86y (2026-09-23)
- The non-1000 criterion's premise changed: with plain `keep-id` the orchestrator, whose image runs as `USER 1000`, became a sub-uid that could not write `/data` for a uid 1001 service user (measured on the GitHub runner, Podman 4.9.3, startup probe EACCES). `compose.podman.yml` now uses `keep-id:uid=1000,gid=1000`, so `podman exec <orchestrator> id` is uid 1000 and `podman top <orchestrator> huser` is the service user; README "Podman setup" says so. Check it in those terms.
- Partial evidence, not this task's acceptance: the Deploy workflow's `transitions` job (`scripts/release/test-transitions.sh`) runs the whole deployment as the runner's uid 1001 with linger and the socket unit; sswjj installs production as `mars` (uid 1002) on the dev host. Neither is a fresh host, and nothing has measured SELinux.
- Podman 4.9 and 5.7 refuse `--userns` inside podman-compose's pod; `compose.podman.yml` sets `x-podman: in_pod: false` (ADR 0048).