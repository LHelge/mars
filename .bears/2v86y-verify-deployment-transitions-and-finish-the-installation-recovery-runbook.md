---
id: "2v86y"
title: Verify deployment transitions and finish the installation/recovery runbook
status: open
priority: P1
created: "2026-09-22T07:32:28.449657Z"
updated: "2026-09-22T07:32:28.449657Z"
tags:
  - deployment
  - implementation
depends_on:
  - hk4xs
  - "2kane"
parent: "2uqww"
---

Owner: implementation.

Add meaningful automated deployment transition coverage on disposable rootless Podman infrastructure using fake credentials and stub sessions. Cover clean install, A-to-B update, live-session adoption, nginx API routing, subsequent launches on the new session image, stable database/data identity, no-op/pin behavior, concurrent/out-of-order candidates, failed pull/backup/health, migration incompatibility and interruption recovery. Verify user-unit boot/linger behavior where the test environment supports systemd; explicitly reserve real-host evidence for the manual tasks. Reuse scripts/verify-deployment.sh and existing CI rather than duplicating those suites.

Acceptance: tests demonstrate success and failure contracts; migration fixtures prove rollback is not attempted on incompatible schema; runbook gives exact first-install, credentials, backups, restore, timer enablement, pause/pin and diagnostics commands. README, configuration examples, architecture rules and ADR agree with final implementation. Cross-reference existing dsyc6 and reuse fresh-host/non-1000-uid/browser evidence when available; do not claim its separate SELinux acceptance is satisfied without measurement. References: README.md 'Running it', 'Development'; ARCHITECTURE.md 'Engine adapter', 'Restart procedure'; SPEC.md 'Health'; dsyc6.