# Claude Code 2.1.282 — live probe observations

Recorded by `tests/claude_probe.rs`. Observations only; the native lines are in the `.jsonl` files beside this one.

## auth_failure

- Launched with the fake credential in `CLAUDE_CODE_OAUTH_TOKEN`.
- Line types seen: ["system", "system", "system", "assistant", "result"].
- Exit status: exit status: 1.

## ephemeral

- `init`, `assistant` and `result` all arrived with no stdin at all.
- `result.subtype`: Some(String("success")), `is_error`: Some(Bool(false)).
- Exit status: exit status: 0.

## mcp_config

- `init` field names (item 6): ["agents", "analytics_disabled", "apiKeySource", "capabilities", "claude_code_version", "cwd", "fast_mode_disabled_reason", "fast_mode_state", "mcp_servers", "memory_paths", "messaging_socket_path", "model", "output_style", "per_turn_effort_active", "permissionMode", "plugins", "product_feedback_disabled", "session_id", "skills", "slash_commands", "subtype", "terminal_slash_commands", "tools", "type", "uuid", "view_mode"].
- Without `--strict-mcp-config`, `init.mcp_servers` = [("repo-local", "failed"), ("mars-orchestrator", "failed")].
- With `--strict-mcp-config`, `init.mcp_servers` = [("mars-orchestrator", "failed")].
- A repository `.mcp.json` defining `repo-local` was present in the working directory for both runs.
- The unreachable MCP URL did not stop the turn: both runs produced a `result`.
- Exit statuses on stdin EOF: exit status: 0 (lenient), exit status: 0 (strict).

## mcp_config_strict

See `mcp_config`.

## mid_turn

- A second message written 1 s into a turn was queued.
- The first turn finished its count to 30 before the second turn began.
- First `result`: subtype Some(String("success")), `num_turns` Some(Number(1)); second `result`: subtype Some(String("success")).
- `control_request`/`interrupt` produced a `control_response` line: true.
- The interrupted turn ended with: None.
- Exit status on stdin EOF: exit status: 1.

## multi_turn

- Two sequential messages produced two `result` lines on one process.
- `num_turns`: Some(Number(1)) then Some(Number(1)).
- `total_cost_usd`: Some(0.0402538) then Some(0.0461412) — cumulative for the process.
- The second turn answered the second message: true.
- Exit status on stdin EOF: exit status: 0.

## prompt_kind

- Line types seen: ["system", "system", "system", "system", "assistant", "assistant", "user", "rate_limit_event", "assistant", "result"].
- Lines mentioning `AskUserQuestion`: 4.
- The turn ended without an answer being written to stdin: subtype Some(String("success")), `is_error` Some(Bool(false)).
- Exit status on stdin EOF: exit status: 0.
- Nothing blocked on stdin: the `result` arrived while stdin was idle.

## resume_prompt

- The fresh process answered with ALPHA: true.
- The resumed process, launched with a changed `--append-system-prompt`, answered with BRAVO: true (ADR 0003: fresh and resume behave identically).
- `init` field names on the resumed launch: ["agents", "analytics_disabled", "apiKeySource", "capabilities", "claude_code_version", "cwd", "fast_mode_disabled_reason", "fast_mode_state", "mcp_servers", "memory_paths", "messaging_socket_path", "model", "output_style", "per_turn_effort_active", "permissionMode", "plugins", "product_feedback_disabled", "session_id", "skills", "slash_commands", "subtype", "terminal_slash_commands", "tools", "type", "uuid", "view_mode"]; a `resumed`-like field is present: false.
- The resumed `init.session_id` equals the one resumed: true.
- Exit statuses on stdin EOF: exit status: 0 (fresh), exit status: 0 (resumed).

## resume_prompt_first

See `resume_prompt`; this is its fresh launch.

## stdin_shape

- The CLI wrote nothing, `init` included, until the first stdin line: a probe that waited for `init` before writing saw no output for 120 s.
- The bare `{"type":"user","message":{...}}` line was accepted.
- `init`, `assistant` and `result` all arrived within the scenario budget.
- `result.subtype`: Some(String("success")), `is_error`: Some(Bool(false)).
- Exit status on stdin EOF: exit status: 0.

## subagent

- Lines carrying `parent_tool_use_id`: 8.
- Subagent tool names in `tool_use` frames: ["Agent"].
- `init.tools` lists: Some(Array [String("Task"), String("Bash"), String("CronCreate"), String("CronDelete"), String("CronList"), String("DesignSync"), String("Edit"), String("EnterWorktree"), String("ExitWorktree"), String("ListAgents"), String("Monitor"), String("NotebookEdit"), String("PushNotification"), String("Read"), String("RemoteTrigger"), String("ReportFindings"), String("ScheduleWakeup"), String("SendMessage"), String("Skill"), String("TaskStop"), String("ToolSearch"), String("WebFetch"), String("WebSearch"), String("Workflow"), String("Write")]).
- Line types seen: ["system", "assistant", "assistant", "system", "user", "rate_limit_event", "assistant", "system", "assistant", "user", "assistant", "system", "assistant", "user", "assistant", "system", "system", "user", "assistant", "result"].
- Exit status on stdin EOF: exit status: 0.

