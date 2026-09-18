---
id: gukce
title: Align the Claude translator, SPEC and stop semantics with the native shapes recorded from Claude Code 2.1.274
status: open
priority: P1
created: "2026-09-18T08:02:01.345008599Z"
updated: "2026-09-18T08:02:01.345008599Z"
tags:
  - orchestrator
  - agent
  - docs
depends_on:
  - h3e43
  - "4c387"
  - eeswv
parent: "8vnwy"
---

## Summary
The credentialed verification of the claude image (task h3e43, `images/claude/VERIFY.md`, recording in `images/stub/fixtures/default.jsonl`) observed native shapes on Claude Code 2.1.274 that the translator task bodies, `SPEC.md` "AgentEvent" and `ARCHITECTURE.md` "Stop semantics" and "Cost accounting" were written without. Bring the translator and the documents in line; the live probe (u3eta) stays the authority and the docs write-back (r6yek) closes the open questions.

## Documents
- `images/claude/VERIFY.md` "Observed on 2.1.274" (source of every item below)
- `SPEC.md` "AgentEvent"; `ARCHITECTURE.md` "Stop semantics", "Cost accounting", "Claude Code invocation", "Input encoding"
- `docs/open-questions.md` items 1, 5, 6 (answered by observation; deleted by r6yek, not here)

## Acceptance criteria
- [ ] `system`/`init` arrives at the start of every turn, not once per process: the translator emits one `init` event per process (or the spec says otherwise) and later ones only refresh state; unit test with three init lines.
- [ ] SIGINT ends the turn with a `user` line `[Request interrupted by user]` and a `result` with `subtype: "error_during_execution"`, `is_error: true`, `terminal_reason: "aborted_streaming"`, exit 0. `ARCHITECTURE.md` "Stop semantics" says so, the echo line is suppressed, and the owner does not treat this result as a failure of the session.
- [ ] `result.total_cost_usd` and `modelUsage` are cumulative for the process and restart at zero on `--resume`; `num_turns` is per turn. "Cost accounting" states the increase-over-previous rule.
- [ ] The subagent tool is `Agent` on 2.1.274 (`Task` has no recorded example); subagent frames start with a `user` text line carrying `parent_tool_use_id` (the subagent prompt), which must not become a `user_message` event.
- [ ] `system`/`permission_denied` has `tool_name`, `tool_use_id`, `decision_reason_type`, `message` and no `decision_reason`; `result.permission_denials` entries are `{tool_name, tool_use_id, tool_input}`.
- [ ] Authentication failure is `system`/`api_retry` lines with `error_status: 401`, `error: "authentication_failed"`, then a synthetic assistant message and a `result`, exit 1; the fatal `error` rule of task 4c387 matches this shape.
- [ ] Line kinds without a rule today get an explicit decision (ignore, or `raw`): top-level `rate_limit_event`; `system` subtypes `status`, `thinking_tokens`, `api_retry`, `task_started`, `task_progress`, `task_updated`, `task_notification`, `vcs_state_changed`; `stream_event` deltas other than `text_delta` (`thinking_delta`, `signature_delta`, `input_json_delta`, start and stop events). `rate_limit_event` carries account usage and must not be logged at `info`.
- [ ] `init.mcp_servers` entries carry `source` and the status `failed` for an unreachable server; `launch_warning` handling covers `failed`.
- [ ] Thinking blocks arrive with empty `thinking` text and a `signature`; the `thinking` event rule says what is emitted then.

## Testing
- Unit tests per rule using lines copied from `images/stub/fixtures/default.jsonl`; the full backend chain.