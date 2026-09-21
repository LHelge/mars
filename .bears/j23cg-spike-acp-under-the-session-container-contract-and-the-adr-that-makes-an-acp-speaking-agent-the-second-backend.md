---
id: j23cg
title: "Spike: ACP under the session container contract, and the ADR that makes an ACP-speaking agent the second backend"
status: open
priority: P2
created: "2026-09-21T12:12:57.966345Z"
updated: "2026-09-21T20:26:14.616203814Z"
tags:
  - orchestrator
  - agent
  - acp
  - spike
  - docs
parent: fgbm3
---

## Summary
The seam is about to be reshaped for a protocol nobody here has run. Read the Agent Client Protocol specification (https://agentclientprotocol.com, schema under `/protocol/schema`) and run a real ACP agent — `opencode acp` (npm `opencode-ai`) is the reference implementation to probe — under the session container contract, record what was observed, and write the ADR. The other tasks of this epic cite the findings instead of a summary from memory.

## Documents
- New ADR under the next free number in `docs/decisions/` (0039 is the layered dev session image; check the directory when starting) and its row in `docs/decisions/README.md`. Decision: the second backend is Mars's own agent, Curiosity, speaking ACP natively; one `agent_backend` value per agent, every ACP agent sharing one adapter module; Claude Code stays on its native stream-json. Rejected alternatives to record: mimicking Claude's stream-json (undocumented, version-specific, quirks Mars only tolerates), a Mars-private protocol (gains nothing for third-party agents), native adapters for Codex/Pi/Copilot CLIs now (pinning cost per CLI; Codex subscription auth is a rotating single-use refresh token that cannot be shared by parallel containers), a single `acp` enum value (launch command, credentials and state directory differ per agent).
- `README.md` "Roadmap after v1", `SPEC.md` "Non-goals for v1", `ARCHITECTURE.md` "Agent process model": replace "GitHub Copilot CLI is the candidate".
- `orchestrator/tests/fixtures/acp/NOTES.md` plus the recorded transcripts beside it (the convention of `tests/fixtures/claude/<version>/NOTES.md`).

## Acceptance criteria
- [ ] `NOTES.md` answers, each with the recorded lines that show it: (1) framing on stdio — one JSON object per line, anything else on stdout; (2) the handshake order and what `initialize` negotiates (`protocolVersion`, `agentCapabilities.loadSession`, `mcpCapabilities.http`, prompt capabilities); (3) how an HTTP MCP server with an `Authorization` header is passed in `session/new` and whether the agent connects to it; (4) the `session/update` variants seen in a turn with text, thinking, a tool call and its result, and what ends a turn (the `session/prompt` response and its `stopReason`); (5) what a second `session/prompt` during a running turn does; (6) `session/cancel` semantics; (7) `session/load` in a **new process** on the same state directory — replayed or not, same id or not; (8) whether `session/request_permission` is sent under the agent's allow-all configuration; (9) usage and cost fields (`usage_update`, `PromptResponse.usage`, `_meta`); (10) behaviour as PID 1 on `SIGINT`/`SIGTERM` and on stdin held open by a FIFO (ADR 0034); (11) the extension mechanism (`_meta`, underscore-prefixed methods) Curiosity may use for exact usage.
- [ ] The ADR is written and the three documents no longer name Copilot as the candidate.
- [ ] A short "what the seam must provide" list closes `NOTES.md`; if it contradicts a sibling task of this epic, that task's body is updated.

## Implementation notes
- Run it in a container built ad hoc on the stub or claude image's entrypoint (`images/claude/mars-entrypoint`), not on the host, so the FIFO and PID 1 findings are real. No image is committed by this task.
- Items 4, 5, 8 and 9 need model turns and therefore a credential (`OPENROUTER_API_KEY`, or a free model if OpenCode offers one). Never commit it or paste it into a fixture (`CLAUDE.md`, rule 3). Without one, record items 1–3, 6, 7, 10, 11, which need none, and file the rest as a new task linked to this one.
- The Rust crate `agent-client-protocol` (Zed) exists; note its version and whether its types are usable without its runtime, for the adapter and for Curiosity.

## Testing
None of its own: the output is documents and fixtures.