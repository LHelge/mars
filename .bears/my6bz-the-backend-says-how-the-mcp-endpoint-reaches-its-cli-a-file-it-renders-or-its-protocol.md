---
id: my6bz
title: "The backend says how the MCP endpoint reaches its CLI: a file it renders, or its protocol"
status: open
priority: P2
created: "2026-09-21T12:17:12.565506Z"
updated: "2026-09-21T20:26:14.755647882Z"
tags:
  - orchestrator
  - agent
  - mcp
  - sessions
depends_on:
  - x8zqq
parent: fgbm3
---

## Summary
`session/prepare.rs` writes Claude's `{"mcpServers":{…}}` document to `/session/mcp.json` for every session, and `LaunchContext` carries only that path. An ACP agent receives its MCP servers in `session/new`; another CLI would want a different file. Hand the backend the endpoint (URL and the per-launch bearer token, ADR 0029) and let it answer how it is delivered.

## Documents
- `ARCHITECTURE.md`, "MCP design", "Launch sequence", "Session container specification" (the `mcp.json` bind); `SPEC.md` where `/session/mcp.json` is named as reserved.

## Acceptance criteria
- [ ] The launcher passes an `McpEndpoint { name, url, token }` to the backend; the backend returns a delivery: `File { path, contents }` (Claude: today's document, byte-identical, same ro bind) or `InProtocol` (the backend keeps the endpoint in `TranslateState` and uses it in its preamble).
- [ ] The token never reaches argv, a log line at any level, a diagnostic or an event (`CLAUDE.md` rule 3); the existing log-leak test in `prepare.rs` covers both deliveries. `McpEndpoint` has a redacting `Debug`.
- [ ] With `InProtocol`, no `mcp.json` is written or bound.
- [ ] The MCP `launch_warning` derived from `init` keeps working for Claude; the rule for a backend that reports MCP status differently is stated in the doc and left to its adapter.

## Implementation notes
- `session/prepare.rs` (~l.243–260 and tests ~l.559–641), `session/launcher.rs`, `agent/mod.rs` (`LaunchContext::mcp_config_path`, `DEFAULT_MCP_CONFIG_PATH`), `agent/claude/launch.rs`, `engine/spec.rs` (the bind).
- After an orchestrator restart the token of a running process is the one of its launch (ADR 0029); `InProtocol` state must be rebuildable or not needed after the handshake — it is only needed in the preamble, which is never replayed.

## Testing
- Unit tests in `prepare.rs` and the Claude launch tests unchanged in expectation; a mock-backend launcher test for `InProtocol`.