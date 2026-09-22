---
id: duexz
title: Package a production Compose deployment using immutable images
status: open
priority: P1
created: "2026-09-22T07:32:16.949877Z"
updated: "2026-09-22T07:32:16.949877Z"
tags:
  - deployment
  - implementation
depends_on:
  - tke9r
parent: "2uqww"
---

Owner: implementation.

Add a production Compose override/bundle that uses the manifest's image digests and never builds on the server. Preserve compose.yml and compose.podman.yml contracts: rootless keep-id, socket, absolute DATA_DIR_HOST, networks and DNS aliases, read-only orchestrator filesystem, health checks and shutdown grace. Use an explicit stable Compose project name so versioned deployment directories reuse the same PostgreSQL volume and network identities. Keep operator .env/secrets and persistent state outside release directories; generated release values must not overwrite unrelated operator settings. Pin PostgreSQL independently and exclude it from routine application updates.

Acceptance: render with supported Compose implementations; prove an initial install and switching release directories keep the same database/data/network identities; missing variables and unsupported config fail before replacing services; explicit -f arguments work with podman-compose's .env behavior; default local-development builds remain usable. Installation instructions cover service user, prerequisites, directories, TLS boundary and credentials without requiring a build toolchain. Update README.md 'Running it' and 'Configuration', .env.example, and relevant architecture container contracts.