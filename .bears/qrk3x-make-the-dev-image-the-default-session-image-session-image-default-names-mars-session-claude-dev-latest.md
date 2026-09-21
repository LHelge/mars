---
id: qrk3x
title: "Make the dev image the default session image: SESSION_IMAGE_DEFAULT names mars-session-claude-dev:latest"
status: open
priority: P2
created: "2026-09-21T10:01:31.876022786Z"
updated: "2026-09-21T10:01:31.876022786Z"
tags:
  - orchestrator
  - config
  - docs
depends_on:
  - zqe6v
parent: qpshf
---

## Summary
New projects' seeded profiles, the role templates served by `GET /profile-templates` and the startup probe all take their image from `SESSION_IMAGE_DEFAULT`. Move its default from the base image to the dev image, so a new project can build Rust and Node out of the box.

## Documents
- `README.md`, "Configuration" — the `SESSION_IMAGE_DEFAULT` row (default value; "build it as described under 'Session image'" now means base then dev); "Session image" — which tag the default names; the deployment walkthrough if it lists the image to build before first start.
- `.env.example` line 84.
- `ARCHITECTURE.md`, "Session image" — the sentence "`mars-session-claude:latest` … is what `SESSION_IMAGE_DEFAULT` names by default".
- `SPEC.md`, "Agent profiles" says profiles start "on the built-in Claude image" — check the wording still holds.

## Acceptance criteria
- [ ] `orchestrator/src/prelude/config.rs`: the `SESSION_IMAGE_DEFAULT` constant and its doc comment name `mars-session-claude-dev:latest`; the config unit tests asserting the default follow.
- [ ] `.env.example` and `README.md` carry the same value (the variable contract, `CLAUDE.md` "Backend conventions").
- [ ] Compose files and the systemd host-run notes are checked for a hard-coded image name; none is left pointing at the base by accident.
- [ ] The startup probe needs nothing from the toolchain, so it works on either image; confirm it still passes against the dev image on Podman (`tests/engine.rs` uses its own probe image and is unaffected).
- [ ] Test fixtures that set `SESSION_IMAGE_DEFAULT` to `mars-session-claude:dev` are arbitrary strings, not the new image; leave them unless a reader would be misled, in which case rename consistently.
- [ ] Backend quality chain passes. E2E is unaffected (`E2E_STUB_IMAGE` overrides the default).

## Implementation notes
- **No migration.** `agent_profiles.image` was copied at seeding and the rows are the user's (ADR 0038); an existing project keeps the base image until someone edits the profile. State that in the README row in one sentence — it is the first thing an operator upgrading will trip over — and do not write an `UPDATE`.
- An operator who pinned `SESSION_IMAGE_DEFAULT` in `.env` is unaffected by the new default; the upgrade note is "build the dev image, then either unset the variable or point it at the dev tag".
- A pull failure of the default image is already a fatal, well-worded startup error ("Startup probe"); the dev image not having been built yet will surface there. Make sure the README's first-start steps build it before `compose up`.