---
id: sswjj
title: "Manual: install the first published deployment and verify Mars on the server"
status: done
priority: P1
created: "2026-09-22T07:33:47.306287Z"
updated: "2026-09-23T20:56:08.944349724Z"
tags:
  - deployment
  - manual
  - operator
depends_on:
  - "2v86y"
  - "9nbnd"
  - a8gbx
  - jg2dp
parent: "2uqww"
assignee: LHelge
attempts: 1
---

Owner: Linus (manual server/browser work).

Install the release bundle/updater/user units following the runbook, leave automatic updates disabled, and deploy a specific published tested-main revision using the deployment command. Verify the actual image digests, persistent volumes/data paths, startup health and scripts/verify-deployment.sh. Through the intended HTTPS URL, change the bootstrap admin password, enter model/Git credentials through the supported UI, create a project, launch a session and inspect its live transcript. Confirm the orchestrator uid and data ownership match the rootless service user.

Acceptance: a working first installation with release commit/digests and non-secret verification evidence recorded here; credentials never pasted into this task. Capture any README deviations as linked fix tasks and resolve deployment-blocking ones before enabling updates. Cross-reference dsyc6 for fresh-host/non-1000/browser evidence; it remains open if its separate Fedora/SELinux checks were not performed. References: README.md 'Running it'; deployment runbook; scripts/verify-deployment.sh; ARCHITECTURE.md 'Uid contract', 'Networks'.

## Evidence (2026-09-23)
- **Release:** commit `0877ed1b4d61cac17722318988130ba59f0a16b9`, bundle `sha256:95dd70d452760fd5a45d8ec5589b81c9139df860fa3fe293744b27e6b3d8a036`. It is the first release with `keep-id:uid=1000,gid=1000` and `in_pod: false` (2v86y). It was installed by the README's "The first install" steps as `mars` (uid 1002), via `sudo machinectl shell mars@`, and applied with `mars-deploy deploy <digest>`, so it is pinned. Automatic updates are off.
- **Host tools:** Podman 6.1.2 and podman-compose 1.6.0 in `/usr/bin`. The user manager's PATH is `/usr/local/bin:/usr/bin`.
- **Preflight:** `check-env` against this manifest passed all six checks. `/srv/mars` and `data`, `releases`, `state`, `backups` are `mars:mars 0700`; `mars.env` is `0600`.
- **Images running:** orchestrator `ghcr.io/lhelge/mars-orchestrator@sha256:1a9b0655928046f019c47ee27f1af543118d42cb4e0a63c9d78d2c5e3a016e61`, nginx `ghcr.io/lhelge/mars-nginx@sha256:bce0990a8c2a897d7930c854f068ef056f25112eed5e3e388027a67ebc655a2e`, postgres `docker.io/library/postgres@sha256:0377e72c5289ed2f98cf61b1a9c2db9eb9d300317fe14244492fbc94343b3d04`. All three are healthy and none is in a pod.
- **Session aliases:** `mars-session-claude:latest` is `a3c829dcaac0…` and `mars-session-claude-dev:latest` is `59393e6eb756…`. The manifest's session images are `…session-claude@sha256:d4cbb679…` and `…session-claude-dev@sha256:16ae7e7c…`.
- **uid contract:** the orchestrator's userns is private (keep-id). `podman exec mars_orchestrator_1 id` gives uid=1000(mars), and `podman top … huser` gives 1002. `/srv/mars/data` is `mars:mars 700`, and `data/tmp` is owned by 1002.
- **Storage:** volume `mars_pgdata` is at `/home/mars/.local/share/containers/storage/volumes/mars_pgdata/_data`. The data directory is `/srv/mars/data`. `current` points to `releases/sha256-95dd70d4…`.
- **Health:** `{"orchestrator":true,"database":true,"engine":true}` on `10.10.1.50:8080` and through Traefik at `https://mars.lhelge.se`. `/` returns HTTP/2 200 with the Content-Security-Policy (checked from lhelge). `verify-deployment.sh` ran inside `deploy` and passed, since the deploy applied.
- **Admin:** the bootstrap `admin` was replaced right after the deploy by a custom administrator with a strong password. The credential is recorded nowhere.
- **Reported by Linus, output not pasted:**
  - `install-units` ran, and the updates timer stays disabled.
  - The model and Git credentials were entered through the UI.
  - A project was created, a session launched on the default profile, and its transcript appeared live.
  - "Everything works as expected"; no README deviation was reported.
- **dsyc6:** adds non-1000-uid evidence on a real server: service uid 1002, orchestrator uid 1000 in the container and 1002 on the host, data owned by 1002, `$XDG_RUNTIME_DIR` socket path `/run/user/1002/…`. Not a fresh host; SELinux not measured, so dsyc6 stays open.
- **Next:** yjhys (backup rehearsal; needs the off-host NAS hook from a8gbx first), then zkpz7 (enable the timer).

