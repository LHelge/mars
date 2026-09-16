---
id: "4c387"
title: Implement Claude translation of system, result and unknown lines including the fatal authentication error
status: open
priority: P1
created: "2026-09-16T20:28:05.957687193Z"
updated: "2026-09-16T20:28:05.957687193Z"
tags:
  - orchestrator
  - agent
depends_on:
  - w8ezk
parent: "8vnwy"
---

## Summary
Create the Claude translator module and implement the line-level dispatch plus the rules that do not depend on assistant content: `system`/`init` to `init`, `system`/`permission_denied` to `permission_denied`, `result` to `result` (with `cost_usd` from `total_cost_usd` and one `permission_denied` per new entry in `permission_denials`), authentication failure to a fatal `error` naming the injected secret and its scope, and everything unrecognised (including non-JSON lines) to `raw`. Later tasks add the assistant and user branches to the same dispatcher.

## Documents
- `SPEC.md` "AgentEvent" (translation rules for `system`/`init`, `system`/`permission_denied`, `result`, "anything else → raw"; `parent_tool_use_id` propagation).
- `ARCHITECTURE.md` "Claude Code invocation" (credentials paragraph: fatal `error` naming `ANTHROPIC_API_KEY` or `CLAUDE_CODE_OAUTH_TOKEN` and its scope; denials arrive as `permission_denied` system messages and in `result.permission_denials`, both translated), "Cost accounting" (`total_cost_usd`, `usage` carried on `result`).
- `docs/open-questions.md` item 6 (`init` may or may not carry `model` and `tools`).
- ADR 0008; `CLAUDE.md` rule 3.

## Acceptance criteria
- [ ] `ClaudeBackend::translate(line, state)` parses the line as JSON; a line that is not a JSON object yields one `raw { backend: "claude", native: <the line as a JSON string> }`; an object with an unknown `type` yields `raw { native: <object> }`.
- [ ] `{"type":"system","subtype":"init",...}` yields `init { cli_session_id: session_id, model: model?, tools: tools or [], mcp_servers: [{name, status}] or [], resumed: state.resumed }`; a missing `session_id` yields `raw` (the owner cannot proceed without it) plus a `tracing::warn!` with `kind = "init"`.
- [ ] `{"type":"system","subtype":"permission_denied",...}` yields `permission_denied { tool_use_id?, name, reason }` and records `tool_use_id` in `state.denied_tool_use_ids`.
- [ ] `{"type":"result",...}` yields `result { subtype, is_error, num_turns, duration_ms, cost_usd: total_cost_usd?, usage?, permission_denials: [] or the native array }` followed by one `permission_denied` per entry of `permission_denials` whose `tool_use_id` is not already in `state.denied_tool_use_ids` (entries without an id are always emitted).
- [ ] A `result` (or `system`) line whose error text indicates authentication failure (case-insensitive match on any of `invalid api key`, `authentication_error`, `not logged in`, `please run /login`, `401`; the exact native shape is confirmed by the live probe and captured as a fixture) yields, after the `result`, `error { message, fatal: true }` where `message` is `Authentication failed with <NAME> (<scope> scope); replace the secret and send the next message.` when `state.credential` is `Some`, else `Authentication failed: no ANTHROPIC_API_KEY or CLAUDE_CODE_OAUTH_TOKEN was injected.`; the message never contains a secret value.
- [ ] Every emitted event copies the native line's top-level `parent_tool_use_id` when present.
- [ ] Every other `system` subtype yields `raw`.
- [ ] `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` pass.

## Implementation notes
- Files: `orchestrator/src/agent/claude/translate.rs` (`pub(crate) fn translate_line(line: &str, state: &mut TranslateState) -> Vec<AgentEvent>` with a `match native.get("type")` dispatcher; branches for `assistant`, `user`, `stream_event` are stubs returning `raw` until the next tasks fill them), `orchestrator/src/agent/claude/native.rs` (serde structs for the native shapes: `NativeSystemInit`, `NativePermissionDenied`, `NativeResult`, all `#[serde(default)]`-tolerant with `#[serde(flatten)] extra: Map<String, Value>` so unknown fields never fail parsing).
- Use `serde_json::from_str::<Value>` first, then typed extraction; never `unwrap`.
- `cost_usd` is `f64`; `usage` is passed through as `Value` untouched. Accumulation into `sessions.cost_usd` etc. is the owner's job (Session lifecycle epic).
- `permission_denied.reason`: native `message`/`reason` field, or `"denied"` when absent; `name`: native `tool_name` or `tool` or `"unknown"`.
- Logging: `tracing::debug!(kind = %event.kind())` only; never log the line or payload at `info` or above.

## Edge cases
- An empty line or whitespace-only line yields no events (the tail may deliver one at EOF).
- A JSON array or scalar line is `raw` with the value as `native`.
- `result` with `is_error: true` but no authentication text is just `result`; the owner decides state.
- Duplicate `permission_denied` for the same `tool_use_id` (system message then `result` list) is emitted once.
- `resumed` is read from the state, not the native line.

## Testing
- Unit tests in `translate.rs` with inline native lines: init with and without `model`/`tools`; init without `session_id`; permission_denied system message; result with cost and usage, with and without `permission_denials`, with a denial already seen; result with `Invalid API key` and `credential = Some(ANTHROPIC_API_KEY, project)` asserting the exact message and `fatal: true`; result with auth error and no credential; non-JSON line; unknown type; `parent_tool_use_id` copied onto `result`.
- `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- none: implements the documented contract as written (the `permission_denials` dedupe rule is an implementation detail consistent with "both are translated").

## Assumes from other epics
- none.