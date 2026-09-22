---
id: qeesg
title: Publish complete, tested main deployments to GHCR
status: open
priority: P1
created: "2026-09-22T07:32:14.721594Z"
updated: "2026-09-22T07:32:14.721594Z"
tags:
  - deployment
  - implementation
depends_on:
  - tke9r
parent: "2uqww"
---

Owner: implementation.

Build and publish the orchestrator, frontend/nginx, Claude and Claude-dev images for the supported server platforms; build the dev layer against the exact base from this release. Publish immutable image references and the bundle/manifest defined by the deployment contract. Reuse existing CI checks and caches without publishing PR builds. Promote only after required backend/frontend/engine/image/deployment/E2E checks applicable to that exact main commit have succeeded; explicitly handle path-filtered workflows and do not mistake a skipped, cancelled or missing required check for success. Existing .github/workflows/deploy.yml only checks packaging paths and is not sufficient as the promotion gate.

Acceptance: source changes trigger the necessary image rebuilds; all artifacts exist before promotion; concurrent/out-of-order runs cannot regress main; PR/fork workflows cannot publish/promote; the commit and digests are recorded; package retention preserves active and recoverable deployments. Exercise successful, failing and superseded candidates. Set minimal package/content permissions and document any GitHub settings the operator must apply. Update README.md 'Development' CI and deployment documentation. References: .github/workflows/{orchestrator,frontend,engine,images,deploy,e2e}.yml; README.md 'Start'; ARCHITECTURE.md 'Session image'.