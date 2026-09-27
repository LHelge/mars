---
id: mpaam
title: A deferred or failed backup names its cause in the updater's status and the journal
status: open
priority: P2
created: "2026-09-24T17:31:42.458881340Z"
updated: "2026-09-24T17:31:42.458881340Z"
tags:
  - deployment
  - implementation
parent: "2uqww"
---

Owner: implementation. Discovered while enabling automatic updates (zkpz7) on 2026-09-24.

When the pre-deploy backup failed, `mars-deploy status` and the journal said only "the pre-deploy backup failed". The cause, `mars-backup: MARS_BACKUP_AGE_RECIPIENTS is set but age is not installed`, appeared nowhere: `cmd_apply` discards the backup's stdout and its stderr line did not reach `journalctl --user -u mars-deploy` either, and the nightly `mars-backup.service` journal shows only systemd's own "status=1/FAILURE" lines with no output from the script. Find out why the script's stderr is missing from the journal (fd inheritance through `flock -o`, the `9>&-` redirections, or `set -e` dying somewhere that prints nothing) and fix it.

Then make the reason travel: the updater captures `mars-backup`'s last stderr line and records it in `.attempted.reason` and `.last_check`, so `status` reads "the pre-deploy backup failed: MARS_BACKUP_AGE_RECIPIENTS is set but age is not installed". Never a value from the environment file, as today.

Acceptance: a transition test with a backup that fails in preflight shows the cause in `status` and in the run's output; `scripts/release/test-backup.sh` asserts the preflight message reaches stderr under `flock -o`. References: README.md "Automatic deployments" (Diagnostics); `deploy/bin/mars-deploy` (`cmd_apply`, `deferred`), `deploy/bin/mars-backup`, `deploy/systemd/mars-backup.service`.