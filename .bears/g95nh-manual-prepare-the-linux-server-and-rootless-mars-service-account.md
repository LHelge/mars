---
id: g95nh
title: "Manual: prepare the Linux server and rootless Mars service account"
status: done
priority: P1
created: "2026-09-22T07:33:30.392532Z"
updated: "2026-09-22T21:36:41.742614424Z"
tags:
  - deployment
  - manual
  - operator
parent: "2uqww"
assignee: LHelge
---

Owner: Linus (manual server work).

Record the server distribution/version, CPU architecture, service username/uid, intended public hostname and available disk space without recording secrets. Install supported Podman, podman-compose and the small runtime utilities listed by the deployment runbook. Create a dedicated unprivileged Mars service user with subordinate uid/gid ranges, a real login session, linger and the user podman.socket. Prefer a non-1000 uid when practical so the existing fresh-host follow-up can reuse evidence.

Create service-owned installation/state/data directories using absolute paths. Check container registry outbound access and firewall/TLS topology; the application should be reachable through the intended proxy while database/API/MCP ports remain private. Verify the runtime socket path for this user and rootless keep-id operation. On an SELinux host, follow measured labeling requirements rather than disabling enforcement.

Acceptance: record non-secret OS/architecture/paths/uid and prerequisite results in this task; the service can use Podman after logging out and has writable persistent storage. References: README.md 'Prerequisites', 'Podman setup', 'Operating notes'; ARCHITECTURE.md 'Uid contract', 'Networks'; existing dsyc6 (reuse applicable evidence, including distro-specific limits).

## Evidence (2026-09-22, reported by Linus, cross-checked from the host)

- Host: Arch Linux, kernel 7.2.6-arch2-1, x86_64 (matches `linux/amd64` in the contract), LAN address 10.10.1.50 on `ens18`. **This is the same machine as the development host** (see follow-up task below).
- Service user: `mars`, uid/gid 1002 (non-1000), home `/home/mars`; subuid and subgid `231072:65536`; linger `yes`.
- Podman 6.1.2 rootless (`Host.Security.Rootless = true`), podman-compose 1.6.0, jq 1.8.2, flock (util-linux 2.42.3). SELinux: not present.
- User socket: `podman.socket` active at `/run/user/1002/podman/podman.sock` (this is `ENGINE_SOCKET_HOST`).
- keep-id: `podman run --userns=keep-id:uid=1000,gid=1000 alpine id` as `mars` gives uid=1000 gid=1000, which is the session uid contract. The passwd name it showed was `lhelge`, which is cosmetic; the orchestrator's startup probe checks the file-ownership side on the first start.
- Directories: `/srv/mars/{data,releases,state}`, owner `mars:mars`, mode `0700`.
- Disk: `/` is 491G with 175G free (63 % used).
- Registry egress: `curl -sI https://ghcr.io/v2/` gives `HTTP/2 405`, so GHCR is reachable.
- TLS topology: public DNS `mars.home.lhelge.se` points at a Traefik reverse proxy on another host, which terminates TLS and forwards to `10.10.1.50:8080`. So `PUBLIC_URL=https://mars.home.lhelge.se`, and `HTTP_PORT` cannot be loopback-only: it has to bind the LAN address. The compose deployment publishes no API/MCP/Postgres ports.
- Not yet checked: host firewall limiting 8080 to the Traefik host.

dsyc6: this gives non-1000-uid evidence for keep-id and the socket path (`/run/user/1002/...`, confirming the README's `$XDG_RUNTIME_DIR` wording). It is **not** a fresh host (warm Podman, same machine as development), so dsyc6's fresh-host criterion stays open.