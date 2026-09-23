---
id: ytzd9
title: Skip the Release workflow for pushes to main that touch only tasks and top-level documentation
status: done
priority: P3
created: "2026-09-23T21:20:39.401565287Z"
updated: "2026-09-23T21:20:53.680462727Z"
tags:
  - infra
  - ci
  - docs
attempts: 1
---

Release runs on every push to main (ARCHITECTURE.md, "Server deployment", "Promotion"), so pushing a Bears task or a documentation edit runs all six suites and republishes a bundle whose content hasn't changed.

Add `paths-ignore` to Release's `push` trigger for `.bears/**`, `.bears.yml`, `docs/**` and root-level `*.md` (the glob does not match `/`, so `frontend/src/help/*.md`, `frontend/tests/README.md` and the orchestrator's embedded prompt templates are still covered). No build input, bundle file or test reads these paths. A push that touches only them starts no run, so nothing is published and `mars-deploy:main` stays on the last tested commit, which ships the same content. The rule that every suite runs unconditionally on any run stays as it is; the change is only which pushes start a run. Update README.md ("CI workflows"), ARCHITECTURE.md ("Promotion") and the header comment of release.yml.