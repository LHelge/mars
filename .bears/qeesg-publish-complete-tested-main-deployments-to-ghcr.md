---
id: qeesg
title: Publish complete, tested main deployments to GHCR
status: done
priority: P1
created: "2026-09-22T07:32:14.721594Z"
updated: "2026-09-23T10:12:15.549837705Z"
tags:
  - deployment
  - implementation
depends_on:
  - tke9r
parent: "2uqww"
assignee: claude
attempts: 1
---

Owner: implementation.

Build and publish the orchestrator, frontend/nginx, Claude and Claude-dev images for the supported server platforms; build the dev layer against the exact base from this release. Publish immutable image references and the bundle/manifest defined by the deployment contract. Reuse existing CI checks and caches without publishing PR builds. Promote only after required backend/frontend/engine/image/deployment/E2E checks applicable to that exact main commit have succeeded; explicitly handle path-filtered workflows and do not mistake a skipped, cancelled or missing required check for success. Existing .github/workflows/deploy.yml only checks packaging paths and is not sufficient as the promotion gate.

Acceptance: source changes trigger the necessary image rebuilds; all artifacts exist before promotion; concurrent/out-of-order runs cannot regress main; PR/fork workflows cannot publish/promote; the commit and digests are recorded; package retention preserves active and recoverable deployments. Exercise successful, failing and superseded candidates. Set minimal package/content permissions and document any GitHub settings the operator must apply. Update README.md 'Development' CI and deployment documentation. References: .github/workflows/{orchestrator,frontend,engine,images,deploy,e2e}.yml; README.md 'Start'; ARCHITECTURE.md 'Session image'.
## Evidence (2026-09-23)

`.github/workflows/release.yml` calls the six suites without path filters on every push to main; a gate requires every result to be `success`; then publish and promote. Promotion is serialised and forward-only (`scripts/release/promote-decision.sh`, unit-tested for promote, same, older and diverged).

Real runs:
- 35788985914 (5ef256b): all suites green, first publish, promotion "nothing promoted yet". This exposed a bug: `imagetools create` wrapped the bundle in a new image index, so `:main` had a different digest from the bundle `sha256:78751bec…`. Fixed in 6e95314 with `--prefer-index=false`, plus a check that `:main` resolves to exactly the published bundle.
- 35789272042 (f3878e4, another session's push) was queued behind the first run, as the `release-main` concurrency intends. It failed the Images credential grep on prose naming `sk-ant-`, so the gate held it and nothing was published (fixed in 2bd01a1).
- 35791957048, 35825052551, 35836188113 and 35842139039 each failed a suite: a README-variables unit test, shellcheck 0.9 SC2015, and two backup-test races on the runner. Every time the gate failed and publish and promote were skipped, so `:main` stayed on the last good release. These are the failing-candidate evidence.
- 35845704906 (299a805): green. Promoted 5ef256b → 299a805, a strict successor. `:main` and `sha-299a805…` both resolve to `sha256:b52897dd6ee45a22046825a32268f09ac71e17a5bf4f47e7ed1fa6ae8e1971b8`, so the carbon copy is verified by pulling both.

Not yet exercised on a real run: reuse of an unchanged image, because every image's inputs changed in this batch. It is covered by the input-key logic and will show as `reusing …` in the publish log of the first commit that leaves an image's inputs alone. Retention pruning is split out as fhfed. GitHub settings for the operator are in README.md, "Automatic deployments" (task 9nbnd).
