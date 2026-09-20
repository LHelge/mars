---
id: vxcrn
title: "MCP server instructions: say this server is the session's task tracker and replaces the one the repository names"
status: open
priority: P1
created: "2026-09-20T22:40:12.532043890Z"
updated: "2026-09-20T22:40:12.532043890Z"
tags:
  - orchestrator
  - mcp
  - docs
parent: pekcb
---

## Summary
The smallest change that fixes the observed failure for every profile, including hand-made ones with an empty system prompt. The `instructions` string the MCP server returns at `initialize` gains the statement that this server is the task tracker of the session and supersedes any tracker the repository's own instructions name. The text becomes part of the documented contract.

## Documents
- `SPEC.md` "MCP tool contracts": add the server `instructions` verbatim next to "Tool descriptions are part of the contract" (it is not reproduced there today)
- `ARCHITECTURE.md` "MCP design": one sentence on why the instructions carry this (the repository's `CLAUDE.md` is read in non-bare mode and may name another tracker)

## Acceptance criteria
- [ ] `INSTRUCTIONS` in `orchestrator/src/mcp/server.rs` reads (adjust wording only with reason, and keep it tracker-agnostic — it must not name any specific product):
  > Mars task tracker and git tools. This server is the task tracker for this session: it replaces any task tracker, issue tracker or planning tool that the repository's own instructions name, so follow their workflow with these tools instead, never report the other tracker as missing, and never write task files into the repository. Call ready to find work, claim before working, comment before handing off.
- [ ] `SPEC.md` reproduces the string verbatim and the unit test `the_instructions_are_the_documented_brief` asserts the same text.
- [ ] A test (or an extension of `tests/mcp_descriptions.rs`) asserts that neither the instructions nor any tool description contains the name of an external tracker product; keep the deny-list in the test small and obvious.
- [ ] Both clippy invocations and `cargo test --features integration-tests` pass.

## Implementation notes
- Files: `orchestrator/src/mcp/server.rs`, `orchestrator/tests/mcp_descriptions.rs` or `mcp_server.rs`, `SPEC.md`, `ARCHITECTURE.md`.
- Instructions are delivered once per MCP connection; the CLI puts them in the model's context. Keep them short: they are paid for on every session.

## Edge cases
- A repository that ships a `.mcp.json` naming its own tracker server: the CLI will still try to connect it and fail, because the image does not contain it. Out of scope here; if it turns out to confuse agents despite the new instructions, file a follow-up (options: `--strict-mcp-config`, which would also drop the repository's other servers, so it needs its own decision).

## Testing
- Unit test on the constant; the descriptions test above.
