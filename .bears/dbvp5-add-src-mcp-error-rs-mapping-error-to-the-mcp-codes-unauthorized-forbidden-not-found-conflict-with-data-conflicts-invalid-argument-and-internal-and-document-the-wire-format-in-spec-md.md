---
id: dbvp5
title: Add src/mcp/error.rs mapping Error to the MCP codes unauthorized, forbidden, not_found, conflict (with data.conflicts), invalid_argument and internal, and document the wire format in SPEC.md
status: done
priority: P0
created: "2026-09-16T20:42:42.812462649Z"
updated: "2026-09-20T06:39:31.255586794Z"
tags:
  - orchestrator
  - mcp
  - docs
depends_on:
  - "8a6qd"
parent: qgj33
attempts: 1
---

## Summary
Define how a tool failure travels over MCP. `src/mcp/error.rs` turns the prelude `Error` (and git conflicts carrying paths) into an `rmcp::model::ErrorData` JSON-RPC error whose `data.code` is one of the six documented string codes, whose `message` is the documented user-facing message, and whose `data.conflicts` lists conflicting paths for git conflicts. Internal errors are logged and answered with a generic message. Because `SPEC.md` names the codes but not the JSON-RPC numbers or the `data` layout, this task writes that wire format back into `SPEC.md` "MCP tool contracts".

## Documents
- `SPEC.md` "MCP tool contracts" (last paragraph: error codes `unauthorized` bad token, `forbidden` tool not in profile, `not_found`, `conflict`, `invalid_argument`, `internal`; `claim` → `conflict` "task is not claimable" / "task is not in a state this profile serves"; `update` → `invalid_argument` with the valid state names; `merge` → `conflict` with `data: { conflicts: string[] }`; `push` non-fast-forward → `conflict`).
- `ARCHITECTURE.md` "Orchestrator internals" → "Errors" (the `Error` enum variants; "MCP tool handlers map `Error` to MCP error codes in `src/mcp/error.rs`"; internal errors logged with `tracing::error!` and answered generically).
- `SPEC.md` "Git" (409 for non-fast-forward and stale/unapproved hand-offs, 422 with `conflicts` for merge/rebase conflicts, 400 for bad ref kinds) — the HTTP statuses that drive the mapping.
- `CLAUDE.md` rule 3 (no secrets in error messages).

## Acceptance criteria
- [ ] `orchestrator/src/mcp/error.rs` defines `#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum McpErrorCode { Unauthorized, Forbidden, NotFound, Conflict, InvalidArgument, Internal }` with `pub fn as_str(self) -> &'static str` (`unauthorized`, `forbidden`, `not_found`, `conflict`, `invalid_argument`, `internal`) and `pub fn json_rpc_code(self) -> i32`: `invalid_argument` = -32602 (JSON-RPC invalid params), `internal` = -32603 (JSON-RPC internal error), `unauthorized` = -32001, `forbidden` = -32003, `not_found` = -32004, `conflict` = -32009.
- [ ] `pub struct McpError { pub code: McpErrorCode, pub message: String, pub conflicts: Option<Vec<String>> }` with constructors `McpError::conflict(msg)`, `::invalid_argument(msg)`, `::forbidden(msg)`, `::not_found(msg)`, `::internal()` (fixed message `internal error`), `::conflict_with_paths(msg, paths)`, and `impl From<McpError> for rmcp::model::ErrorData` producing `ErrorData { code: ErrorCode(json_rpc_code), message, data: Some({ "code": "<as_str>" [, "conflicts": [...]] }) }`. `pub type McpResult<T> = Result<T, McpError>`.
- [ ] `impl From<Error> for McpError` maps by the HTTP status the prelude `Error` already produces (`Error::status_code()`; add that accessor to `prelude/error.rs` if `IntoResponse` computes it inline): 400 → `invalid_argument` with the error's message; 401 → `unauthorized`; 403 → `forbidden`; 404 → `not_found` (message `not found` unless the variant carries one); 409 and 422 → `conflict` with the message, and `conflicts` filled from `Error::conflicts() -> Option<&[String]>` (add this accessor; it returns `Some` only for the git conflict variants that the git REST mapping answers with 422 `{ conflicts }`); any 5xx → `internal` after `tracing::error!(error = ?err)`; model validation errors follow their existing 400 mapping → `invalid_argument`.
- [ ] Internal errors never expose the source message: a test converts `Error::Internal("database exploded")` and asserts the MCP message is exactly `internal error` and `data` has no other keys.
- [ ] Unit tests for every mapping row above, for `data.conflicts` order preservation, and for `ErrorData` serialisation (`serde_json::to_value`) matching the documented shape byte for byte on one example.
- [ ] `SPEC.md` "MCP tool contracts" last paragraph is extended with the wire format: tool failures are JSON-RPC errors; the table of string code → numeric code above; `data.code` always present; `data.conflicts` present only on git merge/rebase conflicts; the message texts quoted elsewhere in the section are the `message` field. Keep it to one short table plus two sentences.
- [ ] `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Implementation notes
- Files: `orchestrator/src/mcp/error.rs`, `orchestrator/src/mcp/mod.rs` (re-export `McpError`, `McpResult`), `orchestrator/src/prelude/error.rs` (`status_code()`, `conflicts()` accessors if missing), `SPEC.md`.
- Handlers return `McpResult<T>`; the dispatcher (next task) converts to `ErrorData` at the boundary. Tools that need a specific message (`claim`, `update`) construct `McpError` directly instead of going through `Error`.
- Do not use `CallToolResult { is_error: true }` for these failures: the documented contract is an MCP error object with a code, and the numeric/`data` layout written here is what the frontend-facing docs will quote.
- `ErrorData` construction: `rmcp::model::ErrorData::new(ErrorCode(code), message, Some(data))`; confirm the constructor name against the pinned `rmcp` version.

## Edge cases
- A `GitError` whose HTTP mapping is 422 but carries no paths (should not exist) → `conflict` without `conflicts`.
- `Error::Conflict(String)` messages that already contain the documented text are passed through unchanged; never prefix or wrap.
- Messages must never contain credential values or stderr from git: the git epic already guarantees that its user-facing messages are generic; do not add source-chain formatting (`{:#}`) into the message.

## Testing
- Unit tests in `error.rs` as listed; no database needed.
- Command: `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- `SPEC.md` "MCP tool contracts": wire-format table (numeric codes, `data.code`, `data.conflicts`) added in the same commit.

## Assumes from other epics
- "Git operations: mirror, clones, integration and REST API": `GitError` variants with their 400/409/422 HTTP mapping and the conflicting-path list (task "Add the git command wrapper, GitError and its HTTP error mapping").
- "Repository scaffolding, tooling and CI" / "Database schema, models, repositories and test harness": the prelude `Error` enum and `IntoResponse`.