---
id: nnvb2
title: Decide whether the claude session image disables the CLI auto-updater (DISABLE_AUTOUPDATER=1)
status: done
priority: P2
created: "2026-09-17T22:23:21.940740573Z"
updated: "2026-09-18T08:02:36.735153977Z"
tags:
  - images
  - agent
depends_on:
  - "64jkc"
parent: deex5
---

## Summary
`images/claude/Dockerfile` pins `@anthropic-ai/claude-code` to `ARG CLAUDE_CODE_VERSION` and records it in the image labels and tag (`ARCHITECTURE.md` "Session image": the CLI is pinned to a version recorded in the image tag). `/usr/local` is root-owned so an in-place npm update cannot succeed, but the CLI's auto-updater may migrate itself to a local install under `$HOME` (`/session/home`, a writable mount), which would silently diverge from the pinned version. Whether a real session triggers this is unknown: the image task only exercised `claude --version`.

## Documents
- `ARCHITECTURE.md` "Session image", "Claude Code invocation"
- `README.md` "Session image"

## Acceptance criteria
- [ ] The credentialed verification (task h3e43) or the adapter epic's live probe observes whether the pinned CLI attempts an auto-update or a local migration when run as `agent` with `HOME=/session/home`.
- [ ] If it does, `images/claude/Dockerfile` sets `ENV DISABLE_AUTOUPDATER=1` (or the variable the pinned version honours) and `ARCHITECTURE.md` "Session image" gains one sentence saying the image disables self-update so the tag stays truthful.
- [ ] If it does not, this task is closed with the observation recorded in `images/claude/VERIFY.md`.

## Testing
- Manual, credentialed; the same procedure as `images/claude/VERIFY.md`.