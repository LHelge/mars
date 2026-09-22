---
id: duexz
title: Package a production Compose deployment using immutable images
status: done
priority: P1
created: "2026-09-22T07:32:16.949877Z"
updated: "2026-09-22T22:12:33.098505123Z"
tags:
  - deployment
  - implementation
depends_on:
  - tke9r
parent: "2uqww"
assignee: claude
attempts: 1
---

Owner: implementation.

Add a production Compose override/bundle that uses the manifest's image digests and never builds on the server. Preserve compose.yml and compose.podman.yml contracts: rootless keep-id, socket, absolute DATA_DIR_HOST, networks and DNS aliases, read-only orchestrator filesystem, health checks and shutdown grace. Use an explicit stable Compose project name so versioned deployment directories reuse the same PostgreSQL volume and network identities. Keep operator .env/secrets and persistent state outside release directories; generated release values must not overwrite unrelated operator settings. Pin PostgreSQL independently and exclude it from routine application updates.

Acceptance: render with supported Compose implementations; prove an initial install and switching release directories keep the same database/data/network identities; missing variables and unsupported config fail before replacing services; explicit -f arguments work with podman-compose's .env behavior; default local-development builds remain usable. Installation instructions cover service user, prerequisites, directories, TLS boundary and credentials without requiring a build toolchain. Update README.md 'Running it' and 'Configuration', .env.example, and relevant architecture container contracts.
## Evidence (2026-09-22)

podman-compose 1.6.0 behaviour, measured on this host before writing the docs: a top-level `name:` fixes the project; `${VAR:?msg}` makes `config` and `up` fail before any container is touched; a `.env` symlink in the release directory serves both interpolation and the orchestrator's `env_file`; `up -d` from a second directory recreates only the services whose configuration changed; `build: !reset null` crashes podman-compose (so the release override keeps `build:`, and a missing image fails with `Dockerfile not found`).

Install and switch on real published images (Release run 35788985914, bundle `sha256:78751bec…`). As lhelge, on a scratch root with project `marsproof` so the dev stack's `mars_pgdata` stayed untouched: two release directories built by `make-bundle.sh` from this branch over the published digests, `.env` linked to one `mars.env`, and `bin/check-env` passing in both (one warning for the http `PUBLIC_URL`). `up -d` from r1: health all true, orchestrator uid = service uid, admin password changed. `up -d` from r2: the `pgdata` volume, `mars-frontend` and `mars-sessions` network IDs and the Postgres container ID were unchanged, and only the orchestrator container was recreated. The new password logs in, `changeme` gets 401, the data-directory marker is still there, and `verify-deployment.sh` from r2 passes all checks. Torn down afterwards. The local development flow (`podman-compose -f compose.yml -f compose.podman.yml` with `build:`) is unchanged: `.env.example` sets every newly required variable and the default project was already `mars`, from the directory name.
