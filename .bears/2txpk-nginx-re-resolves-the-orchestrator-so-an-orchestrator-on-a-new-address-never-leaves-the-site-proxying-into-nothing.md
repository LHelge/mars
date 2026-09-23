---
id: "2txpk"
title: nginx re-resolves the orchestrator, so an orchestrator on a new address never leaves the site proxying into nothing
status: open
priority: P2
created: "2026-09-23T12:44:38.907959618Z"
updated: "2026-09-23T12:44:38.907959618Z"
tags:
  - deployment
  - implementation
  - nginx
parent: "2uqww"
---

Owner: implementation. Discovered in 2kane.

nginx resolves `${ORCHESTRATOR_HOST}` once, when it loads its configuration. On a Podman network a container that is stopped and started comes back on another address: measured on Podman 6.1.2, 10.89.2.2 before and 10.89.2.5 after, with another container started in between. From then on nginx proxies to the old address, and requests hang rather than getting a 502. The deployment tooling works around it:
- the updater recreates nginx after every orchestrator replacement (hk4xs);
- `mars-backup full` restarts nginx after starting the orchestrator again (2kane).

Anything else that restarts the orchestrator container is not covered, most importantly its own `restart: unless-stopped` after a crash, or a manual `podman restart`.

Fix at the source: nginx resolves the upstream at request time through Podman's DNS, e.g. the official image's `NGINX_ENTRYPOINT_LOCAL_RESOLVERS` to template a `resolver` from `/etc/resolv.conf`, plus `proxy_pass` through a variable, with a short `valid=`. Keep in mind:
- **Host-run mode** (`compose.hostrun.yml`) points nginx at `host.containers.internal`, which comes from `/etc/hosts` and which a `resolver` does not read; that mode must keep working.
- **`proxy_pass` with a variable** changes URI handling; the `/api`, `/ws` and SSE locations must pass exactly what they pass today.
- **The CSP and header assertions** in `.github/workflows/deploy.yml` must still hold.

Acceptance: stop and start the orchestrator container alone, and `GET /api/health` through nginx answers again once it is healthy, without touching nginx; asserted in `scripts/verify-deployment.sh` or the Deploy workflow. The workarounds can then go, or stay as harmless. Update ARCHITECTURE.md "Components" (nginx) and README.md accordingly.