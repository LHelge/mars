---
id: fgbm3
title: "Agent seam: make the AgentBackend boundary carry a stateful, bidirectional protocol and nothing Claude-specific outside the adapter"
type: epic
status: open
priority: P2
created: "2026-09-21T12:11:58.671687Z"
updated: "2026-09-21T20:26:14.502535763Z"
tags:
  - orchestrator
  - agent
  - architecture
---

## Scope
`AgentBackend` (`orchestrator/src/agent/mod.rs`; `ARCHITECTURE.md`, "Agent process model") was designed for a second backend but assumes Claude Code's protocol in four places: `encode_input` is stateless and the backend can never originate a stdin line; the owner relies on the CLI queueing mid-turn input; the launcher hard-codes `CLAUDE_CONFIG_DIR` and the Claude `mcpServers` file; the owner applies Claude's cumulative-cost rule. The second backend will speak the Agent Client Protocol (ACP, JSON-RPC over stdio: handshake, request ids, server-initiated requests), so the seam is generalised first, with **no behaviour change**: Claude and the mock are the only backends when this epic closes.

The first task is a spike that reads the ACP specification and runs a real ACP agent under the session container contract, and records the decision as an ADR. The remaining tasks are shaped by its findings.

## Acceptance criteria
- [ ] An ADR records: the second backend is Mars's own agent (Curiosity) speaking ACP; one `agent_backend` value per agent, all ACP agents sharing one adapter module; Copilot CLI is no longer "the candidate" in `README.md`, `SPEC.md`, `ARCHITECTURE.md`.
- [ ] A backend can write a preamble, keep per-process protocol state across `encode_input`/`translate`, and answer a server-initiated request; that state survives an orchestrator restart.
- [ ] The owner holds or forwards mid-turn input according to a policy the backend declares.
- [ ] No Claude-specific name remains in `engine/spec.rs`, `projects/layout.rs`, `session/prepare.rs` or the owner's cost accounting other than through the backend.
- [ ] Every session, engine and e2e test passes unchanged in behaviour; the full backend and frontend quality chains pass.

## Out of scope
A new `agent_backend` value, the ACP adapter, Curiosity itself.