---
id: "32s57"
title: Define AgentEvent and SessionInput types with the storage and API serialisation contract
status: open
priority: P0
created: "2026-09-16T20:26:41.546633971Z"
updated: "2026-09-16T20:51:51.937571055Z"
tags:
  - orchestrator
  - agent
  - core
depends_on:
  - deex5
  - p5tsd
parent: "8vnwy"
---

## Summary
Deliver the Rust types in `orchestrator/src/events/` that every backend translates into and that the session owner, WebSocket, REST and frontend consume: `AgentEvent` (every kind in `SPEC.md` "AgentEvent"), the base fields `parent_tool_use_id` and `message_id`, the `SessionInput` enum (`message`, `answer`), and the exact mapping between an event and an `events` row (`kind` and `payload` columns, `_offset` inside the payload, `_`-prefixed keys stripped on read). Nothing in this task talks to the database; it defines the serde contract the repository primitive and the translator build on.

## Documents
- `SPEC.md` "AgentEvent" (the full `AgentEvent` union and `AgentEventBase`), "WebSocket: session stream" (`SessionInput` shape).
- `docs/data-model.md` `events` (columns `seq`, `ts`, `kind`, `payload`; kinds are `TEXT` owned by the Rust types; 256 KiB tool-result row-size rule).
- `ARCHITECTURE.md` "Durability and recovery" (`_offset` in the payload), "Session owner task".
- ADR 0008.

## Acceptance criteria
- [ ] `events::AgentEvent` has exactly the kinds `init`, `user_message`, `text_delta`, `text`, `thinking`, `tool_call`, `tool_result`, `permission_denied`, `prompt`, `subagent_start`, `subagent_end`, `result`, `error`, `state_change`, `launch_warning`, `git`, `raw`, each with the fields and optionality of `SPEC.md`; serde uses `kind` as the tag in `snake_case`.
- [ ] Base fields `parent_tool_use_id: Option<String>` and `message_id: Option<String>` are available on every kind and are omitted from JSON when `None`.
- [ ] `state_change.signal` serialises as `"SIGINT"` / `"SIGTERM"`; `git.op` as `sync|merge|rebase|push`; `raw.backend` as `"claude"`.
- [ ] A stored form `SessionEvent { seq: i64, ts: DateTime<Utc>, event: AgentEvent }` serialises flat, exactly as the TS `AgentEvent` interface (`seq`, `ts` RFC 3339, `kind`, then the kind's fields and the base fields).
- [ ] `AgentEvent::into_row_parts(&self, offset: Option<u64>) -> (String, serde_json::Value)` returns the `kind` string and a payload object without `kind`, `seq` or `ts` and with `_offset` when `offset` is `Some`.
- [ ] `SessionEvent::from_row(seq, ts, kind: &str, payload: Value) -> Result<SessionEvent>` reconstructs the event, removes every top-level payload key starting with `_` before deserialising, and returns `Error::Internal` (logged with `tracing::error!`, structured fields only) for an unknown kind or a payload that does not match the kind.
- [ ] `events::SessionInput` is `#[serde(tag = "kind")]` with `Message { text }` and `Answer { reply_to: i64, text }`.
- [ ] `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` pass.

## Implementation notes
- Files: `orchestrator/src/events/mod.rs` (re-exports), `orchestrator/src/events/agent_event.rs`, `orchestrator/src/events/input.rs`. `TaskEvent` (the tracker epic) lives beside these in `events/task_event.rs` later; leave room, do not define it here.
- Suggested shape: `pub struct AgentEvent { #[serde(skip_serializing_if = "Option::is_none")] pub parent_tool_use_id: Option<String>, #[serde(skip_serializing_if = "Option::is_none")] pub message_id: Option<String>, #[serde(flatten)] pub body: AgentEventBody }` with `#[serde(tag = "kind", rename_all = "snake_case")] pub enum AgentEventBody { Init { cli_session_id: String, model: Option<String>, tools: Vec<String>, mcp_servers: Vec<McpServerStatus>, resumed: bool }, UserMessage { text: String, user_id: Option<Uuid>, client_id: Option<String>, reply_to: Option<i64> }, TextDelta { text }, Text { text }, Thinking { text, redacted: bool }, ToolCall { tool_use_id, name, input: Value }, ToolResult { tool_use_id, content: Value, is_error: bool, truncated: bool }, PermissionDenied { tool_use_id: Option<String>, name, reason }, Prompt { prompt_id, text, options: Option<Vec<String>> }, SubagentStart { tool_use_id, description, agent_type: Option<String> }, SubagentEnd { tool_use_id, is_error }, Result { subtype, is_error, num_turns: i64, duration_ms: i64, cost_usd: Option<f64>, usage: Option<Value>, permission_denials: Vec<Value> }, Error { message, fatal }, StateChange { from: SessionState, to: SessionState, reason, signal: Option<StopSignal> }, LaunchWarning { message }, Git { op: GitOp, ok, detail: Value }, Raw { backend: Backend, native: Value } }`.
- `McpServerStatus { name: String, status: String }`; `StopSignal { Sigint, Sigterm }` with `#[serde(rename = "SIGINT")]` etc.; `Backend` is the `agent_backend` enum (`claude`) shared with `models`.
- `user_id` is `string | null` in the spec: serialise `None` as `null` (no `skip_serializing_if`) for that field; `client_id` and `reply_to` are omitted when absent.
- Provide `AgentEvent::kind(&self) -> &'static str` for log fields and repository calls.
- Helpers: `pub const TOOL_RESULT_MAX_BYTES: usize = 256 * 1024;` lives here so the translator and the row-size rule share one constant.
- No SQL, no `AppState`, no locking: the append primitive (Database schema epic) takes `(kind, payload)` from `into_row_parts`; the owner (Session lifecycle epic) decides the `offset`.

## Edge cases
- `from_row` must strip any `_`-prefixed key, not only `_offset`, so future internal fields never leak to clients.
- `usage` and `detail` are opaque `serde_json::Value`; never validate their content.
- Kinds are `TEXT` in the database: adding a variant later must not need a migration; do not derive an exhaustive `sqlx::Type`.
- Deserialising a `state_change` whose `SessionState` value is unknown is an internal error, not a panic.
- Keep the types free of any redaction logic (ADR 0027): the values pass through verbatim.

## Testing
- Unit tests in the module: one serde round-trip per kind against a literal JSON string matching `SPEC.md` field names; `into_row_parts` puts `_offset` in the payload and excludes `kind`; `from_row` strips `_offset` and an extra `_future` key; unknown kind and mismatched payload return `Err`; `SessionInput` parses `{"kind":"message","text":"hi"}` and `{"kind":"answer","reply_to":7,"text":"yes"}` and rejects an unknown kind.
- `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- none: implements the documented contract as written. If a field name has to differ from `SPEC.md` for a serde reason, change `SPEC.md` "AgentEvent" in the same commit instead of diverging.

## Assumes from other epics
- "Database schema, models, repositories and test harness": `models::session::SessionState` enum (`creating|running|parked|done|failed`) and the `agent_backend` enum type; the event-append repository primitive taking `(kind, payload)`.
- "Repository scaffolding, tooling and CI": the `events/` module stub, `prelude::{Error, Result}`.