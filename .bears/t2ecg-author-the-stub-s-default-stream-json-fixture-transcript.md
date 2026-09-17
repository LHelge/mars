---
id: t2ecg
title: Author the stub's default stream-json fixture transcript
status: done
priority: P1
created: "2026-09-16T20:28:06.022145935Z"
updated: "2026-09-17T22:13:28.518035888Z"
tags:
  - images
  - tests
depends_on:
  - sywed
parent: deex5
assignee: claude-opus-subagent
attempts: 1
---

## Summary
Write `images/stub/fixtures/default.jsonl`, the transcript the stub replays by default. It must look like the pinned Claude Code CLI's `--output-format stream-json --verbose` output and exercise every translation rule in `SPEC.md` "AgentEvent" across three turns, so that the session view, the owner tests and Playwright see a realistic transcript (text, thinking, tool calls with results, a nested subagent, partial-message deltas, a permission denial, cost and usage).

## Documents
- `SPEC.md` "AgentEvent" (translation rules: `system`/`init`, `system`/`permission_denied`, `assistant` content blocks, CLI `user` tool results, `stream_event`, `result` with `total_cost_usd`, `parent_tool_use_id`, the subagent tool named `Task` or `Agent`)
- `ARCHITECTURE.md` "Session image" (stub paragraph), "Cost accounting" (`total_cost_usd`, `usage` on `result`), "Claude Code invocation" (`--include-partial-messages`, `--forward-subagent-text`)
- `CLAUDE.md` "Testing expectations" (event translation fixtures are recorded per pinned CLI version; this fixture is the stub's, not the adapter's)
- Claude Code CLI reference and Agent SDK message types (`SDKMessage`) for the exact native field names

## Acceptance criteria
- [ ] `images/stub/fixtures/default.jsonl` is valid JSONL (every line parses; `python3 -c 'import json,sys; [json.loads(l) for l in sys.stdin]' < file` succeeds) and contains no real credentials, hostnames or personal data; the repository path in the content is `/session/work` and file names are fictional.
- [ ] Line 1 is a `system`/`init` line (kept for fidelity even though the stub replaces it) with `session_id`, `tools`, `mcp_servers: [{"name":"mars-orchestrator","status":"connected"}]`, `model`, `cwd: "/session/work"`, `permissionMode: "bypassPermissions"`.
- [ ] Every non-init line carries `session_id` and a `uuid` field; `assistant` and `user` lines carry `message` with `role`, `content` (array of blocks) and, for assistant, `id`, `model`, `stop_reason`, `usage`.
- [ ] Exactly three turns, each ending with a `result` line: `{"type":"result","subtype":"success","is_error":false,"duration_ms":...,"duration_api_ms":...,"num_turns":...,"result":"<final text>","session_id":...,"total_cost_usd":<number>,"usage":{"input_tokens":...,"output_tokens":...,"cache_creation_input_tokens":...,"cache_read_input_tokens":...},"permission_denials":[...]}`. Costs are distinct per turn (e.g. 0.0123, 0.0456, 0.0089) so accumulation is testable.
- [ ] Turn 1: assistant `text` block; assistant `tool_use` `Read` (`{"file_path":"/session/work/README.md"}`); `user` line with a `tool_result` block (`tool_use_id`, `content` string, `is_error: false`); assistant `text`; `result`.
- [ ] Turn 2: assistant `thinking` block (with `signature`); assistant `tool_use` named `Task` (`{"description":"Find call sites","prompt":"...","subagent_type":"general-purpose"}`); at least three lines carrying `parent_tool_use_id` equal to that tool_use id (an assistant `text`, an assistant `tool_use` `Grep`, a `user` `tool_result`); a `user` `tool_result` for the `Task` call; assistant `tool_use` `Edit` with `file_path`, `old_string`, `new_string`; its `tool_result`; assistant `tool_use` `Bash` with `command: "git add -A && git commit -m \"Add greeting\""`; a `tool_result` whose content includes a fake 40-hex commit id; assistant `text`; `result`.
- [ ] Turn 3: two `stream_event` lines (`event.type: "content_block_delta"`, `delta.type: "text_delta"`) preceding the complete assistant `text` block that they spell; one `system` line with `subtype: "permission_denied"` naming a tool (`tool_name`/`tool_use_id`/`reason` per the pinned CLI shape); assistant `text`; `result` whose `permission_denials` lists that denial and whose `subtype` is still `success`.
- [ ] A `tool_result` content string in turn 2 exceeds 8 KiB (repeat a line) so the UI's collapse-over-40-lines path is exercised, but the whole file stays under 64 KiB.
- [ ] A header comment is not possible in JSONL; instead `images/stub/fixtures/README.md` (ten lines at most) states the source (hand-authored from the CLI reference for version X, or recorded), the turn structure, and how to re-record.

## Implementation notes
- Files: `images/stub/fixtures/default.jsonl`, `images/stub/fixtures/README.md`.
- Field names must follow the pinned CLI version's stream-json (the version chosen by the claude Dockerfile task). Where the reference is silent (the exact `permission_denied` system message), follow the Agent SDK's TypeScript message types and mark the line in the fixture README as "to be confirmed by the live probe"; the adapter epic's probe (Claude Code agent backend epic) is the authority and may adjust it.
- Generate ids deterministically readable: `toolu_01...`, `msg_01...`, UUIDs with an obvious fake pattern (`00000000-0000-4000-8000-0000000000NN`) so assertions in other epics can reference them by value.
- The stub rewrites `session_id` at replay time, so the fixture's value only needs to be well-formed.

## Edge cases
- The subagent tool is `Task` in the fixture; add a second, single-line fixture `images/stub/fixtures/agent-tool.jsonl` with one turn using the name `Agent` so the translator's constant matching both names can be proven against a container run.
- Keep every line under 32 KiB: the owner reads complete lines and the translator truncates tool results at 256 KiB; the fixture must not test those limits.

## Testing
- Validated by the stub script's unit tests (they load this fixture through `MARS_STUB_FIXTURE` for at least one case and assert three `result` lines) and by the smoke-test task (`-p` replay count).
- A JSONL validity check and the credential-pattern grep (`sk-ant-`, `ghp_`, `@`) run in the images CI lint job.

## Documentation
- `images/stub/fixtures/README.md` created in this task. No main document changes.

## Assumes from other epics
- none (the adapter epic's fixtures under `orchestrator/tests/fixtures/claude/<version>/` are a separate artefact; if the live probe later records a real transcript, the verification task of this epic swaps it in).