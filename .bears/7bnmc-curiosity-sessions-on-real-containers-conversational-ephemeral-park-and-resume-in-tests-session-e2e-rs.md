---
id: "7bnmc"
title: "Curiosity sessions on real containers: conversational, ephemeral, park and resume in tests/session_e2e.rs"
status: open
priority: P1
created: "2026-09-21T12:22:27.662334Z"
updated: "2026-09-21T12:22:27.662334Z"
tags:
  - orchestrator
  - tests
  - engine
  - curiosity
depends_on:
  - p7ghm
  - hy5bz
  - "2mpjd"
parent: ddb8s
---

## Summary
`tests/session_e2e.rs` is the session lifecycle on real containers over the stub image. Add the same lifecycle over the Curiosity image with its scripted mock model: no credentials, deterministic, and — unlike the stub — the real binary speaking the real protocol, including a real MCP round trip to the orchestrator under test.

## Documents
- `README.md`, "Development" (the image variable for the test, as `MARS_STUB_IMAGE` is documented); `CLAUDE.md`, "Testing expectations", Engine tests paragraph.

## Acceptance criteria
- [ ] Runs only with `DOCKER_HOST` and under the `cargo test --test engine --test session_e2e` invocation, serialised with the suite's existing mutex; image from `MARS_CURIOSITY_IMAGE`, skipped with a printed line when unset, as the stub is.
- [ ] Conversational: launch, `init` arrives before any message, a message produces text and a tool call/result and a `result` with usage; a second message during the turn is held and answered after it.
- [ ] MCP: the mock script calls a Mars tracker tool (`ready` or `get_task`) and the tool result is the orchestrator's answer — proving `session/new` MCP injection, the bearer token and Curiosity's MCP client end to end.
- [ ] Park (stop) → exit 0, transcript ends with a closed turn; next message relaunches with `session/load` and the mock script asserts it saw the earlier history.
- [ ] Ephemeral: runs to `done`; fetch-back happens.
- [ ] The mock script reaches the container through an env var or a bind the test harness controls; no test-only switch is added to the production launcher beyond what the stub already uses (`MARS_STUB_*` precedent) — if one is needed, it is behind `integration-tests`.
- [ ] Replace any hand-written ACP fixtures in `tests/fixtures/acp/curiosity/` with recordings from this binary.

## Testing
- On Podman and Docker, as CI's engine workflow does; `engine.yml` builds or pulls the Curiosity image.