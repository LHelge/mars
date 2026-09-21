---
id: pfs5r
title: "curiosity acp: the ACP server on stdio — initialize, session/new, session/prompt, session/update, session/cancel"
status: open
priority: P1
created: "2026-09-21T12:19:19.239709Z"
updated: "2026-09-21T12:19:19.239709Z"
tags:
  - curiosity
  - acp
depends_on:
  - bywt4
  - j23cg
parent: vj82v
---

## Summary
The interface Mars drives. `curiosity acp [--model …] [--system-prompt …] [--effort …]` reads newline-delimited JSON-RPC 2.0 from stdin and writes it to stdout, implementing the agent side of the Agent Client Protocol. The spike's findings (`orchestrator/tests/fixtures/acp/NOTES.md`, the ADR) are the reference for framing, handshake order and the `session/update` variants real agents emit.

## Documents
- `curiosity/README.md`: the protocol surface — methods served, capabilities advertised, update variants emitted, what is deliberately absent. `curiosity/docs/acp.md` if it outgrows the README.

## Acceptance criteria
- [ ] `initialize` negotiates the protocol version and advertises capabilities truthfully (`loadSession` false until the persistence task; `mcpCapabilities.http` false until the MCP task).
- [ ] `session/new` takes `cwd` and returns a session id; one process serves one session at a time — a second `session/new` is answered, the earlier session is closed. `authenticate` is not required: the credential is in the environment.
- [ ] `session/prompt` runs one turn of the loop; `session/update` notifications carry agent text chunks, thought chunks (a liveness signal at least — midgaard never shows reasoning text), `tool_call` and `tool_call_update` with status and content; the response carries the `stopReason` (`end_turn`, `max_tokens`, `cancelled`, `refusal`, as applicable).
- [ ] `session/cancel` cancels the running turn through the cancellation token; the pending `session/prompt` is answered with `cancelled`.
- [ ] A `session/prompt` while a turn is running is rejected with a JSON-RPC error and changes nothing — Mars holds input until the turn ends (seam epic, turn-gating task). Document it.
- [ ] Never sends `session/request_permission`, `fs/*` or `terminal/*` client requests: tools act directly in the container (ADR 0012).
- [ ] Malformed JSON, unknown method, wrong params: JSON-RPC errors, the process keeps serving. stdout never carries anything but protocol lines; stdin EOF ends the process with 0 after the turn in progress is cancelled.
- [ ] Model, system prompt and effort come from argv/env, as in `curiosity run` — that is how a Mars profile's `model` and `system_prompt` arrive.

## Implementation notes
- Evaluate the `agent-client-protocol` crate (Zed) first, per the spike's note: use its schema types if they can be had without adopting a runtime that fights the existing tokio loop; otherwise hand-written serde types for the subset above. Record the choice in the README.
- Map from the internal `EventKind` stream to `session/update`; the internal events stay the source of truth (they are what `curiosity run` prints and what persistence will store).

## Testing
- Integration tests drive the binary (or the server function over in-memory pipes) with the mock script: handshake, a tool-using turn, cancel mid-turn, prompt-while-busy, garbage line. Quality chain passes.