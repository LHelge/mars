---
id: "2kane"
title: Add user systemd lifecycle, update timer and failure reporting
status: open
priority: P1
created: "2026-09-22T07:32:26.269023Z"
updated: "2026-09-22T07:32:26.269023Z"
tags:
  - deployment
  - implementation
depends_on:
  - hk4xs
parent: "2uqww"
---

Owner: implementation.

Provide installable user systemd units for boot-time stack reconciliation and a five-minute deployment timer under the rootless Mars service account. Use the updater command as the single owner of deployment behavior. Ensure socket/network ordering, login linger, consistent project/env paths, bounded timeouts, and mutual exclusion between boot/manual/timer runs. Reboot must recover the installed release even if the registry is unavailable; upgrades can retry later. Boot behavior must respect a pinned release and paused updates.

Acceptance: installer is idempotent, prints concrete paths and commands, and does not enable unattended updates before the operator's verification steps; services work without an interactive login and recover after reboot. Document journal/status, start/stop timer, pause/pin/resume and updater upgrades. Persist actionable failure status and provide an optional operator-configured failure notification hook without embedding credentials or choosing a recipient. No external notification is sent as part of implementation. References: README.md 'Podman setup', 'Operating notes'; existing deploy/mars-orchestrator.service is a host-run fallback, not the lifecycle unit for this container deployment.