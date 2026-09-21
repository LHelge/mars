# 0039. A layered dev session image on top of the contract base

Status: accepted.

## Context

`images/claude` is a deliberately modest base: the `agent` user, the pinned CLI, the entrypoint, `git`, `build-essential`, `python3` and Node 22 from its own base image. An implementer session on a Rust repository found no Rust toolchain, and a session cannot install one at the system level: it runs as uid 1000 with `CapDrop: ALL` and `no-new-privileges`, so `apt-get` and `sudo` are impossible (`ARCHITECTURE.md`, "Session container specification"). Real work needs a toolchain in the image.

Two alternatives were considered.

**Growing the single image** — adding Rust and the Node tooling to `images/claude` — makes the contract and a gigabyte of toolchain one thing. Every CLI bump then rebuilds and re-pushes the toolchain that did not change, and every image that wants only the contract (the stub, a project's own image, a future non-Rust layer) drags the toolchain along.

**Per-language images** — a Rust image and a Node image — does not fit how a session runs: one session runs one image, and a repository like Mars itself is Rust *and* Node at once, so a combined image is needed anyway. The per-language split would produce three images to keep in step instead of one.

## Decision

`images/claude` stays the contract base and pins the CLI. `images/claude-dev` builds `FROM` it (`ARG BASE_IMAGE=mars-session-claude:latest`) and adds one polyglot development toolchain: rustup and cargo under `/opt/rustup` and `/opt/cargo` owned by `agent`, `cargo-binstall`, the system libraries a session cannot add later, `corepack`, and an unprivileged npm global prefix. It adds nothing to the session image contract and restates no part of it, the CLI version included: its tags and labels are derived from the base's single `ARG CLAUDE_CODE_VERSION=` line.

## Consequences

A CLI bump rebuilds the base and re-runs the dev layer's installs; a toolchain bump leaves the base alone. Builds are ordered — base, then dev — in the README and in CI.

The toolchain lives outside `/session/home` because that path is a bind mount at runtime, and `/opt/rustup` and `/opt/cargo` are owned by `agent` so that a repository's own `rust-toolchain.toml` pin and `cargo install` work without root. The registry cache is therefore in the container's writable layer and is lost with the container; a project wanting a warm cache mounts a shared directory at `/opt/cargo/registry`.

Further languages go in this layer, or in a further layer on the base. A repository needing a system library the dev image does not carry builds its own image `FROM mars-session-claude-dev`.
