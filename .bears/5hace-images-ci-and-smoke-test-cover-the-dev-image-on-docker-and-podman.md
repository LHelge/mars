---
id: "5hace"
title: Images CI and smoke test cover the dev image on Docker and Podman
status: done
priority: P2
created: "2026-09-21T10:01:18.377029657Z"
updated: "2026-09-21T11:31:46.493383883Z"
tags:
  - images
  - ci
depends_on:
  - zqe6v
parent: qpshf
attempts: 1
---

## Summary
`.github/workflows/images.yml` lints and builds `images/claude` and `images/stub` and runs `images/smoke-test.sh`. Extend all three steps to `images/claude-dev`, so a broken toolchain layer is a build failure in CI and not a session that cannot compile.

## Documents
- `README.md`, "CI" — the Images row: "build both session images" becomes three; "Session image" — `images/smoke-test.sh` invocation if it gains a variable.
- `images/smoke-test.sh` header comment lists what it proves; add the dev image's lines.

## Acceptance criteria
- [ ] `hadolint` loop in `images.yml` includes `images/claude-dev/Dockerfile`.
- [ ] `build-and-smoke` builds the dev image after the base on both engines, passing `--build-arg BASE_IMAGE=mars-session-claude:ci`, tagged `mars-session-claude-dev:ci`. On Docker with buildx the base must be visible to the dev build (load the base into the daemon, or build the dev image with plain `docker build` as the stub is) — verify rather than assume.
- [ ] The existing retry-once pattern for registry flakiness covers the dev build (rustup and crates downloads).
- [ ] `images/smoke-test.sh` takes `DEV_IMAGE` (skipped with a printed line when unset, so local runs of the old two-image form still work) and proves on a real container, as uid 1000 with the session `HostConfig` shape the script already uses:
  - the contract checks it runs for the claude image hold for the dev image (uid, `HOME`, cwd, entrypoint redirects, pinned CLI);
  - `cargo`, `rustc`, `cargo clippy`, `rustfmt`, `cargo binstall`, `node`, `npm` resolve both for the entrypoint's command and in `/bin/bash -l`;
  - with `/session/home` bind-mounted **empty** (as the launcher does) the toolchain still resolves — the regression this image is most likely to have;
  - `cargo new` + `cargo build --offline` succeeds in `/session/work`;
  - `agent` can create a file under `/opt/rustup` and `/opt/cargo`, and `npm config get prefix` is under `/session/home`.
- [ ] Nothing in the smoke test needs the network beyond the image build.
- [ ] `shellcheck` clean; workflow passes on both engines.

## Implementation notes
- The Images workflow triggers on `images/**`, which already covers the new directory.
- Keep the job's wall-clock in mind: cache the dev layer with the buildx cache the workflow already configures for the claude image, if it does; otherwise accept the cost — this workflow only runs on `images/**`.
- The E2E and Engine workflows use the stub image only and are not touched.