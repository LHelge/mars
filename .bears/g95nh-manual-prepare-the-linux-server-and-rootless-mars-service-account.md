---
id: g95nh
title: "Manual: prepare the Linux server and rootless Mars service account"
status: open
priority: P1
created: "2026-09-22T07:33:30.392532Z"
updated: "2026-09-22T07:48:39.573254Z"
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