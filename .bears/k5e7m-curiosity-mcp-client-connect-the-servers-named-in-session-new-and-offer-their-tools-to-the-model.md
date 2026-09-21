---
id: k5e7m
title: "Curiosity MCP client: connect the servers named in session/new and offer their tools to the model"
status: open
priority: P2
created: "2026-09-21T12:19:33.991832Z"
updated: "2026-09-21T20:26:14.950227552Z"
tags:
  - curiosity
  - mcp
depends_on:
  - pfs5r
parent: vj82v
---

## Summary
In Mars the task tracker is reached over MCP (`SPEC.md`, "MCP tool contracts"): streamable HTTP with a per-launch bearer token (ADR 0029). midgaard's Bears-bound task tool was left behind; this replaces it with a general MCP client, which also serves any server a repository or a user adds later.

## Documents
- `curiosity/README.md`: supported transports, tool naming, failure behaviour.

## Acceptance criteria
- [ ] `session/new` `mcpServers` entries are connected with `rmcp`'s client (`cargo add rmcp` with client + streamable-HTTP + child-process transports): HTTP with the entry's `headers` sent on every request, and stdio (`command`, `args`, `env`). `initialize` now advertises `mcpCapabilities.http: true`. SSE is advertised only if implemented.
- [ ] Each server's tools are listed once at connect and exposed to the model as `mcp__<server>__<tool>` with their JSON schema and description passed through; a call is forwarded, and text/structured results come back as the tool result. An MCP `isError` result is a tool error the model sees.
- [ ] A server that cannot be reached does not fail the session: the turn runs without its tools and the failure is reported once (an event, and an ACP-visible signal the Mars adapter can turn into its MCP `launch_warning` — agree the shape with the spike's notes, `_meta` if nothing standard fits).
- [ ] Header values (the bearer token) never appear in an event, a log line at any level, or an error message (`CLAUDE.md`, rule 3).
- [ ] Tool-name collisions with built-in tools are impossible by the prefix; two servers with one name: the second is refused with a clear error.

## Implementation notes
- The Mars MCP server sends `instructions` saying it is the tracker (epic `pekcb`); pass server instructions into the system prompt, as Claude Code does.
- Tools of an MCP server implement the same `Tool` trait as built-ins, so the loop does not know the difference.

## Edge cases
- A call that outlives `session/cancel`: cancelled with the turn. A server that disappears mid-session: the call errors, the session continues.

## Testing
- Integration test against an in-process `rmcp` server over HTTP on a loopback port with a required `Authorization` header, plus a stdio server fixture; mock model script calls a tool on each.