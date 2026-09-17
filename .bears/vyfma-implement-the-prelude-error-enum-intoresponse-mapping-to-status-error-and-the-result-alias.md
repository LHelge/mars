---
id: vyfma
title: Implement the prelude Error enum, IntoResponse mapping to {status, error} and the Result alias
status: done
priority: P0
created: "2026-09-16T20:27:09.564623059Z"
updated: "2026-09-17T05:25:00.807557374Z"
tags:
  - orchestrator
  - core
depends_on:
  - jeyrc
parent: sywed
assignee: claude-opus-subagent
attempts: 1
---

## Summary
Add the crate-wide error contract to `src/prelude/error.rs`: one `Error` enum with the generic variants and the `sqlx::Error` conversion, `impl IntoResponse` producing the `{ "status": <u16>, "error": "<message>" }` body with the matching HTTP status, generic messages for internal failures logged through `tracing::error!`, and `pub type Result<T> = std::result::Result<T, Error>`. Later epics add their `#[from]` variants (`ClaimsError`, model errors, `EngineError`, `GitError`, `SecretsError`, `EmailError`) into this enum; the mapping rules and body shape are fixed here.

## Documents
- `ARCHITECTURE.md` "Orchestrator internals", **Errors** paragraph: variant list, `IntoResponse` mapping, generic internal message, `Result<T>` alias, MCP mapping lives in `src/mcp/error.rs` (MCP epic).
- `SPEC.md` "REST API": error body shape, the status table (400 validation, 401 missing/invalid token, 403 forbidden, 404 unknown, 409 conflict, 422 git conflicts with `conflicts: string[]`, 429 throttled, 500 never carries internal detail).
- `CLAUDE.md` "Backend conventions": "Functions return `Result`; `unwrap`/`expect` only in tests and at startup. Internal errors log with `tracing::error!` and return a generic message." "API conventions": errors are `{ "status", "error" }`; git conflicts additionally include `conflicts`.

## Acceptance criteria
- [ ] `orchestrator/src/prelude/error.rs` defines `#[derive(Debug, thiserror::Error)] pub enum Error` with at least: `#[error("not found")] NotFound`, `#[error("forbidden")] Forbidden`, `#[error("{0}")] Conflict(String)`, `#[error("{0}")] BadRequest(String)`, `#[error("{0}")] Unauthorized(String)` (message `authentication required` is the conventional value; the auth epic decides per call site), `#[error("too many requests")] Throttled`, `#[error("{0}")] Internal(String)`, `#[error("git conflict")] GitConflict { message: String, conflicts: Vec<String> }`, and `#[error(transparent)] Database(#[from] sqlx::Error)`.
- [ ] Status mapping in `impl IntoResponse for Error`: `BadRequest` → 400, `Unauthorized` → 401, `Forbidden` → 403, `NotFound` → 404, `Conflict` → 409, `GitConflict` → 422 with body `{ "status": 422, "error": <message>, "conflicts": [...] }`, `Throttled` → 429, `Internal` and `Database` → 500 with body `{ "status": 500, "error": "internal error" }`.
- [ ] `sqlx::Error::RowNotFound` maps to 404 `not found` (so repositories can use `fetch_one` and let the conversion do the work); every other `sqlx::Error` is 500.
- [ ] 500 responses log `tracing::error!(error = %err, "internal error")` (or `?err` for the sqlx source) exactly once per response and the body never contains the source message; 4xx responses log at `debug` at most.
- [ ] `Error::status(&self) -> StatusCode` is public so the WebSocket/SSE/MCP layers can reuse the mapping without building an HTTP response.
- [ ] A private `#[derive(Serialize)] struct ErrorBody { status: u16, error: String, #[serde(skip_serializing_if = "Option::is_none")] conflicts: Option<Vec<String>> }` is the only JSON shape; `Content-Type: application/json`.
- [ ] `pub type Result<T> = std::result::Result<T, Error>;` and `prelude/mod.rs` re-exports `Error`, `Result`; `use crate::prelude::*` in every module now resolves `Result` (remove any placeholder from the skeleton task).
- [ ] `cd orchestrator && cargo fmt && cargo clippy --all-targets --features integration-tests -- -D warnings && cargo test --features integration-tests` pass.

## Implementation notes
- Files: `orchestrator/src/prelude/error.rs` (new), `orchestrator/src/prelude/mod.rs`.
- Add a doc comment on the enum listing the `#[from]` variants later epics are expected to add in place (`ClaimsError` → 401, `UserError`/`ProjectError`/`TaskError`/... → 400 or 409 per the model's own `status()` method, `EngineError` → 500 or 409, `GitError` → 500/422, `SecretsError` → 500, `EmailError` → 500) so each epic follows the same shape: each domain error type exposes `fn status(&self) -> StatusCode` and `Display` is the client-visible message.
- Also handle axum's own rejections so validation of JSON bodies returns the documented shape: implement a `Json<T>` extractor wrapper or use `axum::extract::rejection::JsonRejection` → `Error::BadRequest(rejection.body_text())` via `impl From<JsonRejection> for Error`; same for `PathRejection` and `QueryRejection`. Re-export the wrapper from the prelude if a wrapper is used, with a note in the enum doc comment.
- Do not implement `From<anyhow::Error>` (no `anyhow` in the crate list).

## Edge cases
- `Conflict` and `BadRequest` messages are client-facing: callers pass user-readable text (e.g. `password change required`, `duplicate name`), never `format!("{e:?}")` of an internal error.
- `Internal(String)` keeps the string for the log line only; the body is always `internal error`.
- A `sqlx::Error::Database` unique-violation is *not* automatically 409 here (the repositories decide per constraint in the Database epic); it is 500 by default.

## Testing
- Unit tests in `error.rs` using `axum::response::IntoResponse` and `axum::body::to_bytes`: one test per variant asserting status code and the exact JSON body; `GitConflict` includes `conflicts`; `Database(RowNotFound)` is 404; `Internal("secret detail")` body is `{"status":500,"error":"internal error"}` and does not contain `secret detail`; `JsonRejection` conversion yields 400.
- An `axum-test` router test (`tests/error_mapping.rs`, no database) with a handler returning `Err(Error::Conflict("duplicate name".into()))` asserting the wire response.
- Command: `cd orchestrator && cargo fmt && cargo clippy --all-targets --features integration-tests -- -D warnings && cargo test --features integration-tests`.

## Documentation
- none: implements `ARCHITECTURE.md` "Orchestrator internals", Errors, and `SPEC.md` "REST API" as written. If `Unauthorized`/`Throttled`/`GitConflict` are judged to be additions to the documented variant list, add them to the Errors paragraph in the same commit (they are implied by the SPEC status table).

## Assumes from other epics
- Authentication epic adds `ClaimsError` (`#[from]`) → 401.
- Database epic adds each model error and decides which constraint violations become 409.
- Git, Engine, Secrets, Email epics add their error types with `#[from]`.
- MCP epic maps `Error` to MCP error codes in `src/mcp/error.rs` using `Error::status`.