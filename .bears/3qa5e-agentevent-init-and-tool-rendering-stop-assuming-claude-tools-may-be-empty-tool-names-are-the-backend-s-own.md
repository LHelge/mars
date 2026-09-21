---
id: "3qa5e"
title: "AgentEvent init and tool rendering stop assuming Claude: tools may be empty, tool names are the backend's own"
status: open
priority: P2
created: "2026-09-21T12:17:44.599483Z"
updated: "2026-09-21T12:17:44.599483Z"
tags:
  - orchestrator
  - frontend
  - agent
  - spec
parent: fgbm3
---

## Summary
`SPEC.md`, "AgentEvent" says `init.tools` is required because Claude Code always sends it. An ACP agent announces no tool list in its handshake. And the transcript UI may render tool calls by Claude's tool names (`Bash`, `Edit`, `Read`, `Agent`). Audit both and make the schema and the renderer hold for a backend whose tools are unknown in advance. Additive only (ADR 0008).

## Documents
- `SPEC.md`, "AgentEvent" (`init` rule, `tool_call`/`tool_result`), "Frontend" (transcript rendering); `ARCHITECTURE.md`, "Claude Code invocation" where it says `tools` is required.

## Acceptance criteria
- [ ] `init.tools` and `init.mcp_servers` may be empty for a backend that does not announce them; serde accepts an absent field as empty (`#[serde(default)]`) so stored Claude events are unaffected. The Claude translator is unchanged.
- [ ] Wherever the orchestrator derives something from `init` (the MCP `launch_warning`), an empty `mcp_servers` means "unknown", not "Mars server missing": no warning is raised from absence alone. State the rule.
- [ ] Frontend audit written into the task on close: every place a tool name or a Claude-specific input shape selects a renderer. Each falls back to the generic tool-call block (name, collapsed JSON input, result) for an unknown name — add the fallback where it is missing, with a unit test.
- [ ] `frontend/src/types/agentEvent.ts` mirrors the spec exactly.

## Implementation notes
- `orchestrator/src/events/agent_event.rs` (`Init`, ~l.95), `session/owner.rs` / `launcher.rs` for the warning, `frontend/src/session/` reducers and tool renderers, `frontend/src/types/agentEvent.ts`.
- Invoke `/frontend-design` only if a renderer's shape changes; a fallback that already exists needs no design pass.

## Testing
- Rust: serde round-trip of an `init` without `tools`. Frontend: Vitest for the reducer and the fallback renderer; full frontend chain.