---
id: ddb8s
title: "Curiosity sessions end to end: session image, default image per backend, profile editor and Playwright coverage"
type: epic
status: open
priority: P2
created: "2026-09-21T12:12:35.488524Z"
updated: "2026-09-21T12:12:35.488524Z"
tags:
  - images
  - frontend
  - orchestrator
  - curiosity
  - tests
depends_on:
  - vj82v
  - eydgf
---

## Scope
Make a Curiosity session something a user can launch from the browser: a session image that honours the contract of `ARCHITECTURE.md`, "Session image", a default image per backend, the backend choice in the profile editor with the OpenRouter credential set up through the guided form, a real-container probe and Playwright scenarios over Curiosity's scripted mock model.

## Acceptance criteria
- [ ] `images/curiosity/` builds a static binary into an image tagged with the crate version, pinned against the adapter by a unit test as the Claude image is; `images/smoke-test.sh` and the image CI cover it.
- [ ] A profile created with backend `curiosity` defaults to the Curiosity image; `README.md`, "Configuration" and `.env.example` carry the variable.
- [ ] A conversational and an ephemeral Curiosity session run on real containers (`tests/session_e2e.rs` rule: only with `DOCKER_HOST`), park and resume.
- [ ] The profile editor selects the backend, the launch form and the Secrets page show and create the credential of the selected backend.
- [ ] Playwright scenarios run a Curiosity session on the mock model; `frontend/tests/README.md` carries their rows.