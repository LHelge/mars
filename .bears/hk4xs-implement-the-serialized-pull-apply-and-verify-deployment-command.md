---
id: hk4xs
title: Implement the serialized pull, apply and verify deployment command
status: open
priority: P1
created: "2026-09-22T07:32:23.870456Z"
updated: "2026-09-22T07:32:23.870456Z"
tags:
  - deployment
  - implementation
depends_on:
  - qeesg
  - duexz
  - zzr7k
  - grkfj
parent: "2uqww"
---

Owner: implementation.

Implement one idempotent command that resolves the promoted manifest once, validates it, acquires an exclusive lock, stages the immutable bundle and all required images, validates config, creates the required backup, and applies the deployment. Avoid concurrent old/new orchestrators. Gracefully replace the orchestrator, wait for startup/migrations/recovery readiness, then recreate/reload nginx so its upstream resolves the replacement. Preserve PostgreSQL, session containers, data and networks; never use broad compose down or destructive pruning for updates.

Acceptance: wait with bounded timeouts and check API health through nginx plus deployment smoke checks before recording success. Persist desired/attempted/current/previous release identities and diagnostic state; expose dry-run/status, deploy-specific-release, pause/pin/resume, and an explicit compatible rollback command. Same revision is a no-op; failed download/backup leaves the running release untouched; mid-update interruption and failed health have deterministic recovery; timer retries do not repeatedly restart a known failing revision. Never automatically restore the DB. Verify bundle compatibility and report errors without credentials. References: deployment contract; README.md 'Operating notes'; ARCHITECTURE.md 'Restart procedure'; SPEC.md 'Health'; scripts/verify-deployment.sh.