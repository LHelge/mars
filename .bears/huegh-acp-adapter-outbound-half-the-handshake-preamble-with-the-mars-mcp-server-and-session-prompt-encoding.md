---
id: huegh
title: "ACP adapter, outbound half: the handshake preamble with the Mars MCP server, and session/prompt encoding"
status: open
priority: P1
created: "2026-09-21T12:21:03.686056Z"
updated: "2026-09-21T12:21:03.686056Z"
tags:
  - orchestrator
  - agent
  - acp
depends_on:
  - tt9d9
  - wt969
parent: eydgf
---

## Summary
Everything Mars writes to an ACP agent. The handshake is the preamble of the generalised seam; a user message is a `session/prompt` request.

## Documents
- `ARCHITECTURE.md`, "Agent process model": a section "ACP adapter" (shared by every ACP agent) beside "Curiosity invocation"; "Input encoding" gains the ACP shape; "Launch sequence" for when `cli_session_id` becomes known. `SPEC.md` only if an event rule changes.

## Acceptance criteria
- [ ] Native types in `agent/acp/native.rs` for the subset used (requests, responses, notifications), serde-only, tolerant of unknown fields. If the spike found the `agent-client-protocol` crate's types usable, `cargo add` it instead and record the crate in `ARCHITECTURE.md`, "Orchestrator internals".
- [ ] Preamble, fresh launch: `initialize` (client capabilities: no `fs`, no `terminal`), then `session/new` with `cwd: /session/work` and the Mars MCP server as an HTTP entry whose `Authorization` header carries the per-launch token (ADR 0029). Resumed launch: `session/load` with the stored `cli_session_id` instead; if the agent did not advertise `loadSession`, fall back to `session/new` and emit a `launch_warning` saying the conversation could not be resumed.
- [ ] The handshake is pipelined (all lines written at once) only if the spike showed agents accept it; otherwise the adapter sends the next request when `translate` sees the previous response — which the outbound half of `translate` exists for.
- [ ] The `init` event is emitted from the `session/new`/`session/load` response: `cli_session_id` = the ACP session id, `resumed` from the launch, `model` from the launch context, `tools` empty. Unlike Claude, this happens before the first message (no ADR 0032 deadlock: the handshake costs no model turn) — document that `cli_session_id` is known at once for this backend.
- [ ] `encode_input`: a `session/prompt` request with one text content block and a UUID request id recorded in the state as the open turn. Empty or whitespace-only text is `Error::BadRequest`, as for Claude.
- [ ] Stop: Mars's stop is a signal, not `session/cancel`; state that. `session/cancel` encoding exists (a future "stop the turn, keep the process" action would use it) but nothing calls it.
- [ ] The bearer token is never logged and never appears in an event; the preamble lines are excluded from any `debug` line dump.

## Implementation notes
- `orchestrator/src/agent/acp/{mod,native,launch,input,state}.rs`, mirroring `agent/claude/`.
- State needed after a restart (session id, open request id) must be derivable from the transcript (seam epic, restart task): the `session/new` response carries the session id; the open turn is "a `session/update` after the last prompt response".

## Testing
- Unit tests on exact encoded lines (as `agent/claude/input.rs` has); a mock-engine owner test asserting stdin receives handshake then prompt in order for fresh and resumed launches.