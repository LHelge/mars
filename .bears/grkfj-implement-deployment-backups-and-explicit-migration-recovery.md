---
id: grkfj
title: Implement deployment backups and explicit migration recovery
status: open
priority: P1
created: "2026-09-22T07:32:21.240546Z"
updated: "2026-09-22T07:32:21.240546Z"
tags:
  - deployment
  - implementation
depends_on:
  - tke9r
  - duexz
parent: "2uqww"
---

Owner: implementation.

Provide backup hooks/commands and a recovery runbook for PostgreSQL, persistent Mars data and secrets master keys. Require a successful fresh database backup before starting a new orchestrator that may migrate the schema. Distinguish the pre-upgrade database dump from a fully recoverable instance backup: define how filesystem/transcript/git data and database state are made consistent, including quiescing writers when a coordinated backup/restore needs it. Keep off-host copies, retention and encryption configurable and keep credentials out of logs.

Acceptance: backup failure aborts before service replacement; backup metadata records release/schema identity; recover on an isolated scratch installation and verify data plus secret decryption. Explain migration-compatible image rollback versus database restoration and potential loss of writes since the backup. Unknown/incompatible migrations fail into a documented operator recovery path; no unattended down-migration or database restore. Document pausing updates/agents during restoration and retaining necessary image/bundle/key versions. References: ARCHITECTURE.md 'Storage', 'Durability and recovery', secrets keyring; README.md 'Configuration', 'Operating notes'; docs/data-model.md. CLI and README instructions must be directly usable by the manual backup task.