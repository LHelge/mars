---
id: h9u3c
title: "Implement profile-aware tools/list and tools/call dispatch: tracker tools always, git tools only when in profile.mcp_tools, forbidden for unlisted tools, invalid_argument for bad input"
status: open
priority: P0
created: "2026-09-16T20:44:00.701270365Z"
updated: "2026-09-16T20:44:00.701270365Z"
tags:
  - orchestrator
  - mcp
depends_on:
  - "4t5n5"
  - dbvp5
  - "7rf9z"
parent: qgj33
---

## Summary
Implement `McpServer::list_tools` and `McpServer::call_tool`. Listing returns the eight tracker tools always and each git tool only when its name is in the calling profile's `mcp_tools`, with the verbatim descriptions and generated input schemas. Calling resolves the `SessionContext`, refuses unknown names with `invalid_argument` and unlisted git tools with `forbidden`, deserialises the arguments into the typed input, dispatches to the per-tool handler module, and returns the handler's JSON as the tool result or its `McpError` as the JSON-RPC error. Handlers are stubs here and are filled in by the tool tasks.

## Documents
- `ARCHITECTURE.md` "MCP design" → "Tool exposure" (`tools/list` contains the task-tracker tools always and the git tools only if named in `mcp_tools`; a call to an unlisted tool returns an MCP error, not a silent no-op; descriptions from `SPEC.md`), "Authentication" (handlers never take a session id).
- `SPEC.md` "MCP tool contracts" intro ("Tools are listed to a session only if allowed for its profile; task tools are always allowed"), the `(git; profile-gated)` headings, error codes (`forbidden` = tool not in profile, `invalid_argument`).
- `docs/data-model.md` `agent_profiles.mcp_tools` ("Names of MCP tools this profile may call. Empty means the task-tracker set only").
- ADR 0007 (git tools restricted per profile; a future merge agent is a profile with `merge`, `rebase`, `push` and nothing else).

## Acceptance criteria
- [ ] `list_tools` returns `Vec<rmcp::model::Tool>` built from `ToolName::ALL` in order, skipping git tools whose `as_str()` is not contained in `ctx.profile.mcp_tools`; each `Tool` has `name = as_str()`, `description = Some(description())`, `input_schema` generated from the tool's input type with `schemars` (`EmptyInput` for `list_session_branches`), and `output_schema` from the output type when the pinned `rmcp` version supports it.
- [ ] `call_tool` order: resolve `SessionContext` (missing → `internal`); `ToolName::parse(name)` → `None` → `invalid_argument` with message `unknown tool: <name>`; a git tool not in `mcp_tools` → `forbidden` with message `tool <name> is not allowed for this profile` (logged at `warn` with `session_id` and `tool`); deserialise `arguments` (missing arguments = `{}`) into the input type, serde failure → `invalid_argument` with message `invalid arguments: <serde message>`; dispatch; success → `CallToolResult` with `structured_content = Some(value)` and one `Content::text(serde_json::to_string(&value))` block so clients without structured-content support still see the JSON; `McpError` → `Err(ErrorData)` through the error task's conversion.
- [ ] Dispatch table in `orchestrator/src/mcp/tools/mod.rs`: `pub async fn dispatch(state: &AppState, ctx: &SessionContext, tool: ToolName, args: serde_json::Value) -> McpResult<serde_json::Value>` matching on `ToolName` and calling `ready::handle(state, ctx, input)`, `claim::handle(...)`, ... `push::handle(...)`; every handler has the signature `pub async fn handle(state: &AppState, ctx: &SessionContext, input: <InputType>) -> McpResult<<OutputType>>` and, until implemented, returns `McpError::internal()` after `tracing::error!(tool = name, "tool not implemented")`.
- [ ] Tracker tool names appearing in `mcp_tools` are ignored (they are always listed); `mcp_tools` entries that name no tool are ignored here (profile validation is the projects epic's concern).
- [ ] Every call logs one `debug` line `mcp tool call` with `session_id = %ctx.session_id`, `project_id = %ctx.project_id`, `tool = name` and, on completion, `ok = bool` and `code = <as_str>` on failure; arguments and results are never logged at any level above `trace`.
- [ ] Concurrency: `McpServer` holds no per-call mutable state; concurrent calls from the same MCP session are independent (`rmcp` may deliver them concurrently).
- [ ] `tests/mcp_dispatch.rs`: a profile with `mcp_tools = []` lists exactly `ready, claim, get_task, update, release, comment, needs_human, create_task` in that order with descriptions equal to `ToolName::description()`; `mcp_tools = ["merge", "push"]` lists ten tools ending in `merge, push`; `mcp_tools = ["list_session_branches", "merge", "rebase", "push"]` lists all twelve; calling `rebase` with `mcp_tools = ["merge"]` → error with `data.code == "forbidden"` and the documented message; calling `no_such_tool` → `invalid_argument` `unknown tool: no_such_tool`; calling `ready` with `{ "limit": "twenty" }` → `invalid_argument` starting with `invalid arguments:`; calling `claim` with `{ "task": 12.5 }` → `invalid_argument`; calling a stubbed tool with valid input → `internal` with message `internal error` (this test is replaced as tools land); every error response carries `data.code`.
- [ ] `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Implementation notes
- Files: `orchestrator/src/mcp/server.rs` (`ServerHandler::list_tools`, `call_tool`), `orchestrator/src/mcp/tools/mod.rs` (dispatch, one `pub mod` per tool: `ready`, `claim`, `get_task`, `update`, `release`, `comment`, `needs_human`, `create_task`, `list_session_branches`, `merge`, `rebase`, `push`), `orchestrator/tests/mcp_dispatch.rs`.
- Do not use the `#[tool_router]`/`#[tool]` macros: they produce a static tool list and cannot filter per request. Hand-written `list_tools`/`call_tool` over `ToolName` is the documented behaviour.
- Schema generation: `schemars::schema_for!(T)` converted to the `Arc<JsonObject>` that `rmcp::model::Tool` expects; cache the twelve schemas in a `OnceLock` so listing does no repeated work.
- Tool name matching against `mcp_tools` is exact and case-sensitive.
- The profile in `SessionContext` is loaded per request by the middleware, so a profile edit (new `mcp_tools`) takes effect on the agent's next call without a relaunch; the CLI may cache `tools/list`, which is acceptable (a call to a newly allowed tool succeeds even if the list is stale).

## Edge cases
- `arguments` given as a JSON array or scalar → `invalid_argument`.
- Extremely large argument bodies: rely on the `rmcp` transport's body limit; do not add another.
- A handler panic must not kill the listener: wrap dispatch in `tokio::task::spawn` + `JoinHandle` error mapping to `internal` only if `rmcp` does not already isolate handler panics (check once; document the finding in a comment).

## Testing
- `tests/mcp_dispatch.rs` as above, using `seed_mcp_session` with profiles created through `ProjectRepository` and `mcp_tools` set directly.
- Command: `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Projects, agent profiles and shared directories": `AgentProfile.mcp_tools: Vec<String>` and a way to create profiles with a chosen `mcp_tools` in tests.