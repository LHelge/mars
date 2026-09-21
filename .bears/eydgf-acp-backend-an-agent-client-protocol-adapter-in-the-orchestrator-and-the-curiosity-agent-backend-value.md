---
id: eydgf
title: "ACP backend: an Agent Client Protocol adapter in the orchestrator and the `curiosity` agent_backend value"
type: epic
status: open
priority: P1
created: "2026-09-21T12:12:26.529401Z"
updated: "2026-09-21T12:12:26.529401Z"
tags:
  - orchestrator
  - agent
  - acp
depends_on:
  - fgbm3
---

## Scope
The second implementation of `AgentBackend`: one adapter module `orchestrator/src/agent/acp/` that speaks ACP over the generalised seam, and the first agent that uses it, `curiosity`, as a new `agent_backend` enum value with its own launch command, credential (`OPENROUTER_API_KEY`, ADR 0036 unchanged) and state directory. A later ACP agent (OpenCode, Copilot) is another enum value over the same module.

Everything here is tested against recorded and scripted ACP transcripts and the mock engine; real containers are the next epic.

## Acceptance criteria
- [ ] `agent_backend` has the value `curiosity` (migration, model, `BACKENDS`, `docs/data-model.md`, `SPEC.md`, frontend type), with one credential per scope enforced like Claude's.
- [ ] The adapter performs the ACP handshake as a preamble (`initialize`, then `session/new` or `session/load`, carrying the Mars MCP server), encodes a message as `session/prompt`, and translates `session/update` and the prompt response into `AgentEvent`s, fixture-tested.
- [ ] A `session/request_permission` from any agent is answered "allow" without reaching the user (ADR 0033 stands).
- [ ] An authentication failure becomes the fatal `error` event naming `OPENROUTER_API_KEY` and its scope.
- [ ] The session-owner restart tests pass over an ACP transcript: no gaps, no duplicates, protocol state rebuilt.
- [ ] `SPEC.md`, `ARCHITECTURE.md` ("Agent process model": a "Curiosity invocation" section) and `docs/data-model.md` change in the same commits as the behaviour.