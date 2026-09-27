---
id: eyxkc
title: check-env verifies the tools a release needs on the user manager's PATH, age included when backups are encrypted
status: open
priority: P2
created: "2026-09-24T17:31:42.429421437Z"
updated: "2026-09-24T17:31:42.429421437Z"
tags:
  - deployment
  - implementation
parent: "2uqww"
---

Owner: implementation. Discovered while enabling automatic updates (zkpz7) on 2026-09-24.

The first production install passed `check-env` and `install-units`, yet the nightly full backup failed at 03:34 the next day and every timer run since deferred with "the pre-deploy backup failed": `mars.env` sets `MARS_BACKUP_AGE_RECIPIENTS` and `age` was not installed. The user manager's PATH is `/usr/local/bin:/usr/bin`, so a per-user install would not have counted either.

`bin/check-env` already validates the environment file against the manifest; extend it to check that every binary the release's scripts call is on the PATH the units run with (`podman`, `podman-compose`, `jq`, `flock`, `systemd-run`, and `age` when `MARS_BACKUP_AGE_RECIPIENTS` is set), naming each missing one. `install-units` runs the same check and refuses, or warns loudly, when one is missing, so a first install cannot enable the nightly backup unit against a host that cannot run it.

Acceptance: a test in `scripts/release/test.sh` over a PATH without `age` and a recipients file set fails with the tool's name; README.md "Installing a published release" names the check beside the prerequisite list. References: README.md "Installing a published release" (the tools line); `deploy/bin/check-env`, `deploy/bin/install-units`, `deploy/bin/mars-backup`.