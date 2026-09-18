# Verifying the claude session image with real credentials

Manual and credentialed; never runs in CI. Last run: Claude Code 2.1.274, model `claude-sonnet-5`, rootless Podman 6.1.2, 2026-09-18, `CLAUDE_CODE_OAUTH_TOKEN` from `claude setup-token`. About 0.35 USD of reported cost for all runs. The credential lives only in the operator's shell (rule 3 of `CLAUDE.md`).

## Procedure

```bash
V=$(sed -n 's/^ARG CLAUDE_CODE_VERSION=//p' images/claude/Dockerfile)
podman build -t mars-session-claude:$V -t mars-session-claude:latest images/claude
S=$(mktemp -d); mkdir -p $S/{work/.claude,work/src,work/docs,home,log,log2,log3,config}
# work: a throwaway repository with README.md, src/app.py (a `main` function), a docs/CHANGELOG.md
# over 8 KiB, fake user.name/user.email, and the deny rule that produces the permission denial:
echo '{"permissions":{"deny":["Bash(curl:*)","WebFetch"]}}' > $S/work/.claude/settings.json
echo '{"mcpServers":{"mars-orchestrator":{"type":"http","url":"http://127.0.0.1:1/mcp"}}}' > $S/mcp.json
export CLAUDE_CODE_OAUTH_TOKEN   # or ANTHROPIC_API_KEY, never both; set in this shell only
run() { log=$1; shift; podman run --rm -i --name mars-verify --user 1000:1000 \
  --userns=keep-id:uid=1000,gid=1000 --cap-drop ALL --security-opt no-new-privileges \
  -v $S/work:/session/work -v $S/home:/session/home -v $S/$log:/session/log \
  -v $S/config:/session/config -v $S/mcp.json:/session/mcp.json:ro \
  -e HOME=/session/home -e CLAUDE_CONFIG_DIR=/session/config -e CLAUDE_CODE_OAUTH_TOKEN \
  mars-session-claude:$V claude --print --output-format stream-json --verbose \
  --forward-subagent-text --system-prompt-snapshot off --permission-mode bypassPermissions \
  --permission-prompts none --mcp-config /session/mcp.json "$@"; }
CONV="--input-format stream-json --include-partial-messages"
# 1. Conversational: feed stdin from a FIFO, one user line per turn, the next one only after the
#    previous `result` appears in $S/log/stream.jsonl; close the FIFO after the third `result`.
#    Line shape: {"type":"user","message":{"role":"user","content":[{"type":"text","text":"..."}]}}
#    Turn 1 "Read README.md and docs/CHANGELOG.md in full, then say what this project is."
#    Turn 2 "Use a subagent to grep for `main`, then edit src/app.py and commit as \"Add greeting\"."
#    Turn 3 "Run `curl -sS https://example.invalid/` once with the Bash tool. Do not retry."
run log $CONV < $S/in
# 2. SIGINT: start a long turn ("Write a 1200 word essay ..."), wait for stream_event lines, then
run log2 $CONV < $S/in &  podman kill --signal=SIGINT mars-verify; wait
# 3. Resume in a second container on the same config dir, with the session_id of run 1:
run log3 $CONV --resume <session_id> < $S/in
# 4. Ephemeral: the prompt as an argument, no --input-format, stdin closed:
run log3 -p "Reply with exactly the word PONG." < /dev/null
# 5. Scrub run 1 and install it (scrubber below), then check and test:
python3 scrub.py < $S/log/stream.jsonl > images/stub/fixtures/default.jsonl
grep -Ec 'sk-ant[-]|oauth|Bearer' images/stub/fixtures/default.jsonl   # must print 0
python3 -m unittest discover -s images/stub/tests && ENGINE=podman images/smoke-test.sh
```

## Observed on 2.1.274

- [x] Every flag accepted, including `--permission-prompts none`; no usage error in either mode.
- [x] `system`/`init` has `session_id`, `mcp_servers`, and also `model`, `tools`, `claude_code_version`, `permissionMode`, `apiKeySource`, `agents`, `skills`, `slash_commands`, `plugins`, `capabilities`, `memory_paths`, `output_style`. The CLI writes a new `init` line at the start of every turn, not once per process.
- [x] The unreachable MCP server is listed as `{"name":"mars-orchestrator","status":"failed","source":"dynamic"}` and the turn still runs.
- [x] The bare stdin line shape works; `session_id` and `parent_tool_use_id` are not needed on it.
- [x] All three turns produced `result` lines; closing stdin exits 0; `stderr.log` stayed empty.
- [x] `result.total_cost_usd` is cumulative for the process (0.0727, 0.1448, 0.1633); `modelUsage` likewise. `num_turns` is per turn (3, 5, 2). A resumed process starts again from zero.
- [x] SIGINT mid-turn: a `user` line with the text `[Request interrupted by user]`, then a `result` with `subtype: "error_during_execution"`, `is_error: true`, `terminal_reason: "aborted_streaming"`, exit 0 within seconds. PID 1 is `claude`; `Init: true` is not needed on rootless Podman.
- [x] `--resume` in a second container continued the conversation (it recalled the commit message) and its `init` reported the same `session_id`; no `resumed`-like field exists in `init`.
- [x] The transcript is at `<CLAUDE_CONFIG_DIR>/projects/-session-work/<session_id>.jsonl`.
- [x] The subagent tool is named `Agent` (not `Task`); its frames carry `parent_tool_use_id`, the first of them a `user` line with the subagent prompt. `system` lines `task_started`, `task_progress`, `task_updated` and `task_notification` surround it.
- [x] The denial arrives as `system`/`permission_denied` with `tool_name`, `tool_use_id`, `decision_reason_type`, `message` (no `decision_reason`), followed by a `tool_result` with `is_error: true`, and is listed in `result.permission_denials` as `{tool_name, tool_use_id, tool_input}`.
- [x] Line kinds the hand-authored fixture did not have: `rate_limit_event` (top-level type), `system` subtypes `status`, `thinking_tokens`, `vcs_state_changed`; `stream_event` also carries `thinking_delta`, `signature_delta`, `input_json_delta` and the message and block start and stop events. Thinking blocks arrive with empty `thinking` text and a `signature`.
- [x] Authentication failure (run with `sk-ant[-]fake-probe-0000` without the brackets): `system`/`api_retry` lines with `error_status: 401`, `error: "authentication_failed"`, then a synthetic assistant message and a `result`, exit 1.
- [x] Auto-update (Bears nnvb2): nothing was installed or migrated under `/session/home` or `CLAUDE_CONFIG_DIR` across six runs; `home` only gained `.cache/claude-cli-nodejs/`, and `claude --version` still reports the pin.
- [x] The credential value appears nowhere under the work, home, log or config directories.

## Scrubber

`scrub.py`: uuids (the session id included), `msg_`/`toolu_`/`req_` ids and task ids become sequential fakes, thinking signatures become `fixture-signature`, paths outside `/session/work` become `/scrubbed/path`, and the numbers in `rate_limit_info` (account usage) become 0. JSON is rewritten by the script only, never by hand.

```python
import json, re, sys
ids = {}
def fake(kind, fmt):
    def sub(m):
        if m.group(0) not in ids:
            ids[m.group(0)] = fmt % (sum(1 for v in ids.values() if v.startswith(kind)) + 1)
        return ids[m.group(0)]
    return sub
def walk(o, key=None):
    if isinstance(o, dict):
        return {k: "fixture-signature" if k == "signature" else walk(v, k) for k, v in o.items()}
    if isinstance(o, list):
        return [walk(v, key) for v in o]
    return o
def zero(o):
    if isinstance(o, dict): return {k: zero(v) for k, v in o.items()}
    return 0 if isinstance(o, (int, float)) and not isinstance(o, bool) else o
for line in sys.stdin:
    line = re.sub(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}", fake("00000000", "00000000-0000-4000-8000-%012d"), line)
    line = re.sub(r"\b(?:msg|toolu|req)_[A-Za-z0-9]{12,}", lambda m: fake(m.group(0)[:4], m.group(0).split("_")[0] + "_01FIXTURE%04d")(m), line)
    line = re.sub(r'(?<="task_id": ")[0-9a-f]{12,}|(?<=tasks/)[0-9a-f]{12,}', fake("task", "task%04d"), line)
    line = re.sub(r'/(?:tmp|session/config|session/home)/[^"\\\s]*', "/scrubbed/path", line)
    o = walk(json.loads(line))
    if "rate_limit_info" in o: o["rate_limit_info"] = zero(o["rate_limit_info"])
    print(json.dumps(o, separators=(",", ":")))
```
