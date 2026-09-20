---
id: "4gd2v"
title: "MCP tools/list is rejected by the Claude CLI under protocol 2026-07-28: result lacks ttlMs and cacheScope"
status: done
priority: P0
created: "2026-09-20T23:25:48.154105400Z"
updated: "2026-09-20T23:48:42.236706289Z"
tags:
  - orchestrator
  - mcp
  - bug
depends_on:
  - pacy2
---

## Summary
Found on the live stack on 2026-09-21, directly behind `pacy2`: once the Host check let sessions in, the CLI reported `mars-orchestrator` as connected with **no tools**. The CLI's own MCP log (`~/.cache/claude-cli-nodejs/-session-work/mcp-logs-mars-orchestrator/` in the session home) says `tools/list failed (Invalid result for tools/list: ttlMs expected number; cacheScope expected "public" | "private")`, retried three times, then `Failed to fetch tools`.

Cause: Claude Code 2.1.274 negotiates MCP protocol revision `2026-07-28` (stateless, `server/discover` lifecycle), in which `ttlMs` and `cacheScope` are required on list results (SEP-2549). rmcp 3.4.0 supports the revision but leaves both fields `None` unless the handler sets them, and `list_tools` did not. The integration tests use rmcp's own client, which is lenient, so nothing caught it.

## Documents
- `ARCHITECTURE.md` "MCP design", Tool exposure

## Acceptance criteria
- [ ] `list_tools` sets `ttl_ms` (60 s) and `cache_scope: private` (the listing is filtered by the calling session's profile).
- [ ] A regression test sends `tools/list` the way the CLI does (protocol `2026-07-28`, `_meta`, `Mcp-Method` header, no `initialize`) and asserts `resultType`, a numeric `ttlMs` and `cacheScope: "private"` on the raw JSON.
- [ ] Verified by hand that `tools/call` in the same form already answers with `resultType: "complete"`.
- [ ] Both clippy invocations and the test suite pass.

## Follow-up worth its own task
The protocol surface the pinned CLI actually uses is not covered by any automated test: the MCP tests speak through rmcp's client, and the stub image never calls MCP. A small conformance test per pinned CLI version — raw requests recorded from the real CLI, like the event translation fixtures — would have caught both this and `pacy2`.
