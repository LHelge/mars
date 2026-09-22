---
id: "9nbnd"
title: "Manual: configure GitHub publishing permissions and server registry access"
status: open
priority: P1
created: "2026-09-22T07:33:36.392186Z"
updated: "2026-09-22T07:48:41.863106Z"
tags:
  - deployment
  - manual
  - operator
depends_on:
  - tke9r
  - qeesg
  - g95nh
parent: "2uqww"
assignee: LHelge
---

Owner: Linus (manual GitHub/server account work).

Apply the package visibility/linkage and workflow permissions documented by the publishing task. Configure required main checks/branch protection where available so the promotion policy is enforceable; verify only trusted main workflows can publish/promote. Confirm the first successful CI run publishes a complete deployment matching the server CPU architecture. Do not create a production self-hosted Actions runner.

If packages are private, provision the minimum read-only package access required by the service user and store registry authentication in an explicit persistent auth file usable by noninteractive user systemd services; do not rely on a login-only or /run auth file. If public anonymous pulls suffice, document that no token is needed. Confirm the updater can fetch both the pointer/bundle and images with its documented authentication mechanism.

Acceptance: service-user noninteractive retrieval succeeds; credentials survive logout/reboot as intended, have minimum permissions and are absent from the repo/task/logs; record package names/visibility and supported platform, never tokens. References: publishing task; README.md deployment installation/registry instructions.