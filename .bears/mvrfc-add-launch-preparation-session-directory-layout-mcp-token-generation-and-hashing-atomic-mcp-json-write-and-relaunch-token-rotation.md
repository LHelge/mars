---
id: mvrfc
title: "Add launch preparation: session directory layout, MCP token generation and hashing, atomic mcp.json write and relaunch token rotation"
status: open
priority: P1
created: "2026-09-16T20:30:05.913685711Z"
updated: "2026-09-16T20:30:05.913685711Z"
tags:
  - orchestrator
  - sessions
  - mcp
depends_on:
  - "88tdh"
parent: s52qg
---

## Summary
Implement `orchestrator/src/session/prepare.rs` and `session/token.rs`: the session directory layout under `DATA_DIR/sessions/<sid>/`, cryptographically random MCP bearer tokens with their SHA-256 hash, the exact `mcp.json` document written through a temporary file and atomic rename, and the relaunch rotation that commits the replacement hash before the file is rewritten. Nothing starts a container until both the hash and the file are ready (ADR 0029).

## Documents
- `ARCHITECTURE.md` "Storage" (`/data/sessions/<session_id>/{work,home,log/stream.jsonl,log/stderr.log,mcp.json}`; `DATA_DIR` vs `DATA_DIR_HOST`), "MCP design" → "Per-session config file" (exact JSON, server name `mars-orchestrator`, `MCP_URL`, fresh token on every actual launch/resume/retry, temp file plus atomic replacement, raw token never in API or logs, DB then file not one transaction but no process starts until both are ready, adoption leaves both unchanged), "Trust boundaries" (per-session bearer identifies the session).
- `docs/data-model.md` `sessions.mcp_token_hash` (NOT NULL, UNIQUE, SHA-256; initial hash inserted with the row; replacement hash commits before the new process starts).
- `README.md` "Configuration" (`MCP_URL` default `http://orchestrator:7001/mcp`, `DATA_DIR`, `DATA_DIR_HOST`).
- ADR 0029.

## Acceptance criteria
- [ ] `SessionDirs::for_session(data_dir: &Path, data_dir_host: &Path, sid: Uuid)` exposes both the orchestrator-side and host-side paths for `root`, `work`, `home`, `log`, `stream_jsonl`, `stderr_log`, `mcp_json`.
- [ ] `SessionDirs::ensure(&self) -> Result<()>` creates `work/`, `home/`, `log/` (idempotent) and touches `log/stream.jsonl` so the owner can open it before the CLI writes; `work/` is left empty for the git clone.
- [ ] `McpToken::generate() -> McpToken` produces 32 bytes from `rand::rngs::OsRng` encoded as URL-safe base64 without padding (43 characters); `McpToken::hash(&self) -> String` is lowercase hex SHA-256 of the token string; `pub fn hash_mcp_token(raw: &str) -> String` is the one function the MCP epic's bearer middleware must reuse. `McpToken` implements `Zeroize` on drop and a `Debug` that prints `McpToken(<redacted>)`.
- [ ] `write_mcp_json(dirs: &SessionDirs, mcp_url: &str, token: &McpToken) -> Result<()>` writes exactly `{"mcpServers":{"mars-orchestrator":{"type":"http","url":"<MCP_URL>","headers":{"Authorization":"Bearer <token>"}}}}` (serde_json, key order as documented) to `<root>/mcp.json.tmp`, `sync_all`, then `rename` over `<root>/mcp.json`; file mode `0600` is not usable because the container reads it as uid 1000 mapped to the orchestrator's uid under the uid contract — use `0644` and note that `/data` is orchestrator-owned.
- [ ] `rotate_token(&self, session_id) -> Result<McpToken>` (on `LaunchPreparer` or a free function taking the pool and dirs): generates a token, `UPDATE sessions SET mcp_token_hash = $2 WHERE id = $1 AND state IN ('parked','creating','failed')` in its own committed transaction (zero rows → `Error::Conflict("session is not relaunchable")`), then writes the file; any failure returns `Err` and the caller must not start a process.
- [ ] `initial_token()` for first creation: the route generates the token before inserting the row (its hash goes into `NewSessionRow.mcp_token_hash`) and passes the raw token to the launcher, which writes the file after creating the directories.
- [ ] No code path logs the raw token or the config file contents; tracing spans carry `session_id` only. A test greps captured logs for the token string.

## Implementation notes
- Files: `orchestrator/src/session/prepare.rs`, `orchestrator/src/session/token.rs`, `orchestrator/src/session/mod.rs`; crates already listed in `ARCHITECTURE.md`: `rand`, `sha2`, `base64`, `zeroize`.
- `Config` must expose `data_dir`, `data_dir_host` and `mcp_url` (delivered by the scaffolding epic's `Config::from_env()`; add the fields if missing and update `README.md` "Configuration" only if a variable is new — none should be).
- Adoption after restart never calls `rotate_token` or `write_mcp_json` (the recovery task relies on this).

## Edge cases
- `mcp.json` already exists from a previous launch: the rename overwrites it atomically; a stale `mcp.json.tmp` from a crashed preparation is overwritten by the next write.
- The `UNIQUE` constraint on `mcp_token_hash` colliding is practically impossible; treat it as `Error::Internal`.
- `DATA_DIR` not writable: `ensure()` returns `Error::Internal` with the path in the log (not in the API message).

## Testing
- Unit tests: token length and hex hash length (64), `hash_mcp_token` deterministic and equal to `McpToken::hash`, two tokens differ, `Debug` output redacted, `mcp.json` content byte-exact for a sample URL and token, no `.tmp` left after a successful write.
- Integration test with `TestApp` and a `tempfile` data dir: `rotate_token` on a `parked` session replaces the hash and rewrites the file; on a `running` session it returns 409 semantics and leaves both hash and file untouched; a write failure (make `root` read-only) leaves the previous file intact.
- `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Repository scaffolding, tooling and CI" delivers `Config::from_env()` with `DATA_DIR`, `DATA_DIR_HOST`, `MCP_URL`.
- "MCP server and agent tools" will call `hash_mcp_token` in its bearer middleware; this task defines it.