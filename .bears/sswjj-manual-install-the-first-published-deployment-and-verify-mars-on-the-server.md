---
id: sswjj
title: "Manual: install the first published deployment and verify Mars on the server"
status: open
priority: P1
created: "2026-09-22T07:33:47.306287Z"
updated: "2026-09-22T07:48:49.201971Z"
tags:
  - deployment
  - manual
  - operator
depends_on:
  - "2v86y"
  - "9nbnd"
  - a8gbx
parent: "2uqww"
assignee: LHelge
---

Owner: Linus (manual server/browser work).

Install the release bundle/updater/user units following the runbook, leave automatic updates disabled, and deploy a specific published tested-main revision using the deployment command. Verify the actual image digests, persistent volumes/data paths, startup health and scripts/verify-deployment.sh. Through the intended HTTPS URL, change the bootstrap admin password, enter model/Git credentials through the supported UI, create a project, launch a session and inspect its live transcript. Confirm the orchestrator uid and data ownership match the rootless service user.

Acceptance: a working first installation with release commit/digests and non-secret verification evidence recorded here; credentials never pasted into this task. Capture any README deviations as linked fix tasks and resolve deployment-blocking ones before enabling updates. Cross-reference dsyc6 for fresh-host/non-1000/browser evidence; it remains open if its separate Fedora/SELinux checks were not performed. References: README.md 'Running it'; deployment runbook; scripts/verify-deployment.sh; ARCHITECTURE.md 'Uid contract', 'Networks'.