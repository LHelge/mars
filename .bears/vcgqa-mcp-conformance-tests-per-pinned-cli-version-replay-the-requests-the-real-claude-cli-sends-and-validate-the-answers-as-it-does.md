---
id: vcgqa
title: "MCP conformance tests per pinned CLI version: replay the requests the real Claude CLI sends and validate the answers as it does"
status: open
priority: P1
created: "2026-09-20T23:39:04.846177870Z"
updated: "2026-09-20T23:39:04.846177870Z"
tags:
  - orchestrator
  - mcp
  - testing
---

## Summary
Two P0 bugs reached the live stack on 2026-09-21 (`pacy2`: every session refused with 403 by the Host check; `4gd2v`: `tools/list` discarded by the CLI for lacking `ttlMs`/`cacheScope`). Both were invisible to the test suite for the same reason: the MCP tests talk to the server through rmcp's own client over `127.0.0.1`, which sends a loopback `Host`, speaks whatever rmcp speaks and validates nothing strictly; and the stub session image never calls MCP. Nothing tests the server against what the **pinned Claude Code CLI** actually sends and accepts.

Add a fixture-based conformance suite, in the spirit of the event-translation fixtures: per pinned CLI version, the recorded HTTP requests of a real CLI's MCP conversation, replayed against the in-process listener, with the answers checked against the constraints that CLI version enforces.

## Documents
- `CLAUDE.md` "Testing expectations" (new bullet: MCP conformance fixtures; a CLI version bump adds fixtures, never edits old ones)
- `ARCHITECTURE.md` "MCP design" (one sentence: which protocol revision the pinned CLI negotiates and that the conformance suite pins it)
- `images/claude/VERIFY.md` (how the fixtures were recorded)

## Acceptance criteria
- [ ] `orchestrator/tests/fixtures/mcp/<cli version>/` holds the recorded exchange of the pinned CLI (2.1.274) with the Mars MCP server: for each request its method, path, the headers that matter (`Host`, `MCP-Protocol-Version`, `Mcp-Method`, `Mcp-Name`, `Accept`, `Content-Type`; never `Authorization`) and body. At least: the discovery/initialize step the CLI performs, `tools/list`, one successful `tools/call` (`ready`), one failing `tools/call` (unknown tool), and whatever the CLI sends on shutdown.
- [ ] `orchestrator/tests/mcp_conformance.rs` replays each fixture through real HTTP against `mcp_router` with a seeded session token, **sending the recorded `Host`** (the `MCP_URL` authority, not loopback), and asserts per response: HTTP status, JSON-RPC shape, and the fields the CLI validates — for protocol `2026-07-28`: `resultType` on every result, numeric `ttlMs` and `cacheScope ∈ {public, private}` on list results, `content`/`isError` on call results, error objects with integer `code` and string `message`.
- [ ] Every tool's `inputSchema` and `outputSchema` in the `tools/list` answer is asserted to be a JSON object with top-level `"type": "object"` (the CLI's SDK rejects a listing containing a tool whose schema is not), and tool names match `^[a-zA-Z0-9_-]{1,64}$`.
- [ ] The suite fails on today's two bugs when their fixes are reverted (check by hand once and say so in the commit message).
- [ ] Recording is reproducible: a documented procedure (script or `VERIFY.md` section) that points a real CLI at a logging proxy in front of a development orchestrator and writes the fixture files, with the bearer token stripped. No real credential ends up in a fixture (rule 3); the recording run needs a model credential only if the CLI refuses to start MCP without one — establish which and write it down.
- [ ] Both clippy invocations and the test suite pass; the new suite needs no container engine and no network.

## Implementation notes
- The CLI's view of a failure is in its own log inside the session home: `home/.cache/claude-cli-nodejs/-session-work/mcp-logs-mars-orchestrator/*.jsonl`. That is where `4gd2v` was diagnosed and is the fastest way to see what the CLI rejected.
- Hand-written requests that reproduce the CLI's `tools/list` and `tools/call` are in the body of `4gd2v` and in `tests/mcp_server.rs` (`MODERN_TOOLS_LIST`); the recorded fixtures replace guesses like those with what the CLI really sends (it may send more: `server/discover`, a `GET` for the listen stream, a `DELETE`).
- `McpClient::raw_post` in `tests/common/mcp.rs` sends arbitrary headers; add `raw_get`/`raw_delete` if the recording shows the CLI uses them.
- The validation rules are a hand-maintained mirror of the CLI's schema, so keep them in one small module with the protocol revision in its name, and extend it only from observed rejections or the published schema for that revision.
- `tests/claude_probe.rs` already launches the real CLI for the stdin protocol; if it can be pointed at a live listener cheaply, a probe-style test that asserts "the real CLI ends up with 8 tools" is the strongest check of all — consider it, but it needs the image and so cannot replace the fixture suite.

## Edge cases
- A CLI version bump may change the negotiated protocol revision: that adds a fixture directory and possibly a validator module; old fixtures stay and keep passing as long as that version is supported.
- rmcp upgrades can change defaults silently (both bugs were library defaults): this suite is what makes an rmcp bump safe, so run it first when bumping.

## Testing
- The suite is the test. Plus the one-off revert check named above.
