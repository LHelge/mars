---
id: u9jp4
title: "Fix the cargo-registry shared-directory preset: the dev image's registry is /opt/cargo/registry"
status: done
priority: P2
created: "2026-09-22T19:08:28.086248735Z"
updated: "2026-09-22T20:30:24.704037022Z"
tags:
  - frontend
  - docs
  - bug
parent: gtbp5
attempts: 1
---

`frontend/src/utils/sharedDir.ts` (`SHARED_DIR_PRESETS`, `cargo-registry`) and the README "Operating notes" shared-directories table share `/session/home/.cargo/registry`, but `images/claude-dev/Dockerfile` sets `CARGO_HOME=/opt/cargo` (ARCHITECTURE.md "Session image", which already recommends `/opt/cargo/registry`). On the default dev image the preset shares a directory Cargo never uses.

Point the preset and the README table at `/opt/cargo/registry`, noting that it follows the image's `CARGO_HOME` (a custom image with the default `CARGO_HOME` would use `/session/home/.cargo/registry`). Check the shared-dir path validation accepts `/opt/...` (it forbids only `/data` and ancestors of the session mounts). Update any unit/E2E test that asserts the preset path.