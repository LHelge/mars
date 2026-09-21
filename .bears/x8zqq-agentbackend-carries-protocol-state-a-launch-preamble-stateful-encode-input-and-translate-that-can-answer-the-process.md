---
id: x8zqq
title: "AgentBackend carries protocol state: a launch preamble, stateful encode_input, and translate that can answer the process"
status: open
priority: P1
created: "2026-09-21T12:13:15.219185Z"
updated: "2026-09-21T12:13:15.219185Z"
tags:
  - orchestrator
  - agent
  - sessions
depends_on:
  - j23cg
parent: fgbm3
---

## Summary
A JSON-RPC protocol needs three things the trait cannot express: lines written before any user input (the handshake), per-process state while encoding an input (request ids, the protocol's session id), and lines written in reply to something the process said (a response to a server-initiated request). Add them, with Claude's behaviour unchanged.

## Documents
- `ARCHITECTURE.md`, "Agent process model" (the trait listing and the paragraph on `LaunchContext`/`TranslateState`), "Input encoding", "Launch sequence".
- ADR: a short one if the spike's ADR does not already cover the trait shape.

## Acceptance criteria
- [ ] `fn preamble(&self, ctx: &LaunchContext, state: &mut TranslateState) -> Vec<String>`: lines the owner writes as soon as stdin is attached, before any queued input (ADR 0032 order kept). Claude returns none.
- [ ] `encode_input` takes `&mut TranslateState`. Claude's hash bookkeeping of sent inputs moves into it if that is where it naturally lives; behaviour identical.
- [ ] `translate` returns events **and** outbound lines (a small struct, not a tuple); the owner writes outbound lines through the same single stdin writer as inputs, in order, never interleaved inside a line.
- [ ] Backend-specific protocol state lives in `TranslateState` behind a per-backend member, not as Claude fields every backend sees.
- [ ] `MockAgentBackend` can script a preamble and an outbound reply; an owner test asserts both reach the engine's stdin in order.
- [ ] Ephemeral launches still attach no stdin writer (ADR 0034); a backend that needs a preamble for an ephemeral run is a case the ACP epic handles — state the rule in the doc, do not build it here.

## Implementation notes
- `orchestrator/src/agent/mod.rs`, `agent/state.rs`, `agent/claude/{mod,input}.rs`, `agent/mock.rs`; call sites `session/owner.rs` (`translate` ~l.793 and ~l.1769, `encode_input` ~l.1005).
- Outbound lines are not user input: they produce no `user_message` event and are never logged at `info` (they may carry the MCP bearer token, `CLAUDE.md` rule 3).

## Edge cases
- stdin not yet attached when `translate` yields an outbound line (recovery replay): see the restart task; here, replayed lines must not re-send replies.

## Testing
- Unit tests beside the trait; owner tests in `tests/` through the mock backend and mock engine. Both clippy invocations and the nextest suite pass.