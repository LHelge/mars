---
id: zzr7k
title: Define and implement managed session-image updates for future launches
status: done
priority: P1
created: "2026-09-22T07:32:19.052082Z"
updated: "2026-09-22T22:19:56.043661221Z"
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
## Decision and evidence (2026-09-23)

Mechanism: managed local aliases. `mars-session-claude:latest` and `mars-session-claude-dev:latest` are moved by `deploy/bin/session-images set <base> <dev>` (shipped in the bundle) to the release's digests after the digest pulls and before the orchestrator is replaced. The step is reverted with the previous release's images on an automatic rollback. `SESSION_IMAGE_DEFAULT` keeps its default, the dev alias. No orchestrator or schema change: the launcher already re-reads the profile on every launch, resume and retry, and pulls only when the image is absent. Rejected alternative: rewriting `agent_profiles.image` to digests per release. That needs a migration or API sweep, silently changes rows users edited, and makes rollback a data change.

`scripts/release/test.sh` (Podman, throwaway images under a test prefix) proves: an absent image is refused before either alias moves; a new container from the alias gets release A and then B after the switch; the container created on A keeps A; a custom image and its container are untouched; `get` reports B; moving the aliases back makes new containers A again. The retention half ("do not prune images of running containers or retained releases") belongs to the updater's retention (hk4xs), and ARCHITECTURE.md "Retention" says so.
