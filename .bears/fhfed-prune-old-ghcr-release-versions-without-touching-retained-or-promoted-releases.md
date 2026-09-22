---
id: fhfed
title: Prune old GHCR release versions without touching retained or promoted releases
status: open
priority: P3
created: "2026-09-22T21:44:55.194465800Z"
updated: "2026-09-22T21:44:55.194465800Z"
tags:
  - deployment
  - implementation
  - ci
depends_on:
  - qeesg
parent: "2uqww"
---

Owner: implementation. Discovered in qeesg.

qeesg publishes every tested main commit (`sha-<commit>` and `inputs-<key>` tags on five packages) and deletes nothing, so the retention rule of ARCHITECTURE.md, "Server deployment", "Retention", holds trivially for now while package storage grows. Implement the pruning half: a scheduled workflow, separate from Release, that deletes only versions not referenced by any bundle promoted in the last 90 days and never touches the bundle tagged `main` or anything it references. Because an image reused under a later commit keeps its original version (and so its original `created_at`), "referenced" has to be read from bundle manifests, not from version age.

Acceptance: dry-run mode that lists what would go; a test over a recorded package listing; minimal `packages: write` only in that workflow; README.md "CI" row. References: ARCHITECTURE.md "Server deployment" (Artifacts, Retention); `.github/workflows/release.yml`.