---
id: dsyc6
title: Re-run the deployment walkthrough on a fresh host with a non-1000 service uid and check the session view in the browser
status: open
priority: P2
created: "2026-09-19T17:06:22.486307016Z"
updated: "2026-09-19T17:06:22.486307016Z"
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