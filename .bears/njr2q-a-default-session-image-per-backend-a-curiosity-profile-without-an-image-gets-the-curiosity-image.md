---
id: njr2q
title: "A default session image per backend: a Curiosity profile without an image gets the Curiosity image"
status: open
priority: P2
created: "2026-09-21T12:22:17.111171Z"
updated: "2026-09-21T12:22:17.111171Z"
tags:
  - orchestrator
  - config
  - profiles
depends_on:
  - tt9d9
parent: ddb8s
---

## Summary
`SESSION_IMAGE_DEFAULT` names one image (`mars-session-claude-dev:latest`, the toolchain image of ADR 0039), is copied into the seeded profiles at project creation (ADR 0038) and is what the startup probe pulls. With two backends the default depends on the backend, and a profile whose backend and image disagree launches a container without the CLI its command names.

## Documents
- `README.md`, "Configuration" and "Session image"; `.env.example`; compose files; `SPEC.md`, "Agent profiles" (how `image` defaults); `docs/data-model.md`, `agent_profiles.image` note.

## Acceptance criteria
- [ ] `SESSION_IMAGE_DEFAULT` keeps its meaning for `claude` (no deployment breaks); a new optional `SESSION_IMAGE_CURIOSITY` defaults to `mars-session-curiosity-dev:latest` (the toolchain variant, matching what the Claude default became). The startup probe keeps pulling only `SESSION_IMAGE_DEFAULT`: a deployment that never builds the Curiosity image must still start, so a missing Curiosity image is a launch failure with a readable error, not a startup failure — state that in `README.md`. `Config::from_env()` and the fail-fast rule cover it.
- [ ] Profile creation without `image` takes the default of the profile's `backend`. Changing `backend` on update without sending `image` when the current image is the *old* backend's default switches to the new default; any other image is left alone.
- [ ] A launch whose container exits at once because the command is not found fails with a `sessions.error` that says the image has no `<binary>` and names the profile's backend and image — check what the engine reports today and make it readable, rather than validating images up front (an image cannot be inspected for a binary cheaply).
- [ ] The defaults are exposed to the frontend where the profile editor already reads the image default (find it; add to that DTO), so the editor can show the placeholder per backend.

## Testing
- `tests/profiles.rs`: create and update cases; config unit tests; a mock-engine launch test for the readable error.