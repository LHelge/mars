---
id: hk4xs
title: Implement the serialized pull, apply and verify deployment command
status: done
priority: P1
created: "2026-09-22T07:32:23.870456Z"
updated: "2026-09-23T12:21:30.233810495Z"
tags:
  - deployment
  - implementation
depends_on:
  - qeesg
  - duexz
  - zzr7k
  - grkfj
parent: "2uqww"
assignee: claude
attempts: 1
---

Owner: implementation.

Implement one idempotent command that resolves the promoted manifest once, validates it, acquires an exclusive lock, stages the immutable bundle and all required images, validates config, creates the required backup, and applies the deployment. Avoid concurrent old/new orchestrators. Gracefully replace the orchestrator, wait for startup/migrations/recovery readiness, then recreate/reload nginx so its upstream resolves the replacement. Preserve PostgreSQL, session containers, data and networks; never use broad compose down or destructive pruning for updates.

Acceptance: wait with bounded timeouts and check API health through nginx plus deployment smoke checks before recording success. Persist desired/attempted/current/previous release identities and diagnostic state; expose dry-run/status, deploy-specific-release, pause/pin/resume, and an explicit compatible rollback command. Same revision is a no-op; failed download/backup leaves the running release untouched; mid-update interruption and failed health have deterministic recovery; timer retries do not repeatedly restart a known failing revision. Never automatically restore the DB. Verify bundle compatibility and report errors without credentials. References: deployment contract; README.md 'Operating notes'; ARCHITECTURE.md 'Restart procedure'; SPEC.md 'Health'; scripts/verify-deployment.sh.
## Evidence (2026-09-23)

`deploy/bin/mars-deploy` (in every bundle, with `bin/validate-manifest`). Commands: `run [--dry-run]`, `status`, `deploy <commit|digest>`, `unpin`, `pause`, `resume`, `retry`, `accept-epoch`, `rollback`, `start`, plus the internal `apply`, run from the candidate's own bundle. Exit 0 applied, nothing to do or held; 1 failed; 75 deferred. `verify-deployment.sh` now accepts `HTTP_PORT=address:port`.

Two real bugs were found by the proof and fixed:
- **Lock leak.** The flock descriptor was inherited by `conmon`/`rootlessport` of containers compose started, so the lock stayed held after the updater exited. Every child now closes fd 9.
- **Unbounded wait.** podman-compose 1.6.0 waits on `depends_on: service_healthy` with no limit, so an orchestrator that never became healthy hung `up -d` for 35 minutes. `bring_up` now starts postgres, orchestrator and nginx one at a time with `--no-deps`; the updater waits against its own bound, and every compose call runs under `timeout 600`.

Proof as lhelge, project `marsproof`: the real published images of 299a805, bundles built from this branch served by a local registry, session aliases under a test name. All nine scenarios passed, with no process holding the lock between runs:
1. First install by hand-extracted bundle and `deploy <digest>`: applied, health all true, `current` linked.
2. No-op: "up to date".
3. Upgrade A→C: orchestrator replaced, postgres container kept. No pre-deploy backup, because the orchestrator image was unchanged.
4. A promoted again (older sequence): ignored.
5. Paused: nothing resolved.
6. `rollback`: previous release applied (same migrations).
7. F, an orchestrator that never becomes healthy: failed after 120 s; no migration added, so the previous release was started again and is healthy; exit 1.
8. F still promoted: held as a known failure, orchestrator untouched.
9. X, an image that cannot be pulled: deferred, exit 75, nothing touched.
- `status` reads as intended, and `state.json` holds no value from `mars.env`.
- Unplanned extra, from the first proof run: a run killed mid-replacement, with `attempted` left at phase `replacing` and no outcome. The installed updater's next `run` marked the release failed ("interrupted during replacement"), started the previous release again, and holds the failed one afterwards.

Not covered here (task 2v86y): the automated transition suite on CI, and the migration-added failure path, which needs a release with a new migration.
