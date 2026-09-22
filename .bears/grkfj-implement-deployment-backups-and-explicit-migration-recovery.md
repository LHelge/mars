---
id: grkfj
title: Implement deployment backups and explicit migration recovery
status: in_progress
priority: P1
created: "2026-09-22T07:32:21.240546Z"
updated: "2026-09-22T22:23:44.092104261Z"
tags:
  - deployment
  - implementation
depends_on:
  - tke9r
  - duexz
parent: "2uqww"
assignee: claude
attempts: 1
---

Owner: implementation.

Provide backup hooks/commands and a recovery runbook for PostgreSQL, persistent Mars data and secrets master keys. Require a successful fresh database backup before starting a new orchestrator that may migrate the schema. Distinguish the pre-upgrade database dump from a fully recoverable instance backup: define how filesystem/transcript/git data and database state are made consistent, including quiescing writers when a coordinated backup/restore needs it. Keep off-host copies, retention and encryption configurable and keep credentials out of logs.

Acceptance: backup failure aborts before service replacement; backup metadata records release/schema identity; recover on an isolated scratch installation and verify data plus secret decryption. Explain migration-compatible image rollback versus database restoration and potential loss of writes since the backup. Unknown/incompatible migrations fail into a documented operator recovery path; no unattended down-migration or database restore. Document pausing updates/agents during restoration and retaining necessary image/bundle/key versions. References: ARCHITECTURE.md 'Storage', 'Durability and recovery', secrets keyring; README.md 'Configuration', 'Operating notes'; docs/data-model.md. CLI and README instructions must be directly usable by the manual backup task.
## Decisions and evidence (2026-09-23)

Chosen with Linus: backups go to a local directory, `age`-encrypted when configured, and an operator hook syncs them off the host. Each deploy takes a db-only dump; a nightly full set is the consistent restore point. `deploy/bin/mars-backup` (`db`, `full`, `list`) ships in the bundle, reads `MARS_BACKUP_*` and the database credentials from `mars.env` through `deploy/lib/envfile.sh` (never sourced), and prints no value. Pieces:
- A full set stops only the orchestrator, and the stop is always undone.
- `podman unshare tar` archives the data directory; shared directories are excluded by default.
- The environment file (and so the master keys) goes into a set only when it is encrypted.
- Metadata records the release, the applied migrations, the PostgreSQL version and a checksum per file.
- Sets are written under a `.partial-` name and renamed when complete; retention is per kind; exit 3 means the hook failed and the set was kept.

`scripts/release/test-backup.sh` (Podman, real postgres:18, stand-in orchestrator, age), 25 checks, in the Deploy workflow's release-scripts job:
- db set written, both migrations recorded, checksum matches, restores into a fresh server;
- full set stops and restarts the orchestrator, archives session data, leaves shared directories out;
- a stopped database fails with nothing left behind, and a failure mid-way after the stop still restarts the orchestrator;
- retention per kind; a failing hook exits 3 and keeps the set;
- age: every file encrypted, env included, decrypts and reads back;
- no value printed.

Rehearsal of README "Restoring to a scratch instance" on the published images (Release run 35788985914) as lhelge, project `marsproof`:
- Before the backup: admin password changed, a global secret stored, a marker file written.
- Encrypted `full` backup taken, then the instance destroyed with `down -v` and its root removed.
- Restored with the README's seven steps verbatim. Result: health all true, login with the password from before the backup, secret listed, and PATCH rename of the secret 200 (decrypt plus re-encrypt, so the master keys in the set work). Marker restored, `list` reads the set. Torn down.

Nightly scheduling is left to the lifecycle units (2kane), and the updater wiring of the pre-deploy dump to hk4xs; both are referenced in README and ARCHITECTURE.
