---
id: "4t5n5"
title: "Add the MCP bearer middleware: hash lookup against sessions.mcp_token_hash, 401 unknown, 403 for done/failed, SessionContext attached to the request"
status: in_progress
priority: P0
created: "2026-09-16T20:42:11.805742741Z"
updated: "2026-09-20T04:50:20.721815074Z"
tags:
  - orchestrator
  - mcp
  - sessions
  - auth
depends_on:
  - "8a6qd"
parent: qgj33
attempts: 1
---

## Summary
Add the tower middleware on the `/mcp` router that authenticates every HTTP request with `Authorization: Bearer <session token>`: it hashes the token with the sessions epic's `hash_mcp_token`, looks the hash up in `sessions.mcp_token_hash`, answers 401 when the header is missing, malformed or unknown, 403 when the session is `done` or `failed`, and otherwise inserts `SessionContext { session_id, project_id, profile }` into the request extensions. It also adds the `TestApp` helper that seeds a session with a known raw token so every MCP test can connect.

## Documents
- `ARCHITECTURE.md` "MCP design" → "Authentication" (hash the token, look up `sessions.mcp_token_hash`, 401 if missing, 403 if the session is `done` or `failed`, `SessionContext` attached, handlers never take a session id), "Trust boundaries" item 2 (the token identifies the session; from it the orchestrator derives the project and profile; agents never self-identify), "Restart procedure" step 2 (adoption leaves the hash unchanged).
- `docs/data-model.md` `sessions.mcp_token_hash` (NOT NULL, UNIQUE, SHA-256 of a fresh token per process launch; replacement hash commits before the new process starts), `sessions.state`, `agent_profiles`.
- `SPEC.md` "MCP tool contracts" (error code `unauthorized` = bad token; the session context supplies `session_id`, `project_id` and the profile).
- ADR 0029 (a replaced token fails authentication after a relaunch; adoption does not rotate).
- `CLAUDE.md` rule 3 (never log tokens).

## Acceptance criteria
- [ ] `orchestrator/src/mcp/auth.rs` exposes `pub async fn require_session(State(state): State<AppState>, mut request: Request, next: Next) -> Response` installed on the MCP router with `axum::middleware::from_fn_with_state`. Order of checks: no `Authorization` header, a scheme other than `Bearer`, or an empty token → 401 `{ "status": 401, "error": "missing or invalid bearer token" }`; hash not found → 401 with the same message; session `state` in (`done`, `failed`) → 403 `{ "status": 403, "error": "session has ended" }`; otherwise the request proceeds with `SessionContext` in `request.extensions_mut()`.
- [ ] `SessionRepository::find_by_mcp_token_hash(&self, hash: &str) -> Result<Option<McpSessionRow>>` where `McpSessionRow { session_id: Uuid, project_id: Uuid, profile_id: Uuid, state: SessionState }`, implemented with one `sqlx::query_as!` (`SELECT id, project_id, profile_id, state FROM sessions WHERE mcp_token_hash = $1`), and the profile is loaded with the projects epic's `ProjectRepository::get_profile(project_id, profile_id)`; a profile that cannot be loaded is an internal error (logged with `tracing::error!(session_id = %id)`, answered 500 with the generic message).
- [ ] The token string never appears in logs, spans or error bodies: a test captures `tracing` output for a failed and a successful request and asserts the raw token and its hash are absent. Successful resolution logs at `debug` with `session_id = %id` only.
- [ ] `TestApp` (or `tests/common/mcp.rs`) gains `pub async fn seed_mcp_session(&self, project_id: Uuid, profile_id: Uuid, state: SessionState) -> SeededSession { session_id: Uuid, token: String }`: generates a token with `McpToken::generate()`, inserts a session row through `SessionRepository` with `mcp_token_hash = token.hash()`, `base_ref`/`branch` filled with valid placeholders (`main`, `session/<id>`), and returns the raw token for the test only.
- [ ] `tests/mcp_auth.rs` covers: no header → 401; `Authorization: Basic xyz` → 401; `Bearer` with an unknown token → 401; token of a `done` session → 403; token of a `failed` session → 403; tokens of `creating`, `running` and `parked` sessions → initialize succeeds; the 401/403 bodies are the standard JSON error shape; a POST to `/mcp` without the header never reaches `McpServer` (assert via the response body, not just the status).
- [ ] Token replacement: seed a `parked` session, connect with its token, call the sessions epic's `rotate_token(session_id)` (the relaunch path), then assert a new request with the old token → 401 and a request with the new token → succeeds; a second rotation invalidates the second token as well. Adoption is not exercised here (the sessions epic's recovery tests cover "token unchanged").
- [ ] `cargo sqlx prepare` run and `.sqlx/` committed.
- [ ] `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Implementation notes
- Files: `orchestrator/src/mcp/auth.rs`, `orchestrator/src/mcp/mod.rs` (layer installation: `mcp_router` applies `.route_layer(middleware::from_fn_with_state(state.clone(), require_session))` to the `/mcp` service only), `orchestrator/src/repositories/sessions.rs`, `orchestrator/tests/common/mcp.rs`, `orchestrator/tests/mcp_auth.rs`.
- Reuse `mars_orchestrator::session::token::hash_mcp_token(raw: &str) -> String` (lowercase hex SHA-256) from the sessions epic; do not re-implement hashing.
- Parse the header with `axum_extra::TypedHeader<Authorization<Bearer>>` or a manual split on the first space; trim nothing else (a token with surrounding whitespace is invalid).
- The middleware answers with `Error::IntoResponse` (`Error::Unauthorized`-equivalent → 401; `Error::Forbidden` → 403) so the body shape matches the API. If the prelude `Error` has no unauthorized variant, add `Error::Unauthorized(String)` mapping to 401 and note it in `ARCHITECTURE.md` "Orchestrator internals" → "Errors" in the same commit.
- Database lookups happen per request; there is no cache, so a rotated hash takes effect immediately and a session ending mid-conversation is refused on its next call.

## Edge cases
- Two sessions with the same hash are impossible (`UNIQUE`); treat a duplicate as internal.
- A session whose project or profile was deleted between lookups: `profile_id` is `ON DELETE RESTRICT`, so this cannot happen for a live session; handle `None` defensively as internal.
- `Authorization` header present twice: take the first; the second is ignored.
- Requests to the MCP `GET` (SSE stream) and `DELETE` (session close) methods go through the same middleware; the test for `done` sessions must cover a `GET` too.

## Testing
- `tests/mcp_auth.rs` as above, with `McpClient::connect` for the success cases and `McpClient::raw_post` for the header-shape cases.
- Log capture: use `tracing_subscriber::fmt().with_writer(...)` into a shared buffer or the `tracing-test` crate (`cargo add --dev`), whichever the sessions epic already used for its "no token in logs" test.
- Command: `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- none unless `Error::Unauthorized` is new (then `ARCHITECTURE.md` "Orchestrator internals" → "Errors" lists it).

## Assumes from other epics
- "Session lifecycle: launcher, owner, recovery and sessions API": `session::token::{McpToken, hash_mcp_token}` and `rotate_token(session_id)` (task "Add launch preparation: session directory layout, MCP token generation and hashing, atomic mcp.json write and relaunch token rotation"); `SessionRepository` inserts.
- "Projects, agent profiles and shared directories": `ProjectRepository::get_profile` returning `AgentProfile` with `mcp_tools`.
- "Database schema, models, repositories and test harness": `TestApp`, `SessionState` enum.