---
id: "5czwa"
title: "Deployment packaging: compose, nginx and orchestrator image"
type: epic
status: done
priority: P2
created: "2026-09-16T20:15:04.556266020Z"
updated: "2026-09-19T18:14:10.519438038Z"
tags:
  - infra
depends_on:
  - s52qg
  - "2f5u2"
---

## Scope

Make `podman-compose up -d` / `docker compose up -d` work as `README.md` describes.

- Orchestrator `Dockerfile`: multi-stage build, unprivileged user, read-only root filesystem where the engine allows, `git` and the binary only.
- `nginx/`: Dockerfile serving the built frontend; `nginx.conf` proxying `/api/` and `/ws/` with `proxy_http_version 1.1`, upgrade headers and a 1 h read timeout on `/ws/`, `proxy_buffering off`/`proxy_cache off`/`Connection ''` on `/tasks/stream`, a log format without `$request`/`$args` on both (token in query), `index.html` fallback for client routes, MCP port never proxied.
- `compose.yml`: the four services with `mars-frontend`, `mars-sessions` (`internal: true`) and `mars-egress` networks, Postgres 18 with a volume, orchestrator with the engine socket mount, `DATA_DIR_HOST` bind, `userns_mode: keep-id` (Podman) or `user: "1000:1000"` (Docker) documented as variants, health checks on `/api/health`.
- Verify the "Running it" walkthrough on rootless Podman and on Docker; fix README drift found on the way.

## Documents

`README.md` "Deployment shape", "Running it", "Podman setup", "Configuration", "Start"; `ARCHITECTURE.md` "Components", "Networks", "Frontend architecture" (nginx requirements), "Uid contract".

## Acceptance criteria

- [ ] A fresh host following `README.md` reaches the login page, completes the first-login password change and runs a stub-image session on both engines.
- [ ] nginx access logs for `/ws/` and `/tasks/stream` contain no query strings.
- [ ] The MCP listener is unreachable from the host and from the frontend network.

## Out of scope

Egress allow-lists and sandboxed runtimes as defaults (non-goals).