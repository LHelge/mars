---
id: dgy3e
title: "Curiosity release shape: a static musl binary with a pinned version, built in CI"
status: open
priority: P2
created: "2026-09-21T12:20:17.497966Z"
updated: "2026-09-21T12:20:17.497966Z"
tags:
  - curiosity
  - ci
  - build
depends_on:
  - pfs5r
parent: vj82v
---

## Summary
What makes Curiosity easy to put into any session image — including a project's own toolchain image or a devcontainer — is that it is one static file. Make the build produce that, for the two architectures Mars's images are built for, and make the version something an image tag and a pin test can rely on.

## Acceptance criteria
- [ ] `x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl` release builds succeed with rustls only (no OpenSSL, no native-tls anywhere in `cargo tree`); `ldd` reports a static executable.
- [ ] `curiosity --version` prints exactly the `Cargo.toml` version; the version is bumped by hand and is what `images/curiosity` will tag with (`mars-session-curiosity:<version>`), mirroring `CLAUDE_CODE_VERSION`.
- [ ] `curiosity.yml` builds both targets and uploads them as workflow artifacts; no release publishing.
- [ ] `curiosity/README.md`: how to build it locally on macOS for a Linux container (cross or a builder container), since the session image task needs it.
- [ ] Binary size and a cold-start time (`--version`, and `acp` to `initialize` response) are recorded in the README as a baseline.

## Implementation notes
- Check the architectures in `.github/workflows/images.yml` and match them.

## Testing
- CI is the test; locally, one target built and run inside a `scratch`- or `alpine`-based container.