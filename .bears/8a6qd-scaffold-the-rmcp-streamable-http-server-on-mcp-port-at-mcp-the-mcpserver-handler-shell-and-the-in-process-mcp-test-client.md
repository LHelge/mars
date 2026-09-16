---
id: "8a6qd"
title: Scaffold the rmcp Streamable HTTP server on MCP_PORT at /mcp, the McpServer handler shell and the in-process MCP test client
status: open
priority: P0
created: "2026-09-16T20:41:32.384628065Z"
updated: "2026-09-16T20:51:52.204993472Z"
tags:
  - orchestrator
  - mcp
  - tests
depends_on:
  - xjaah
parent: qgj33
---

## Summary
Create the `orchestrator/src/mcp/` module with an `rmcp` Streamable HTTP service mounted at `/mcp` on its own listener bound to `MCP_PORT` (default 7001), an `McpServer` type implementing `rmcp::ServerHandler` that advertises the `tools` capability and nothing else yet, the `SessionContext` type the bearer middleware will attach, and the `tests/common/mcp.rs` client helper every later MCP test uses. This is the transport skeleton; authentication, error mapping, listing and tools follow in their own tasks.

## Documents
- `ARCHITECTURE.md` "MCP design" (first paragraph: `rmcp` over Streamable HTTP on its own listener `MCP_PORT`, path `/mcp`; "Authentication": `SessionContext { session_id, project_id, profile }` attached to the request, handlers never take a session id).
- `ARCHITECTURE.md` "Orchestrator internals" (`mcp/` = "rmcp server, tool handlers, bearer auth"; crate table: `rmcp` server + Streamable HTTP transport; `AppState` contents), "Components" and "Networks" (the MCP listener binds on all interfaces, is never proxied by nginx, and the host does not publish its port).
- `README.md` "Configuration" (`MCP_PORT`, `MCP_URL`), "Development" (`cargo run` listens on `API_PORT` and `MCP_PORT`).
- `CLAUDE.md` "Testing expectations" ("MCP tests drive the tool handlers through the `rmcp` server in-process with a session bearer token from `TestApp`").

## Acceptance criteria
- [ ] `orchestrator/src/mcp/mod.rs` exports `pub fn mcp_router(state: AppState) -> axum::Router` which nests an `rmcp` `StreamableHttpService` at `/mcp`, built with a `LocalSessionManager` and a factory closure `move || Ok(McpServer::new(state.clone()))`. Any other path on this router answers 404 with the standard `{ "status", "error" }` body.
- [ ] `orchestrator/src/main.rs` binds `0.0.0.0:<MCP_PORT>` with `tokio::net::TcpListener` and serves `mcp_router(state.clone())` concurrently with the API listener, sharing the same graceful-shutdown signal; startup logs `mcp listener bound` with the address at `info`. A bind failure is fatal at startup, naming the port.
- [ ] `Config` exposes `mcp_port: u16` (default 7001, parsed from `MCP_PORT`); if the scaffolding epic did not add it, add it and keep `README.md` "Configuration" and `.env.example` unchanged (the variable is already documented).
- [ ] `orchestrator/src/mcp/server.rs` defines `pub struct McpServer { state: AppState }` implementing `rmcp::ServerHandler` with `get_info()` returning `ServerInfo` whose `server_info.name` is `mars-orchestrator`, `capabilities` enables tools only, and `instructions` is `Some("Mars task tracker and git tools. Call ready to find work, claim before working, comment before handing off.")`. `list_tools` and `call_tool` are left to the dispatch task; the default `rmcp` implementations (empty list / method not found) are acceptable here.
- [ ] `orchestrator/src/mcp/context.rs` defines `#[derive(Clone, Debug)] pub struct SessionContext { pub session_id: Uuid, pub project_id: Uuid, pub profile: AgentProfile }` and `McpServer` has a private `fn context(&self, ctx: &RequestContext<RoleServer>) -> Result<SessionContext, ErrorData>` reading it from the request extensions (`ctx.extensions.get::<SessionContext>()`, falling back to `ctx.extensions.get::<http::request::Parts>()` and that request's extensions, depending on how the pinned `rmcp` version forwards HTTP request parts). A missing context is an internal error logged with `tracing::error!` ("mcp request without session context"), never a panic.
- [ ] `orchestrator/tests/common/mcp.rs` (declared from `tests/common/mod.rs`) provides `pub struct McpClient` with `pub async fn connect(app: &TestApp, token: &str) -> Result<McpClient, McpConnectError>`, which binds `mcp_router(app.state.clone())` on `127.0.0.1:0` inside the test process, starts `axum::serve` on a background task, and connects an `rmcp` client (`StreamableHttpClientTransport` with the bearer token in the transport config's auth header) to `http://127.0.0.1:<port>/mcp`. `McpConnectError` exposes the HTTP status when the initialize request is refused (needed by the 401/403 tests).
- [ ] `McpClient` offers `pub async fn list_tools(&self) -> Vec<rmcp::model::Tool>`, `pub async fn call(&self, name: &str, args: serde_json::Value) -> Result<serde_json::Value, rmcp::model::ErrorData>` (returns the tool's structured JSON result, or the JSON-RPC error), `pub fn addr(&self) -> SocketAddr`, and `pub async fn raw_post(&self, headers: &[(&str, &str)], body: &str) -> reqwest::Response` for tests that need to send a malformed request.
- [ ] `orchestrator/tests/mcp_server.rs` smoke test: `McpClient::connect` with any token succeeds while no middleware is installed, `list_tools` returns an empty list, and a `GET /health` on the MCP listener returns 404 with the JSON error body.
- [ ] `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Implementation notes
- Files: `orchestrator/src/mcp/mod.rs`, `orchestrator/src/mcp/server.rs`, `orchestrator/src/mcp/context.rs`, `orchestrator/src/main.rs`, `orchestrator/src/prelude/config.rs` (if `mcp_port` is missing), `orchestrator/tests/common/mcp.rs`, `orchestrator/tests/mcp_server.rs`.
- Add the crate with `cargo add rmcp --features server,transport-streamable-http-server` and, for tests, `cargo add --dev rmcp --features client,transport-streamable-http-client` plus `reqwest` if not already a dependency (it is, for Resend). Do not edit versions by hand.
- `StreamableHttpServerConfig`: keep the defaults (stateful sessions with `Mcp-Session-Id`); the bearer middleware of the next task runs per HTTP request, so token rotation does not depend on the MCP session lifecycle.
- The factory closure creates one `McpServer` per MCP session; it must be cheap (clone `AppState` only). Nothing session-specific is stored on `McpServer`; everything comes from `SessionContext` per request.
- Log at `debug` with `session_id = %id` once the context is resolved; never log request bodies or headers.
- The MCP router is separate from the API router: it must not carry the JWT middleware, CORS, or the `/api` nesting.

## Edge cases
- `MCP_PORT` already in use: startup fails with a clear error naming the port.
- The `rmcp` version's handling of HTTP request extensions must be verified once (spike inside this task); record the finding as a doc comment on `McpServer::context` so the middleware task can rely on it.
- `axum::serve` in the test helper must be aborted when `McpClient` is dropped (keep the `JoinHandle` and abort in `Drop`).

## Testing
- `tests/mcp_server.rs` as above.
- Unit test in `server.rs` for `get_info()` fields.
- Command: `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- none: implements the documented contract as written (`ARCHITECTURE.md` "MCP design", `README.md` "Configuration").

## Assumes from other epics
- "Repository scaffolding, tooling and CI": `Config::from_env()`, `AppState`, the API listener and graceful shutdown in `main.rs`.
- "Database schema, models, repositories and test harness": `TestApp::spawn()` with `state: AppState` and `tests/common/mod.rs`.
- "Projects, agent profiles and shared directories": the `AgentProfile` model with `mcp_tools: Vec<String>`.