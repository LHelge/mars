---
id: v4ry6
title: "Decide whether the claude session image moves from node:22-bookworm-slim to trixie"
status: open
priority: P3
created: "2026-09-19T18:14:09.490596655Z"
updated: "2026-09-19T18:14:09.490596655Z"
tags:
  - images
  - infra
---

## Summary
The orchestrator image moved to `debian:trixie-slim` in epic 5czwa (task n3kps) because bookworm is ageing and its git 2.39.5 broke `checkout --end-of-options`. `images/claude/Dockerfile` is still `FROM node:22-bookworm-slim`. Nothing in Mars runs the orchestrator's git commands inside the session image (only the agent uses git there), so this was left out of scope; this task decides whether to move it for base-image freshness and consistency.

## Acceptance criteria
- [ ] Either `images/claude/Dockerfile` moves to the trixie variant of the node image (the Images workflow and `images/smoke-test.sh` pass on Docker and Podman, the `agent` uid 1000 contract and the entrypoint still hold, `README.md` "Session image" updated if it names the base), or the decision to stay on bookworm is recorded where the base is documented.

## Documents
`ARCHITECTURE.md` "Session image", "Uid contract"; `README.md` "Session image".

Follow-up of n3kps (epic 5czwa).