---
id: qgj33
title: MCP server and agent tools
type: epic
status: open
priority: P1
created: "2026-09-16T20:14:07.871123795Z"
updated: "2026-09-16T20:15:44.443140356Z"
tags:
  - orchestrator
  - mcp
depends_on:
  - xjaah
---

## Scope

`mcp/`: the `rmcp` Streamable HTTP server on `MCP_PORT` at `/mcp`.

- Bearer middleware: hash lookup against `sessions.mcp_token_hash`, 401 unknown, 403 for `done`/`failed`, `SessionContext { session_id, project_id, profile }` attached; handlers never accept a session id argument.
- Tool listing: tracker tools always, git tools only when in `profile.mcp_tools`; calling an unlisted tool is an error.
- Tools with the verbatim descriptions from `SPEC.md`: `ready`, `claim`, `get_task`, `update` (including hand-off inputs with the source session derived from the caller), `release`, `comment`, `needs_human`, `create_task` (provenance rules), `list_session_branches`, `merge`, `rebase` (work-tree update for the caller's own branch), `push` (force requires `force: true` and the `push` permission). Task arguments accept UUID, number, or `"#12"`.
- `src/mcp/error.rs` mapping `Error` to `unauthorized`, `forbidden`, `not_found`, `conflict` (with `data.conflicts` for git), `invalid_argument`, `internal`.
- Side-effect rules: successful changes commit events and session links together; reads and rejections write nothing (ADR 0030).

## Documents

`SPEC.md` "MCP tool contracts"; `ARCHITECTURE.md` "MCP design"; ADRs 0007, 0029, 0030.

## Acceptance criteria

- [ ] MCP tests drive every tool through the in-process `rmcp` server with a session bearer token from `TestApp`, covering happy paths, `conflict` on lost claims and stale hand-offs, `forbidden` for gated git tools, `invalid_argument` for unknown states, ambiguous provenance and cycles, and 401/403 middleware cases.
- [ ] Tool descriptions in code match `SPEC.md` byte for byte (asserted by a test).
- [ ] A replaced token fails authentication after a relaunch.

## Out of scope

Tracker and git semantics (their own epics); the `mcp.json` file and token generation (Session lifecycle epic).