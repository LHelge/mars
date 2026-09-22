---
id: zkpz7
title: "Manual: enable automatic main updates and verify upgrade and reboot recovery"
status: open
priority: P1
created: "2026-09-22T07:33:57.847651Z"
updated: "2026-09-22T07:48:54.961751Z"
tags:
  - deployment
  - manual
  - operator
depends_on:
  - sswjj
  - yjhys
  - "2kane"
parent: "2uqww"
assignee: LHelge
---

Owner: Linus (manual server work).

After initial-install and restore verification, enable the user update timer with the documented five-minute interval. Confirm a subsequent successfully tested main release is picked up, deployed and recorded; observe an existing live session across the orchestrator restart and verify a new launch uses the managed new session image. Confirm unchanged checks do not restart services, pause/pin prevents upgrades, and resume restores tracking. Use scratch/disposable verification for injected failures rather than intentionally breaking production.

Reboot the server and verify user linger, podman.socket, stack startup, persistent data, timer and authentication work without interactive login; verify the installed release can start without registry access using the documented safe test. Check actionable failure status/notification configuration and know how to stop the timer and run manual recovery. Reboot testing is distinct from orchestrator-only restart: do not assume agent processes survive a host reboot.

Acceptance: record observed installed revisions, update/reboot results and operator commands for status, logs, pause/pin/resume and recovery. Only complete the epic after this succeeds. References: user systemd/task runbook; README.md 'Podman setup', 'Operating notes'; ARCHITECTURE.md 'Restart procedure'.