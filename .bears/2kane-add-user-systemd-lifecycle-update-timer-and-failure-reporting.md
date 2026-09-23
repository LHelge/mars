---
id: "2kane"
title: Add user systemd lifecycle, update timer and failure reporting
status: done
priority: P1
created: "2026-09-22T07:32:26.269023Z"
updated: "2026-09-23T12:51:57.719250208Z"
tags:
  - deployment
  - implementation
depends_on:
  - hk4xs
parent: "2uqww"
assignee: claude
attempts: 1
---

Owner: implementation.

Provide installable user systemd units for boot-time stack reconciliation and a five-minute deployment timer under the rootless Mars service account. Use the updater command as the single owner of deployment behavior. Ensure socket/network ordering, login linger, consistent project/env paths, bounded timeouts, and mutual exclusion between boot/manual/timer runs. Reboot must recover the installed release even if the registry is unavailable; upgrades can retry later. Boot behavior must respect a pinned release and paused updates.

Acceptance: installer is idempotent, prints concrete paths and commands, and does not enable unattended updates before the operator's verification steps; services work without an interactive login and recover after reboot. Document journal/status, start/stop timer, pause/pin/resume and updater upgrades. Persist actionable failure status and provide an optional operator-configured failure notification hook without embedding credentials or choosing a recipient. No external notification is sent as part of implementation. References: README.md 'Podman setup', 'Operating notes'; existing deploy/mars-orchestrator.service is a host-run fallback, not the lifecycle unit for this container deployment.
## Evidence (2026-09-23)

Units in `deploy/systemd/`, installed by `bin/install-units` (both in the bundle):

| Unit | Role | Enabled by install-units |
| --- | --- | --- |
| `mars.service` | `mars-deploy start` at boot, `stop` at shutdown; oneshot, RemainAfterExit | yes |
| `mars-deploy.service` + `.timer` | `run` 3 min after boot, then every 5 min; exit 75 counts as success | no (operator, zkpz7) |
| `mars-backup.service` + `.timer` | nightly `full` at 03:30, Persistent, under the updater lock via `flock -o` | yes |
| `mars-failed@.service` | OnFailure of the three; calls the notification hook | — |

`install-units` is idempotent. `--refresh`, which the updater runs after each apply, re-renders the installed units and never changes what is enabled. It warns when linger is off. `mars-deploy` gained `start`/`stop` (`start` takes the lock), `notify`, and deduplicated notification through `MARS_DEPLOY_NOTIFY_HOOK`: applied, failed, held with a new reason, deferral after 12 runs and its end, unit-failed. The variable is in README.md "Configuration", `.env.example` and `README_VARIABLES`.

`scripts/release/test.sh`: the units render with root, project and `--env` and leave no placeholder; `systemd-analyze --user verify` passes over them, stand-in ExecStart binaries included; `units.env` is recorded for `--refresh`.

Live proof under lhelge's real user manager (scratch root, project `marsproof`, published images, a local registry for bundles built from this branch, a notify hook writing a log):
- `install-units` enables mars.service and mars-backup.timer, leaves mars-deploy.timer disabled; a second run changes nothing.
- mars.service stop takes every container down; start brings it back healthy.
- mars-deploy.service with a newer release promoted: `Result=success`, applied, and nginx healthy after the unit ended. A second run: success, no notification.
- mars-backup.service: success, one `full-nightly` set, orchestrator running, and health through nginx again within 90 s.
- mars-deploy.service with a never-healthy release: the unit fails (exit 1), the previous release is healthy again, and OnFailure ran.
- The hook logged exactly four events: applied, applied, failed, unit-failed.
- All units, containers and the scratch root were removed afterwards.

Two real bugs found by the live proof, fixed:
1. **Containers killed after a timer run.** Containers started from inside a oneshot unit left rootlessport (and conmon) in the unit's cgroup, which systemd killed when the run ended, taking nginx's published port with it. The first proof run showed "Result: timeout" and a dead stack. Every container start (compose in mars-deploy, the orchestrator start in mars-backup) now runs under `systemd-run --user --scope`.
2. **The nightly backup broke the site.** A stopped and started container gets a new address (measured 10.89.2.2 → 10.89.2.5), and nginx keeps the old one, so the site hung after every full backup. mars-backup now restarts nginx after the orchestrator. The general fix, nginx re-resolving, is follow-up 2txpk.

Checks: shellcheck 0.9/0.11 clean; release, unit and backup tests pass; `cargo fmt`/clippy clean; library suite 1057/1057 with DOCKER_HOST. Real reboot evidence belongs to zkpz7.
