---
id: p7ghm
title: "images/curiosity: the Curiosity session image, tagged by the crate version and pinned against the adapter"
status: open
priority: P1
created: "2026-09-21T12:22:05.529079Z"
updated: "2026-09-21T12:22:05.529079Z"
tags:
  - images
  - curiosity
  - ci
depends_on:
  - dgy3e
  - tj596
  - k5e7m
  - dtq74
  - tt9d9
parent: ddb8s
---

## Summary
A third session image honouring the contract of `ARCHITECTURE.md`, "Session image": user `agent` (uid 1000), `HOME=/session/home`, `git`, the CLI on `PATH`, `/usr/local/bin/mars-entrypoint` making the FIFO and redirecting stdout/stderr to `/session/log/`. The CLI is the static `curiosity` binary, built in a builder stage from `curiosity/`.

## Documents
- `ARCHITECTURE.md`, "Session image": the image, its tag rule and the pin; `README.md`, "Session image" and "Development" (build command); `images/curiosity/VERIFY.md` in the manner of `images/claude/VERIFY.md` — what was observed on real engines.

## Acceptance criteria
- [ ] `images/curiosity/Dockerfile`: multi-stage, build context the repository root or `curiosity/` (say which; `.dockerignore` adjusted), final stage on the same distribution base as `images/claude` (today `node:22-bookworm-slim`; Node stays because the dev layer's Node tooling expects it), tagged `mars-session-curiosity:<version>` and `:latest`. The version line has one greppable spelling (as `ARG CLAUDE_CODE_VERSION=` has), and a unit test in `agent/acp/` asserts it equals the adapter's `CURIOSITY_VERSION` and `curiosity/Cargo.toml`.
- [ ] The entrypoint is shared with the Claude image, not copied a third time if that can be avoided (one source file, `COPY`ed by each Dockerfile); if build contexts make that impossible, a test asserts the copies are identical.
- [ ] A toolchain variant exists, because a session that cannot build the repository is of little use: ADR 0039 layers `images/claude-dev` on a `BASE_IMAGE` argument and adds nothing CLI-specific, so build the same Dockerfile with `BASE_IMAGE=mars-session-curiosity:latest` as `mars-session-curiosity-dev`. If anything in that Dockerfile turns out to assume the Claude base (labels, the version tag derived from `CLAUDE_CODE_VERSION`), make the layer CLI-neutral (rename the directory to `images/dev` or parameterise the tag source) rather than copying it; amend ADR 0039 and `README.md`, "Session image" accordingly.
- [ ] `images/smoke-test.sh` covers the image (and the dev variant through its existing `DEV_IMAGE` checks): contract checks plus `curiosity acp` answering `initialize` through the FIFO.
- [ ] `VERIFY.md` records, on rootless Podman and Docker: `SIGINT`/`SIGTERM` reach PID 1 without `Init` and produce exit 0 / 143; a turn with the mock script; files written under `/session/work` and `CURIOSITY_HOME` have the uid the contract wants.
- [ ] `.github/workflows/images.yml` builds it for the same architectures as the Claude image; `deploy.yml` and the compose files know the image if they enumerate session images.

## Implementation notes
- A published image containing only Mars's own MIT/own-licence binary has none of the redistribution concerns a third-party CLI would have.

## Testing
- `images/smoke-test.sh` locally on one engine; CI on both.