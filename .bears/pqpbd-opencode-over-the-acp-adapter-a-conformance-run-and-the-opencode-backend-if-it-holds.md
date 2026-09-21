---
id: pqpbd
title: "OpenCode over the ACP adapter: a conformance run, and the `opencode` backend if it holds"
status: open
priority: P3
created: "2026-09-21T12:23:32.102420Z"
updated: "2026-09-21T12:23:32.102420Z"
tags:
  - orchestrator
  - agent
  - acp
  - backlog
  - agent-backends
depends_on:
  - "7bnmc"
---

## Summary
The ACP adapter was developed against an agent Mars controls. Run it against an agent it does not — `opencode acp` (npm `opencode-ai`, MIT) — to learn whether the adapter implements the protocol or Curiosity's reading of it. If it holds, OpenCode is a third backend at the cost of an enum value, an image and configuration.

## Acceptance criteria
- [ ] A probe in the manner of `orchestrator/tests/claude_probe.rs`: the real `opencode acp` in a container under the session contract, driven through the adapter's own preamble and `encode_input`, recordings under `tests/fixtures/acp/opencode/<version>/` with a `NOTES.md`. Needs a credential (`OPENROUTER_API_KEY`); never committed.
- [ ] Findings answered: handshake accepted as sent; HTTP MCP entry with a bearer header connects (OpenCode's `mcpCapabilities.http`); updates translate without `raw` for text, thinking and tool calls; permission requests under its allow-all configuration; usage and cost present or absent; `session/load` in a fresh process on the same state directory; PID 1 signal behaviour (a Node CLI may need `Init: true` — `ARCHITECTURE.md`, "Session image" says what to do then).
- [ ] Every adapter defect found is fixed or filed as a task linked here.
- [ ] A decision recorded in this task on close: add `agent_backend = 'opencode'` (then file the epic: enum + index, state directory via XDG variables, image with a pinned version, allow-all permission config, how the profile's system prompt reaches it, credential names — which collides with the provider-credential question below) or not, and why.

## Notes from the feasibility discussion
- `opencode run --format json` and `opencode serve` (HTTP+SSE) are not candidates: the first is a process per turn with open event-loss bugs, the second bypasses the transcript file that recovery depends on (ADR 0010).
- Claude subscriptions cannot be used through OpenCode (Anthropic bills third-party harness use as per-token extra usage since April 2026); ChatGPT-subscription OAuth has the rotating refresh-token problem described in the ADR of the ACP spike.