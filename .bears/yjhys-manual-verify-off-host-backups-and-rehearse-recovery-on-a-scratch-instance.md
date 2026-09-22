---
id: yjhys
title: "Manual: verify off-host backups and rehearse recovery on a scratch instance"
status: open
priority: P1
created: "2026-09-22T07:33:52.327229Z"
updated: "2026-09-22T07:48:52.479273Z"
tags:
  - deployment
  - manual
  - operator
depends_on:
  - sswjj
  - grkfj
parent: "2uqww"
assignee: LHelge
---

Owner: Linus (manual server work).

With automatic updates still disabled, run the configured coordinated backup procedure and verify off-host retention, encryption and recoverable master keys. Restore to an isolated scratch instance using the recorded release and schema, preventing restored agents from launching real work or contacting production resources. Verify database records, filesystem/git/transcript data and successful decryption using the backed-up key material. Never overwrite the live instance for this rehearsal.

Exercise the documented pause/pin and compatible image rollback path in the scratch environment; confirm an incompatible migration requires explicit recovery and cannot trigger an automatic DB downgrade/restore. Record recovery duration and any data-loss window implied by the backup schedule; fix failed recovery before enabling unattended deployment.

Acceptance: actual backup/restore evidence and non-secret backup locations recorded here, with missing keys/data identified and resolved. References: backup/recovery implementation task; README.md 'Operating notes'; ARCHITECTURE.md 'Storage', 'Durability and recovery'; docs/data-model.md.