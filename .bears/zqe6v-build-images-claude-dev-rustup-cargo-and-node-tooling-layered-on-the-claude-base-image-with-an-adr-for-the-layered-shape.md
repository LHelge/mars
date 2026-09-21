---
id: zqe6v
title: "Build images/claude-dev: rustup, cargo and Node tooling layered on the claude base image, with an ADR for the layered shape"
status: done
priority: P1
created: "2026-09-21T10:00:44.691569881Z"
updated: "2026-09-21T11:13:12.835631719Z"
tags:
  - images
  - docs
parent: qpshf
attempts: 1
---

## Summary
A second real session image, `images/claude-dev/Dockerfile`, `FROM` the claude base image, adding a Rust toolchain and the Node tooling a development session needs. It inherits the whole session image contract (agent user, pinned CLI, entrypoint) and adds nothing to it.

## Documents
- `ARCHITECTURE.md`, "Session image" — today says "v1 ships that one image for real work"; rewritten by this task to describe base + dev layer, tags, and where toolchains live and why. "Per-project toolchains are a later extension" stays.
- `README.md`, "Session image" — build commands for the dev image (base first, then dev), and the repository layout list (`images/` line).
- New ADR `docs/decisions/0039-layered-dev-session-image.md`: decision and the two rejected alternatives as recorded in the epic `qpshf` (grow the single image; per-language images).
- `images/claude/Dockerfile` comment "this image is a base that profiles and per-project images build on" now has a concrete first consumer; point it at `images/claude-dev`.

## Acceptance criteria
- [ ] `images/claude-dev/Dockerfile` starts `ARG BASE_IMAGE=mars-session-claude:latest` / `FROM ${BASE_IMAGE}`, switches to `USER root` for installation and ends on `USER agent` with the inherited `ENTRYPOINT` untouched and no `CMD`.
- [ ] Tagged `mars-session-claude-dev:<CLAUDE_CODE_VERSION>` and `mars-session-claude-dev:latest`; the version is still derived from the one `ARG CLAUDE_CODE_VERSION=` line in `images/claude/Dockerfile` (no second pin of the CLI). Same OCI labels pattern as the base, plus a label for the Rust version.
- [ ] **Rust**: rustup with a pinned stable toolchain (`ARG RUST_VERSION=<x.y.z>`), components `rustfmt` and `clippy`, installed with `RUSTUP_HOME=/opt/rustup` and `CARGO_HOME=/opt/cargo` — outside `/session/home`, which is a bind mount at runtime and would shadow them. Both directories are **owned by `agent`**, so a repository's `rust-toolchain.toml` pin can be installed by rustup at runtime and `cargo install` works, without root. The rustup-init download is verified (checksum or pinned version URL), not `curl | sh` unverified.
- [ ] `cargo-binstall` is included so an agent installing a cargo tool (`cargo-nextest`, `sqlx-cli`, …) does not spend minutes compiling it.
- [ ] System packages that user-level installs cannot provide later: `pkg-config`, `libssl-dev`, `clang`, `lld`, `cmake`, `unzip`, `xz-utils`, `ripgrep`, `fd-find` — keep the list justified by a comment, as the base does.
- [ ] **Node**: Node 22 and npm come from the base. Add `corepack enable` (pnpm/yarn shims) and an unprivileged global prefix: `NPM_CONFIG_PREFIX=/session/home/.npm-global` with its `bin` on `PATH`, so `npm install -g` works as `agent`.
- [ ] `PATH` carries `/opt/cargo/bin` and the npm prefix both through `ENV` (the CLI process and execs) **and** through `/etc/profile.d/mars-toolchains.sh`, because the terminal runs `/bin/bash -l` and Debian's `/etc/profile` resets `PATH` for login shells.
- [ ] Build-time assertion block in the style of the base image, run as `agent`: `cargo`, `rustc`, `rustfmt`, `cargo clippy`, `cargo binstall`, `node`, `npm`, `corepack` resolve; `su agent -l -c 'command -v cargo'` succeeds (the login-shell path); `agent` can write into `/opt/rustup` and `/opt/cargo`; a `cargo new` + `cargo build --offline` of a hello-world succeeds; the contract assertions of the base still hold (uid, home, shell, entrypoint).
- [ ] `hadolint` clean with the same ignore discipline as the base (each ignore carries its reason).
- [ ] Docs above updated in the same commit (rule 1).

## Implementation notes
- The build order matters: the base must exist under the tag `BASE_IMAGE` names before the dev image builds. Say so in the README command block; CI wiring is task "Images CI and smoke test cover the dev image".
- `CARGO_HOME=/opt/cargo` puts the registry cache in the container's writable layer, which is lost with the container. That is accepted; a project wanting a warm cache mounts a shared directory at `/opt/cargo/registry` (`ARCHITECTURE.md`, "Shared directories"). Mention this in "Session image"; build nothing for it.
- Under rootless Podman `keep-id:uid=1000,gid=1000` maps the service user to uid 1000, so image files owned by `agent` stay writable by the session process. Docker runs the container as `1000:1000` directly. No orchestrator change is needed for either.
- Do not change `images/claude/mars-entrypoint`; `images.yml` asserts it is byte-identical to the stub's.

## Edge cases
- Expect roughly +1.5 GB over the base. Note the size in `ARCHITECTURE.md` so nobody is surprised by the first pull, and see memory of disk pressure on the dev machine before building repeatedly.
- System libraries cannot be added at runtime (no root). A repository needing one builds its own image `FROM mars-session-claude-dev` and names it in its profiles — say this in "Session image".
- Playwright browsers need `install-deps` (root) and a container engine for the Mars E2E stack; not a goal of this image.