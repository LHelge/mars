---
id: qpshf
title: "Session dev image: a Rust and Node toolchain layered on the base image, and role prompts that let agents install what is missing"
type: epic
status: open
priority: P1
created: "2026-09-21T10:00:16.816029740Z"
updated: "2026-09-21T10:00:16.816029740Z"
tags:
  - images
  - orchestrator
  - profiles
---

## Summary
An implementer session on a Rust repository found no Rust toolchain: `images/claude` is a deliberately modest base (`build-essential`, `git`, `python3`, Node 22 from its base image) and nothing else ships. This epic adds a **layered dev image** — `images/claude-dev`, `FROM` the base, carrying rustup/cargo and the Node tooling — makes it what `SESSION_IMAGE_DEFAULT` names, and teaches the role prompts that the container is disposable and a missing tool is something to install, not a blocker.

## Decisions already taken (2026-09-21, with the user)
- **Image shape: layered, one polyglot dev image.** `images/claude` stays the contract base; `images/claude-dev` builds on it with Rust + Node. Rejected: growing the single image (base and fat toolchain become one thing, every CLI bump rebuilds everything) and per-language images (a session runs one image, and a Rust+Node repository such as Mars itself needs both at once, so a combined image is needed anyway). Recorded as an ADR by the image task.
- **Prompt home: the role templates**, not a launcher-owned preamble. Consistent with ADR 0038 (copied at creation, never resolved live): new projects get the text, existing projects pick it up through the template picker (`GET /profile-templates`) or a manual edit.

## Facts the tasks rely on
- Sessions run as uid 1000 with `CapDrop: ALL` and `no-new-privileges` (`ARCHITECTURE.md`, "Session container specification"), so **`apt-get` and `sudo` are impossible at runtime**. "Install what is missing" means user-level installs only: `rustup`, `cargo install`, `npm -g` into a user prefix, `npx`, a Python venv. System libraries must be in the image.
- `/session/home` is a bind mount, so anything an image bakes under `$HOME` is shadowed at runtime. Toolchains live outside it (`/opt/...`).
- The root filesystem is writable and the container disposable; `/session/home` and `/session/work` survive for the session's life.
- The terminal runs `/bin/bash -l`, and Debian's `/etc/profile` resets `PATH` for login shells, so a `PATH` set only with `ENV` is lost there.

## Documents
- `ARCHITECTURE.md`, "Session image", "Session container specification"
- `README.md`, "Session image", "Configuration", "CI"
- `SPEC.md`, "Role profile templates"
- ADR 0038; a new ADR for the image shape

## Out of scope
- Per-project toolchain setup scripts run by the entrypoint (`README.md`, "Roadmap after v1") — unchanged, still later.
- Build caches across sessions (sccache, a shared cargo registry). A project can already mount a shared directory at the cache path; nothing new is built for it here.
- Further languages (Go, Python tooling beyond `python3`, JVM). The layer is where they go later.