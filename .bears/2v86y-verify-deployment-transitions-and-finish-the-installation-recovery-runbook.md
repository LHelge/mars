---
id: "2v86y"
title: Verify deployment transitions and finish the installation/recovery runbook
status: done
priority: P1
created: "2026-09-22T07:32:28.449657Z"
updated: "2026-09-23T14:57:23.840665268Z"
tags:
  - deployment
  - implementation
depends_on:
  - hk4xs
  - "2kane"
parent: "2uqww"
attempts: 1
---

Owner: implementation.

Add meaningful automated deployment transition coverage on disposable rootless Podman infrastructure using fake credentials and stub sessions. Cover clean install, A-to-B update, live-session adoption, nginx API routing, subsequent launches on the new session image, stable database/data identity, no-op/pin behavior, concurrent/out-of-order candidates, failed pull/backup/health, migration incompatibility and interruption recovery. Verify user-unit boot/linger behavior where the test environment supports systemd; explicitly reserve real-host evidence for the manual tasks. Reuse scripts/verify-deployment.sh and existing CI rather than duplicating those suites.

Acceptance: tests demonstrate success and failure contracts; migration fixtures prove rollback is not attempted on incompatible schema; runbook gives exact first-install, credentials, backups, restore, timer enablement, pause/pin and diagnostics commands. README, configuration examples, architecture rules and ADR agree with final implementation. Cross-reference existing dsyc6 and reuse fresh-host/non-1000-uid/browser evidence when available; do not claim its separate SELinux acceptance is satisfied without measurement. References: README.md 'Running it', 'Development'; ARCHITECTURE.md 'Engine adapter', 'Restart procedure'; SPEC.md 'Health'; dsyc6.
## Handoff (2026-09-23, from the session that built tke9r, qeesg, duexz, zzr7k, grkfj, hk4xs, 2kane)

### Where the epic stands
- **Done:**
  - implementation: tke9r contract, qeesg Release workflow, duexz production compose, zzr7k session aliases, grkfj backups, hk4xs `mars-deploy`, 2kane units;
  - manual: g95nh server, jg2dp shared host, a8gbx env/TLS/backup key, 9nbnd registry access.
- **Open:**
  - **2v86y (this task)** blocks **sswjj** (first install), which blocks yjhys (backup rehearsal) and zkpz7 (enable the timer; epic done).
  - Also open: **2txpk** (P2, nginx re-resolving the orchestrator), **fhfed** (P3, GHCR pruning), and dsyc6 (fresh-host walkthrough; cross-reference only).
- Release run **35863351268** (7771fc9) was queued at handoff. It is the **first release whose bundle contains `bin/mars-deploy` and `systemd/`**, and the one sswjj should install. Check that it is green and that `mars-deploy:main` moved: `podman pull -q ghcr.io/lhelge/mars-deploy:main && podman image inspect --format '{{.Digest}}' ghcr.io/lhelge/mars-deploy:main` must equal the digest of the `sha-7771fc9…` tag. The first published release (5ef256b) predates `name: mars` in compose.yml and must never be installed.

### What exists to build on
- `scripts/release/test.sh`, 70+ checks, no containers needed except the Podman-gated parts:
  - promotion decision, bundle assembly with a podman-compose render, manifest validator, check-env;
  - session-image aliases on Podman;
  - unit rendering plus `systemd-analyze --user verify`.
- `scripts/release/test-backup.sh`: 25 checks on real postgres:18.
- Both run in `.github/workflows/deploy.yml`, job `release-scripts`, which Release calls on every main push.
- Manual proofs, all recorded as evidence in hk4xs, 2kane, grkfj and duexz, with harnesses copied to `~/dev/mars-2v86y-handoff/` (outside the repo, not committed). Set `S` to a scratch dir and `mkdir -p "$S"` first; they run as lhelge with project `marsproof`, HTTP on 127.0.0.1:18080, and session aliases under `localhost/marsproof-session-*`, so the dev stack's `mars_pgdata` and dev images are never touched:
  - `updater-proof.sh`: a local `registry:2` on 127.0.0.1:5055 serves bundles built from HEAD (variants A/C/F/X via jq on the manifest; F adds `command: ["no-such-subcommand"]` to the orchestrator in `compose.release.yml`), over the real published images pulled from GHCR (needs `podman login ghcr.io` with read:packages). Nine scenarios: first install, no-op, upgrade, older ignored, pause, rollback, never-healthy then auto-rollback, known failure held, unpullable image deferred.
  - `units-proof.sh`: the same setup plus the units installed into the real user manager with `--env` overrides, and a notify hook logging to a file. Covers start/stop, the deploy unit applying, the backup unit, a failing deploy unit with OnFailure. It removes the units afterwards.
  - `recovery.sh`: encrypted full backup, destroy, restore with the README's seven steps. Needs `$S/bundle-digest` (a published bundle digest) and an `age` binary on PATH, e.g. `$S/age/` from the FiloSottile/age v1.3.2 release.
  - `proof.sh`: the duexz install-and-switch identity check.

### Gaps 2v86y has to close
1. **Automated transition tests on CI**, the acceptance of this task. The manual proofs are the scenario list, and the updater's knobs make it testable: `MARS_ROOT`, `MARS_PROJECT`, `MARS_REGISTRY`, `MARS_HEALTH_TIMEOUT`, plus `SESSION_IMAGES_{BASE,DEV}_ALIAS` and `CONTAINERS_REGISTRIES_CONF`. Open design questions:
   - Where do the images come from on CI? The packages are private; GITHUB_TOKEN with `packages: read` can pull them, or the Deploy job builds the orchestrator/nginx images anyway and a local registry can serve them. The manifest validator only accepts `ghcr.io/lhelge/<name>@sha256:` image refs, so a test registry needs a validator knob, or the images must really live on ghcr.io.
   - Does a GitHub runner have a usable `systemctl --user` (enable-linger for `runner`, XDG_RUNTIME_DIR)? Measure it. If not, the unit path stays manual evidence and the tests run the updater directly.
2. **Migration-added failure path**, never exercised. A candidate whose `schema.migrations` has one more entry than the installed one and that fails health must be marked failed, **left in place**, and held with "manual recovery". The updater decides purely from the manifest lists, so a variant manifest with an extra 14-digit version plus the F-style broken orchestrator is enough; it needs no real migration.
3. **Held rules not yet exercised:** epoch higher than accepted (and `accept-epoch`), a missing required variable, migrations that do not extend the installed ones, a PostgreSQL major mismatch, the wrong platform.
4. **Concurrency:** two `mars-deploy run` at once, the second waiting for the lock; a stale lock must never block, and nothing may hold fd 9 after a run (the proofs check `/proc/<conmon>/fd`).
5. **Runbook completeness** (README "Automatic deployments", "Installing a published release", "Backups and recovery"): exact commands for first install, `install-units`, credentials, backups, restore, enabling the timer, pause/pin, diagnostics. Most already exist; read them as an operator would for sswjj and fix what is missing. dsyc6: reuse the non-1000-uid evidence (the server runs as `mars`, uid 1002) but do not claim its fresh-host or SELinux criteria.

### Measured facts and gotchas (all hit in this epic)
- **podman-compose 1.6.0:**
  - crashes on `build: !reset null` (bundles keep `build:`; a missing image fails with "Dockerfile not found");
  - waits on `depends_on: condition: service_healthy` without any limit, which is why `bring_up` starts one service at a time with `--no-deps` under `timeout 600`;
  - `-p` overrides compose.yml's `name:`;
  - `up -d` recreates only services whose config changed;
  - `up -d --force-recreate --no-deps nginx` touches nginx alone;
  - `${VAR:?msg}` fails `config`.
- **Podman:**
  - a stopped and started container gets a **new IP**, and nginx keeps the old one (2txpk);
  - `podman manifest inspect` fails on single-image manifests, so resolve digests with `podman pull -q` plus `podman image inspect --format '{{.Digest}}'` (no skopeo on the host);
  - containers started from inside a oneshot unit leave rootlessport/conmon in its cgroup and die with it, so every start goes through `systemd-run --user --scope`;
  - child processes inherit the flock fd, so every child gets `9>&-`;
  - Podman 4.9 (CI) does not show a container's pre-restart logs.
- **CI:**
  - runners have shellcheck **0.9.0**, which flags SC2015 where 0.11 does not; check locally with `docker.io/koalaman/shellcheck:v0.9.0`;
  - `buildx imagetools create` wraps a single image in an index unless `--prefer-index=false`;
  - the postgres image's first start runs a temporary server that `pg_isready` already reports ready, so wait for the "init process complete" log line on a first start only;
  - a Release run is serialised (`release-main`); a throwaway branch plus `gh workflow run deploy.yml --ref <branch>` tests the release-scripts job without a Release;
  - the orchestrator test `env_example_carries_exactly_the_readme_variables_and_loads` fails when `.env.example` gains a variable that `README_VARIABLES` in `orchestrator/src/prelude/config.rs` lacks.
- **Local:** the orchestrator library suite needs `DOCKER_HOST=unix:///run/user/1000/podman/podman.sock` for its database tests. `pkill -f <pattern>` kills the shell whose own command line contains the pattern; kill by PID.
- **The dev host is the production server:** the dev host, 192.0.2.50, a VM. Production runs as `mars` (uid 1002) under `/srv/mars`, published on 192.0.2.50:8080 behind Traefik (192.0.2.4) at https://mars.example.com. Never touch `/srv/mars` or the `mars` user's Podman from lhelge; tests use `marsproof`.
- **Linus's working style for this epic:** step by step in the main session, not implement-epic; ask before every push to main; verify on real containers before claiming anything.

## Evidence (2026-09-23)
- `scripts/release/test-transitions.sh`, Deploy workflow job `transitions`: branch run **35871349983** green, 166 checks on Ubuntu 24.04, Podman 4.9.3, runner uid 1001, linger plus `podman.socket`, user units in the real user manager; about 10 min of transitions, 11 min for the job with a warm build cache. Locally on the dev host (Podman 6.1.2, uid 1000, project `marstrans`), 167 checks green in 642 s, including the pod-era release A and the move out of the pod. Nothing was left behind: no containers, pod, units or images.
- Measured on the runner: `sudo loginctl enable-linger runner` starts `user@1001.service`, and `systemd-run --user --scope` works. So the units are CI-tested; only a reboot stays manual.
- Found and fixed by it: Podman 4.9.3 and 5.7.0 refuse `--userns` in podman-compose's pod (`x-podman: in_pod: false`, ADR 0048); plain `keep-id` breaks for a service uid other than 1000 (orchestrator now `keep-id:uid=1000,gid=1000`); `status` did not show the recovery; tools must be on the user manager's PATH.
- sswjj should install the first release **after** this commit: the 7771fc9 release still has plain `keep-id`, which fails for `mars` (uid 1002) on Podman 4.9/5 and is unmeasured on Podman 6 with a non-1000 uid.
- The migration-added case is asserted without a real migration (manifest lists only): failed, left in place, `recovery: manual…`, no rollback attempted, pre-deploy backup present, next run held.
- dsyc6 is cross-referenced but not claimed: this is not a fresh host and nothing measured SELinux.

