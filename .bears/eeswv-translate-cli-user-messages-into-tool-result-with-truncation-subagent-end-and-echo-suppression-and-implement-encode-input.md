---
id: eeswv
title: Translate CLI user messages into tool_result with truncation, subagent_end and echo suppression, and implement encode_input
status: open
priority: P1
created: "2026-09-16T20:29:00.019662055Z"
updated: "2026-09-16T20:29:00.019662055Z"
tags:
  - orchestrator
  - agent
depends_on:
  - xnacj
parent: "8vnwy"
---

## Summary
Complete the Claude translator's `user` branch and the stdin side of the adapter. CLI-produced `user` messages carry tool results: each `tool_result` block becomes a `tool_result` event, truncated at 256 KiB with `truncated: true`, and a result for an open subagent tool call additionally emits `subagent_end`. The CLI's echo of a message the orchestrator wrote is recognised by content hash and dropped. `encode_input` serialises a `message` or `answer` into the one-line SDK user-message shape the owner writes to stdin.

## Documents
- `SPEC.md` "AgentEvent" (`user` rule: `tool_result` per block, `subagent_end`, echo dropped by content hash; `tool_result.content: string | unknown`, `is_error`, `truncated`), "WebSocket: session stream" (`SessionInput`).
- `ARCHITECTURE.md` "Input encoding" (exact stdin JSON shape; the owner records `user_message` at write time), "Session owner task" step 2.
- `docs/data-model.md` `events` (256 KiB rule: truncated in the payload, full text stays in the transcript file).
- `docs/open-questions.md` items 1 and 3 (stdin shape; whether `answer` can occur) are verified by the probe task; this task implements the documented shape.
- ADRs 0008, 0027.

## Acceptance criteria
- [ ] `{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":..,"content":..,"is_error":..}, ...]},"parent_tool_use_id":..}` yields one `tool_result { tool_use_id, content, is_error: is_error or false, truncated }` per block, in order, each carrying `parent_tool_use_id` when present.
- [ ] `content` is kept as a string when native is a string and as the native JSON value otherwise; when the string, or the serialised JSON of a non-string value, exceeds `TOOL_RESULT_MAX_BYTES` (256 KiB) the stored `content` is the first 256 KiB of that text cut at a UTF-8 char boundary and `truncated: true`; otherwise `truncated: false`.
- [ ] A `tool_result` whose `tool_use_id` is in `state.open_subagents` additionally yields `subagent_end { tool_use_id, is_error }` after the `tool_result` and removes the id from the set.
- [ ] A `user` message whose content is a string, or consists only of `text` blocks, is hashed (SHA-256 of the concatenated text) and, when the hash is in `state.sent_input_hashes`, yields nothing and removes the hash; when the hash is not present it yields `raw { native: <message> }`.
- [ ] Mixed content (text and tool_result blocks) translates the tool_result blocks and ignores the text blocks; other block types yield `raw` per block.
- [ ] `ClaudeBackend::encode_input(&SessionInput::Message { text })` returns exactly `{"type":"user","message":{"role":"user","content":[{"type":"text","text":"<text>"}]}}` followed by a single `\n`, using compact serde output; `Answer { reply_to, text }` returns the same shape (the CLI has no distinct answer message under `--permission-prompts none`; `reply_to` is orchestrator bookkeeping only) unless the probe task documents a different shape, in which case that task changes this function.
- [ ] `encode_input` returns `Err(Error::BadRequest("input text must not be empty"))` for an empty or whitespace-only `text`, and the produced line contains no raw newline (serde escapes them).
- [ ] `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` pass.

## Implementation notes
- Files: `orchestrator/src/agent/claude/translate.rs` (`user` branch), `orchestrator/src/agent/claude/input.rs` (`encode_input`, `pub(crate) fn user_message_line(text: &str) -> String`), `orchestrator/src/agent/claude/native.rs` (`NativeUser`, `NativeToolResultBlock`).
- Hashing uses `sha2::Sha256` over the UTF-8 bytes of the text; the owner calls `TranslateState::record_sent_input(text)` with the same text it passes to `encode_input` (Session lifecycle epic), so both sides hash identical bytes.
- Truncation helper `pub(crate) fn truncate_content(content: Value) -> (Value, bool)` in `translate.rs`, reused by nothing else but unit-tested on its own.
- Do not attempt any secret redaction on `content` (ADR 0027).

## Edge cases
- `tool_result.content` absent: store `""` with `truncated: false`.
- A `user` message with `parent_tool_use_id` and text-only content is a subagent's prompt echo, not the orchestrator's message: its hash will not match, so it becomes `raw` (accepted; the frontend renders `raw` collapsed).
- A `subagent_end` for an unknown id is not emitted; a subagent whose result never arrives leaves the id in `open_subagents` for the life of the process (no leak concern: bounded by tool calls per process).
- Truncation cuts at `is_char_boundary`; never panics on multi-byte text.
- `encode_input` must never include `session_id` unless the probe task records that the CLI requires it (open question 1).

## Testing
- Unit tests: single and multiple `tool_result` blocks with string and array content; `is_error` default; 300 KiB string truncated to exactly 262144 bytes ending on a char boundary with `truncated: true`; 300 KiB array content truncated to its serialised text; `subagent_end` emitted after `subagent_start` was recorded (drive `xnacj`'s assistant branch first in the same test) and not for unknown ids; echo suppressed when `record_sent_input` was called with the same text and `raw` otherwise; mixed content; `encode_input` exact string for a message with quotes, newline and unicode; empty text rejected; `answer` shape.
- `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- none: implements the documented contract as written (the `answer` encoding fallback is provisional until the probe task resolves open question 3).

## Assumes from other epics
- "Session lifecycle: launcher, owner, recovery and sessions API": the owner records `user_message`, calls `record_sent_input(text)`, then writes the `encode_input` line to stdin.