---
id: a8gbx
title: "Manual: configure Mars secrets, TLS and persistent backup storage"
status: open
priority: P1
created: "2026-09-22T07:33:41.875899Z"
updated: "2026-09-22T07:48:44.924615Z"
tags:
  - deployment
  - manual
  - operator
depends_on:
  - g95nh
  - duexz
  - grkfj
parent: "2uqww"
assignee: LHelge
---

Owner: Linus (manual server work).

Using the production installation instructions, create a restricted-permission operator environment file outside release directories. Set PUBLIC_URL, stable JWT_SECRET, SECRETS_MASTER_KEYS, database credentials, absolute data paths and actual rootless socket path. Keep master keys recoverable in a separate secure location. Set proxy/TLS and DNS for the intended hostname; if the TLS proxy runs on this host, bind Mars HTTP to loopback. Keep bootstrap access restricted until the administrator password has been changed.

Provision the backup destination and off-host encrypted storage/credentials, retention and permissions needed by the backup commands; store secrets locally in the documented protected files. Choose a failure notification destination or deliberately use the documented local-status mechanism. No credentials belong in Bears.

Acceptance: configuration validation/rendering passes without starting an exposed bootstrap instance; TLS routing and private-port boundaries are ready; backup destination is writable with available space; record non-secret locations and choices. References: README.md 'Configuration', 'Start', 'Operating notes'; production bundle/install instructions and backup runbook.