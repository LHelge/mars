---
id: vj82v
title: "Curiosity speaks ACP: stdio server, MCP client, resumable sessions and the PID 1 contract"
type: epic
status: open
priority: P1
created: "2026-09-21T12:12:18.681440Z"
updated: "2026-09-21T12:12:18.681440Z"
tags:
  - curiosity
  - agent
  - acp
depends_on:
  - w9nsq
---

## Scope
Turn the headless agent of the foundation epic into a process Mars can drive: an Agent Client Protocol server on stdio (newline-delimited JSON-RPC), with the Mars task tracker and any other server reached through an MCP client, conversations persisted so `session/load` survives a container replacement (ADR 0003, ADR 0015), and the process behaviour the session image contract demands (`ARCHITECTURE.md`, "Session image": the CLI is PID 1, handles `SIGINT`/`SIGTERM` itself, reads a FIFO, writes JSON lines to stdout).

The ACP findings of the seam epic's spike (ADR and `NOTES.md`) are the reference for what the protocol needs; this epic does not re-derive them.

## Acceptance criteria
- [ ] `curiosity acp` serves `initialize`, `session/new`, `session/load`, `session/prompt`, `session/cancel` and streams `session/update`; it never sends `session/request_permission` (ADR 0012: the container is the boundary).
- [ ] MCP servers named in `session/new` (HTTP with headers, stdio) are connected and their tools offered to the model.
- [ ] A session persisted by one process is loaded by another from the same state directory and continues the conversation.
- [ ] `SIGINT` ends the turn in progress and the process; `SIGTERM` exits 143; stdout carries nothing but protocol lines.
- [ ] Every turn reports token usage and, where the provider gives it, cost.
- [ ] A scripted mock model makes a run deterministic without credentials, for Mars's end-to-end tests.

## Out of scope
Compaction, instruction-file loading, the browser tool, providers beyond OpenRouter (backlog tasks).