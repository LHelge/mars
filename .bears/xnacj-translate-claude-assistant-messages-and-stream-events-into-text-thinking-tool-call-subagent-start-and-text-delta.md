---
id: xnacj
title: Translate Claude assistant messages and stream events into text, thinking, tool_call, subagent_start and text_delta
status: open
priority: P1
created: "2026-09-16T20:28:28.543946403Z"
updated: "2026-09-16T20:28:28.543946403Z"
tags:
  - orchestrator
  - agent
depends_on:
  - "4c387"
parent: "8vnwy"
---

## Summary
Fill the `assistant` and `stream_event` branches of the Claude translator: one event per assistant content block (`text`, `thinking`, `tool_call`), an additional `subagent_start` when a `tool_call` names the subagent tool (`Task` or `Agent`, one constant), `message_id` from the native message id, `parent_tool_use_id` on every event, and `stream_event` text deltas to `text_delta` with every other stream event dropped.

## Documents
- `SPEC.md` "AgentEvent" (assistant rule, subagent tool constant, `stream_event` rule, `parent_tool_use_id` grouping, `message_id` on the base).
- `ARCHITECTURE.md` "Claude Code invocation" (`--forward-subagent-text`, subagent messages carry `parent_tool_use_id`; `--include-partial-messages` by profile flag).
- ADR 0008.

## Acceptance criteria
- [ ] `{"type":"assistant","message":{"id":..,"content":[...]},"parent_tool_use_id":..}` yields, in block order: `text { text }` for `text` blocks; `thinking { text: thinking, redacted: false }` for `thinking` blocks and `thinking { text: "", redacted: true }` for `redacted_thinking` blocks; `tool_call { tool_use_id: id, name, input }` for `tool_use` blocks; any other block type yields `raw { native: <block> }`.
- [ ] `pub(crate) const SUBAGENT_TOOL_NAMES: [&str; 2] = ["Task", "Agent"];` a `tool_use` whose `name` matches (exact, case-sensitive) additionally yields `subagent_start { tool_use_id, description: input.description or "", agent_type: input.subagent_type? }` immediately after the `tool_call`, and inserts `tool_use_id` into `state.open_subagents`.
- [ ] Every event from an assistant message carries `message_id = message.id` when present and `parent_tool_use_id` from the native top-level field when present.
- [ ] A `stream_event` whose `event.type == "content_block_delta"` and `event.delta.type == "text_delta"` yields `text_delta { text: delta.text }`; all other `stream_event`s (message_start, content_block_start, input_json_delta, thinking_delta, message_delta, message_stop, ...) yield nothing.
- [ ] An assistant message with an empty or missing `content` array yields nothing; a `content` given as a plain string yields one `text`.
- [ ] `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` pass.

## Implementation notes
- Files: `orchestrator/src/agent/claude/translate.rs` (branches), `orchestrator/src/agent/claude/native.rs` (`NativeAssistant`, `NativeContentBlock` as an internally tagged enum on `type` with an `Other(Value)` catch-all via `#[serde(other)]`-style handling or manual matching, `NativeStreamEvent`).
- `tool_call.input` is the native `input` object as `Value`, untouched (ADR 0027: no redaction).
- `text_delta` events are the only per-token rows; the complete `text` block follows in the assistant message, so the frontend replaces deltas with the block (Frontend session views epic). Do not try to suppress the final `text` when deltas were emitted.
- Keep the subagent bookkeeping (`open_subagents`) in `TranslateState` so the user-message task can emit `subagent_end`.

## Edge cases
- `message.id` absent on older/newer versions: `message_id = None`, never an error.
- A `tool_use` block without `id` yields `raw` for that block and a `tracing::warn!(block_type = "tool_use")`.
- `thinking` blocks may carry a `signature`; ignore it.
- `text` blocks with empty strings are still emitted (the frontend decides rendering).
- Assistant messages inside a subagent (`parent_tool_use_id` set) are translated identically; nesting is the frontend's job.

## Testing
- Unit tests: multi-block assistant message (text + thinking + tool_use) in order with `message_id`; `redacted_thinking`; `Task` and `Agent` tool calls both yield `subagent_start` with `description` and `agent_type`, other names do not; `parent_tool_use_id` copied onto every emitted event; string `content`; empty `content`; `content_block_delta`/`text_delta` yields `text_delta`; `input_json_delta`, `message_start`, `message_stop` yield nothing; unknown block type yields `raw`.
- `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- none.