---
id: mxryx
title: Move the session images, frontend CI and the nginx build stage from Node 22 to Node 24
status: done
priority: P2
created: "2026-09-25T12:05:20.194162185Z"
updated: "2026-09-25T12:20:19.506664044Z"
attempts: 1
---

Discovered while bumping the CLI (yf5zf): 2.1.282 ships a native binary with its own runtime, so the base image's Node is now only the agents' development toolchain. Node 22 is maintenance LTS; move to Node 24 (active LTS) in `images/claude` (`node:24-bookworm-slim`), and align `frontend.yml`, `e2e.yml` and `nginx/Dockerfile` so agents build Mars on the Node CI uses.

## References
- `ARCHITECTURE.md` "Session image" (the dev layer's Node bullet); `README.md` "Session image" / the nginx image paragraph

## Acceptance
- [ ] Base and dev images build on Node 24; `corepack`, `pnpm` and `npm install -g` work as the agent user; smoke test passes
- [ ] CI workflows and the nginx build stage use Node 24; the frontend chain passes on Node 24
- [ ] Documents name Node 24