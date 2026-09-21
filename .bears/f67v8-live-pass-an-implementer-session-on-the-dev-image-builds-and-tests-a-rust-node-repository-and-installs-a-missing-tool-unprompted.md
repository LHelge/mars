---
id: f67v8
title: "Live pass: an implementer session on the dev image builds and tests a Rust+Node repository and installs a missing tool unprompted"
status: in_progress
priority: P2
created: "2026-09-21T10:01:49.299009734Z"
updated: "2026-09-21T11:31:48.343321657Z"
tags:
  - images
  - verification
depends_on:
  - zqe6v
  - g8rx8
  - qrk3x
parent: qpshf
attempts: 1
---

## Summary
Manual, credentialed verification with the real CLI, in the manner of `images/claude/VERIFY.md` and task `scfnj`: prove that the reported problem is gone end to end — a seeded `implementer` on the dev image can build and test a Rust project — and that the new prompt paragraph changes behaviour when a tool really is missing.

## Documents
- `images/claude-dev/VERIFY.md` (new, written by this task): procedure, date, CLI version, engine, observed results, reported cost. Same rule-3 discipline as the base image's file: the credential lives only in the operator's shell, never in the file or a log.
- `ARCHITECTURE.md`, "Session image" — a pointer to the new VERIFY file, and any correction the run forces.

## Acceptance criteria
- [ ] A fresh project created on an orchestrator whose `SESSION_IMAGE_DEFAULT` is the dev image; its seeded `implementer` carries the new environment paragraph and the dev image, with no manual edit.
- [ ] Scenario A — toolchain present: a small throwaway repository with a Cargo crate and a `package.json`; a task asking for a change with tests. The session runs `cargo build`/`cargo test` and `npm test` successfully; no "cargo: command not found" anywhere in the transcript.
- [ ] Scenario B — pinned toolchain: the repository carries a `rust-toolchain.toml` naming a version other than the image's. rustup installs it at runtime without root and the build proceeds.
- [ ] Scenario C — missing tool: the repository's instructions require a tool the image does not ship (for instance `cargo nextest run`). The agent installs it at user level and carries on, rather than stopping or handing off as blocked.
- [ ] Scenario D — root needed: the instructions require something only `apt` could provide. The agent does not loop on `sudo`/`apt`; it reports the limitation in the task comment or hand-off, as the paragraph asks.
- [ ] The web terminal (`/bin/bash -l`) in the same session resolves `cargo` and `node` — the login-shell `PATH` case.
- [ ] Nothing installed shows up in the session's commits.
- [ ] Findings that contradict an acceptance criterion of `zqe6v` or `g8rx8` become new Bears tasks linked to this one, not edits made on the side.

## Implementation notes
- Rootless Podman on the dev machine; pinned CLI; OAuth token from the operator's environment only. Use a cheap model for the runs and record the reported cost.
- Check free disk before building: base + dev image plus a cargo build inside a session is several GB.
- Scenarios C and D are about prompt wording. If the agent hesitates, the fix is a wording change in the templates (a follow-up task against `SPEC.md`, "Role profile templates"), which again reaches new projects only.