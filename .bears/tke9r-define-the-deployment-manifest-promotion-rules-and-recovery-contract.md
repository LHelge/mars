---
id: tke9r
title: Define the deployment manifest, promotion rules and recovery contract
status: in_progress
priority: P1
created: "2026-09-22T07:32:11.833493Z"
updated: "2026-09-22T21:24:17.166847865Z"
tags:
  - deployment
  - implementation
parent: "2uqww"
assignee: claude
attempts: 1
---

Owner: implementation.

Specify the versioned deployment manifest and bundle: source commit, supported platform(s), fully qualified image digests for orchestrator/nginx/Claude/Claude-dev, matching Compose files, configuration compatibility requirements, and migration/rollback policy. Choose a concrete GHCR-compatible publication/discovery format and document how a read-only client retrieves it. The single promoted main pointer must never describe a partial release or regress when an older workflow finishes late. Pin installed bundles to immutable identities; validate downloaded data and never source manifest data as shell code.

Define newest-tested-main semantics, skipped/path-filtered checks, manual pin/pause/resume, same-revision no-op, failure hold/retry, deployment state and image retention. Default to blocking unattended deployment of changes explicitly requiring operator migration/configuration action; never infer schema compatibility merely from image health. Define how the updater and bundle format themselves evolve safely. Keep PostgreSQL outside application promotion.

Acceptance: an implementable contract and sample manifest/bundle with fake values, an ADR explaining timer-based pull deployment versus SSH/runner/image watchers, and corresponding operation/recovery rules in README.md and ARCHITECTURE.md. References: README.md 'Deployment shape', 'Configuration', 'Operating notes'; ARCHITECTURE.md 'Restart procedure', 'Durability and recovery'; docs/data-model.md migration contract.