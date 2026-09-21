---
id: "2mpjd"
title: "ACP adapter failure paths: a rejected credential parks the session naming OPENROUTER_API_KEY; an MCP server that did not connect is a launch_warning"
status: open
priority: P2
created: "2026-09-21T12:21:40.860549Z"
updated: "2026-09-21T12:21:40.860549Z"
tags:
  - orchestrator
  - agent
  - acp
  - secrets
depends_on:
  - sw5d3
parent: eydgf
---

## Summary
Two things the Claude adapter derives from Claude-specific lines and the ACP adapter must derive from its own: the fatal authentication `error` that names the injected secret and its scope (`ARCHITECTURE.md`, "Claude Code invocation", Credentials), and the MCP `launch_warning`.

## Documents
- `ARCHITECTURE.md`, "ACP adapter" and "Curiosity invocation"; `SPEC.md`, "AgentEvent", `error` and `launch_warning` rules if their wording names Claude.

## Acceptance criteria
- [ ] A prompt error whose `data.kind` is `authentication` (Curiosity's classification; its README is the contract) → one `error` event with `fatal: true` naming the injected credential (from `TranslateState.credential`: name and scope, never a value), once per process; the owner parks the session exactly as for Claude. A generic ACP agent without that field: HTTP-status heuristics are **not** added — the error is non-fatal text and the session stays up.
- [ ] A launch with no credential resolved keeps today's `launch_warning` and is not refused (ADR 0036).
- [ ] MCP status: if the agent reports a failed server (Curiosity's documented signal), the adapter emits the same `launch_warning` wording the Claude path uses for a degraded session; absence of a report raises nothing (seam epic, `init` task).
- [ ] The process exiting during the handshake (bad argv, missing binary in the image, protocol version refused) fails the launch with a readable `sessions.error`, using stderr's tail as the Claude path does.

## Testing
- Fixture lines for each case; owner tests through the mock engine for park-on-auth-failure and handshake exit.