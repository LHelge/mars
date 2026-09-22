---
id: zzr7k
title: Define and implement managed session-image updates for future launches
status: in_progress
priority: P1
created: "2026-09-22T07:32:19.052082Z"
updated: "2026-09-22T22:14:59.888602032Z"
tags:
  - deployment
  - implementation
depends_on:
  - tke9r
  - duexz
parent: "2uqww"
assignee: claude
attempts: 1
---

Owner: implementation.

Implement the release contract for session images, accounting for SESSION_IMAGE_DEFAULT, image strings persisted in existing profiles, and the launcher/bootstrap behavior that pulls only when an image is absent locally. Merely changing a mutable registry tag or SESSION_IMAGE_DEFAULT must not leave managed existing profiles silently stale. Choose and document a minimal explicit mechanism (for example managed local aliases moved only after digest pulls, versus explicit tracking policy); custom operator image references must retain their intended pinning.

Acceptance: pre-pull all managed images by release digest before application replacement; startup probe can use the selected default; new launches and documented resume/relaunch cases use the intended release; existing running containers retain their container identity/image/process; custom pinned profiles remain unchanged; failed deployments retain usable previous image mappings. Do not prune images required by running containers or retained releases. Test an existing profile across two deployments, a new profile, a custom image, and rollback/failure. References: README.md 'Configuration'; ARCHITECTURE.md 'Session image', 'Launch sequence', 'Restart procedure'; SPEC.md profile image contract; docs/data-model.md agent_profiles. Update owning docs and schema docs if schema changes.