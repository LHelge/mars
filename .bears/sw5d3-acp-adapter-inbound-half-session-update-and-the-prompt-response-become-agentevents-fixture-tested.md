---
id: sw5d3
title: "ACP adapter, inbound half: session/update and the prompt response become AgentEvents, fixture-tested"
status: open
priority: P1
created: "2026-09-21T12:21:20.758025Z"
updated: "2026-09-21T12:21:20.758025Z"
tags:
  - orchestrator
  - agent
  - acp
  - events
depends_on:
  - huegh
  - "3qa5e"
parent: eydgf
---

## Summary
The translation of ADR 0008 for ACP: one native line in, zero or more `AgentEvent`s out, pure and fixture-tested.

## Documents
- `SPEC.md`, "AgentEvent": a mapping table for ACP beside the Claude rules; any additive field. `ARCHITECTURE.md`, "ACP adapter".

## Acceptance criteria
- [ ] Mapping:
  - `agent_message_chunk` → `text_delta` while streaming and one `text` when the message is complete (chunk boundary rule from the spike: a different update kind or the prompt response closes a message); with `partial_messages` false only the whole `text` is emitted.
  - `agent_thought_chunk` → `thinking` (coalesced the same way).
  - `tool_call` → `tool_call` (id = `toolCallId`, name = `title`/`kind` as the spike shows agents fill them, input = `rawInput`); `tool_call_update` with a terminal status → `tool_result` (`is_error` from `failed`), intermediate updates produce nothing or refresh state.
  - `plan` and any unknown update → `raw` (nothing is dropped).
  - user message echoes and a `session/load` history replay → nothing (Mars already has those events); the replay is recognised as "updates before the first prompt of this process".
  - the `session/prompt` response → `result`: `subtype`/`terminal_reason` from `stopReason` (`cancelled` must map to what the owner treats as a user stop, not a failure — `ARCHITECTURE.md`, "Stop semantics"), `is_error`, `duration_ms` measured by the adapter, `num_turns: 1`, `usage` with `input_tokens`/`output_tokens` keys and `cost_usd` read from the standard field or Curiosity's `_meta` key.
  - a JSON-RPC error response to a prompt → `error` + a closing `result` with `is_error: true`, so a turn always closes.
- [ ] `parent_tool_use_id` is absent (ACP has no subagents); nothing in the reducer depends on it being present.
- [ ] Tool results larger than the event limit follow the existing truncation rule.
- [ ] Fixtures: `orchestrator/tests/fixtures/acp/curiosity/<version>/*.jsonl` with expected `AgentEvent` sequences beside them, recorded from the real binary with the mock model (no credentials in fixtures); plus the OpenCode recordings of the spike as a second dialect the translator must not choke on (expected output may contain `raw`).

## Implementation notes
- `agent/acp/translate.rs`; test harness as `tests/` uses for `fixtures/claude/`.
- If Curiosity is not yet able to produce recordings when this task starts, hand-write fixtures from the ACP schema and replace them in the end-to-end epic; say so in the fixture `NOTES.md`.

## Testing
- Fixture tests, unit tests per mapping rule. Full backend chain.