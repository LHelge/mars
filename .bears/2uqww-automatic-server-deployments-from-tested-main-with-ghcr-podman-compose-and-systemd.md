---
id: "2uqww"
title: Automatic server deployments from tested main with GHCR, Podman Compose and systemd
type: epic
status: open
priority: P1
created: "2026-09-22T07:30:49.226387Z"
updated: "2026-09-22T07:30:49.226387Z"
tags:
  - infra
  - deployment
  - ci
  - operations
---

Deliver a single-server Mars installation that follows the latest successfully tested main commit through a server-side pull deployment. GitHub-hosted CI publishes immutable application/session images and a versioned deployment bundle, then promotes one complete deployment manifest; a rootless service user's systemd timer reconciles that manifest on the server. Reuse the supported Podman Compose deployment, preserve live session containers, and make backup, migration handling, failure reporting, pinning and recovery explicit.

Repository work and operator work are separate child tasks. Tasks tagged manual/operator and titled 'Manual:' are for Linus to perform on the server or in GitHub; implementation tasks must not claim those steps happened. Track dependencies so automatic updates are enabled only after first-install verification and a recovery rehearsal. This epic is planning only until its tasks are explicitly taken up.

Completion: a tested main deployment is published and installed; a subsequent main deployment arrives through the timer; unchanged/failed candidates cannot disrupt the current deployment; reboot and live-session adoption are verified; backups and recovery have been exercised; Linus has usable pause, pin, status and recovery commands.

Scope: rootless Podman Compose, GHCR, GitHub-hosted builds, a local deployment script and user systemd units. PostgreSQL version upgrades are separately managed. No production self-hosted Actions runner, Watchtower, Kubernetes, zero-downtime promise, or automatic database restore/downgrade. Images/Compose/config must correspond to the same deployment. Do not expose secret values in tasks or logs.

References: README.md 'Deployment shape', 'Running it', 'Configuration', 'Operating notes', 'Development'; ARCHITECTURE.md 'Durability and recovery', 'Restart procedure', 'Storage', 'Networks', 'Engine adapter'; SPEC.md 'Health'; docs/data-model.md; CLAUDE.md documentation rules. Add an ADR for this deployment decision and update the owning documents alongside implementation. Existing fresh-host walkthrough task dsyc6 overlaps validation and should be cross-referenced rather than duplicated or silently closed.