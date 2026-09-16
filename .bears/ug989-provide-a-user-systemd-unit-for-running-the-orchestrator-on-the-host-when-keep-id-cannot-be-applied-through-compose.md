---
id: ug989
title: Provide a user systemd unit for running the orchestrator on the host when keep-id cannot be applied through compose
status: open
priority: P3
created: "2026-09-16T20:45:06.272041833Z"
updated: "2026-09-16T20:45:06.272041833Z"
tags:
  - infra
  - docs
depends_on:
  - sgg2e
parent: "5czwa"
---

## Summary
`README.md` "Podman setup" promises a fallback: "If the compatibility API cannot apply `keep-id` to the orchestrator container, it can run as a plain user systemd service." Ship that fallback as a documented unit file `deploy/mars-orchestrator.service` plus a matching `compose.hostrun.yml` override that removes the `orchestrator` service's dependency from nginx and points nginx at the host, so an operator on a Podman whose compose implementation drops `userns_mode` still gets a supported deployment without inventing one.

## Documents
- `README.md` "Podman setup" (the fallback sentence; "Session containers still require `keep-id:uid=1000,gid=1000` support and must pass the startup probe"), "Development" → "Running locally" → "Orchestrator" (`DATA_DIR` equals `DATA_DIR_HOST` on the host; `MCP_URL=http://host.containers.internal:7001/mcp`; `SESSION_EXTRA_HOSTS=host.containers.internal:host-gateway`), "Configuration".
- `ARCHITECTURE.md` "Session container specification" → "Development on the host" (same variables), "Networks" (the orchestrator creates both session networks; on the host it is not attached to `mars-sessions`, so sessions reach it through the host gateway).
- ADR 0004 (rootless Podman through the compat API).

## Acceptance criteria
- [ ] `deploy/mars-orchestrator.service` is a **user** unit (`~/.config/systemd/user/`): `[Service] Type=simple`, `EnvironmentFile=%h/mars/.env`, `Environment=DATA_DIR=%h/mars/data` is **not** hard-coded — instead the unit comments say `.env` must set `DATA_DIR` and `DATA_DIR_HOST` to the same absolute host path, `DOCKER_HOST=unix:///run/user/%U/podman/podman.sock` (can also come from `.env`), `MCP_URL=http://host.containers.internal:7001/mcp` and `SESSION_EXTRA_HOSTS=host.containers.internal:host-gateway`; `ExecStart=%h/mars/bin/mars-orchestrator`, `Restart=on-failure`, `RestartSec=5`, `KillSignal=SIGTERM`, `TimeoutStopSec=40`, `[Install] WantedBy=default.target`; `ProtectSystem=strict` with `ReadWritePaths=` for the data directory and `PrivateTmp=yes`.
- [ ] `compose.hostrun.yml` override: `services.orchestrator.profiles: ["disabled"]` (so it is not started), `services.nginx.depends_on: {}` cleared (compose cannot delete a `depends_on` in an override; use `!reset` where supported or document that the hostrun deployment uses `podman-compose up -d postgres nginx` explicitly), `services.nginx.environment.ORCHESTRATOR_HOST=host.containers.internal`, `services.nginx.extra_hosts: ["host.containers.internal:host-gateway"]`, and `services.postgres.ports: ["127.0.0.1:5432:5432"]` so the host-run orchestrator reaches Postgres on `localhost`.
- [ ] `README.md` "Podman setup" replaces the one-sentence promise with a short "Running the orchestrator on the host" paragraph: build or download the binary (`cargo build --release` produces `orchestrator/target/release/mars-orchestrator`), copy it and `.env`, `systemctl --user enable --now mars-orchestrator`, start `postgres` and `nginx` with the hostrun override, and the `DATABASE_URL` for `.env` in this mode (`postgres://…@127.0.0.1:5432/…`). It states the trade-off: the orchestrator is not in a container, so the read-only filesystem and image minimalism no longer apply (systemd hardening partially replaces them).
- [ ] The hostrun stack passes `scripts/verify-deployment.sh` with `API_PORT`/`MCP_PORT` closed on the loopback exception documented in the script (host-run means 7000/7001 are open on the host by design; the script gets an `HOSTRUN=1` switch that inverts check 2 and skips the `mars-sessions` alias check).

## Implementation notes
- Files: `deploy/mars-orchestrator.service`, `compose.hostrun.yml`, `README.md`, `scripts/verify-deployment.sh` (`HOSTRUN=1`).
- The MCP listener on the host binds `0.0.0.0:7001`; on a host with a public interface that is exposed. The README paragraph must say to firewall 7001 to the Podman bridge (or bind it to the bridge address once the orchestrator supports a bind address; not in v1), because "the host does not publish its port" no longer holds.
- `%h`, `%U` specifiers are standard systemd; `loginctl enable-linger` from "Podman setup" already applies.
- Keep the unit free of secrets; everything comes from `.env` (mode 0600, owned by the service user).

## Edge cases
- Podman's `host.containers.internal` is added automatically by Podman 4.x+ for containers with a gateway; on `internal: true` networks there is no gateway, so session containers reach the host only through the egress network's gateway — `SESSION_EXTRA_HOSTS` with `host-gateway` covers it; verify that a stub session can call MCP in this mode.
- `ProtectSystem=strict` blocks writes to `/tmp` unless `PrivateTmp=yes`; the git wrapper writes a temporary config file, so `PrivateTmp` is required.
- A `TimeoutStopSec` shorter than `STOP_GRACE_SECS` would SIGKILL the orchestrator mid-shutdown; keep 40 s ≥ 20 s + margin.

## Testing
- On a rootless Podman host: enable the unit, start `postgres` and `nginx` with the hostrun override, log in through nginx, run a stub session, run `HOSTRUN=1 scripts/verify-deployment.sh`; record in the PR.
- `systemd-analyze --user verify deploy/mars-orchestrator.service` clean.

## Documentation
- `README.md` "Podman setup" paragraph, same commit. No ADR: this implements the fallback the README already states.

## Assumes from other epics
- "Container engine adapter": `SESSION_EXTRA_HOSTS` and `MCP_URL` handling for host-run development already works, since this mode is the documented development mode with a unit around it.
- "Session container images": the stub image for the smoke session.