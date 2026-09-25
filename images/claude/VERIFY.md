# Verifying the claude session image with real credentials

Manual and credentialed; never runs in CI. The pin moved to 2.1.282 on 2026-09-25 on the strength of the live probe (`orchestrator/tests/fixtures/claude/2.1.282/NOTES.md`: the same line kinds, the same `init` per turn, the same subagent tool names and the same authentication-failure shape) and the MCP recording below; the procedure below was not repeated for it. Last full run: Claude Code 2.1.274, model `claude-sonnet-5`, rootless Podman 6.1.2, 2026-09-18, `CLAUDE_CODE_OAUTH_TOKEN` from `claude setup-token`. About 0.35 USD of reported cost for all runs. The credential lives only in the operator's shell (rule 3 of `CLAUDE.md`).

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
run() { log=$1; shift; podman run --rm --name mars-verify --user 1000:1000 \
  --userns=keep-id:uid=1000,gid=1000 --cap-drop ALL --security-opt no-new-privileges \
  -v $S/work:/session/work -v $S/home:/session/home -v $S/$log:/session/log \
  -v $S/config:/session/config -v $S/mcp.json:/session/mcp.json:ro \
  -e HOME=/session/home -e CLAUDE_CONFIG_DIR=/session/config -e CLAUDE_CODE_OAUTH_TOKEN \
  mars-session-claude:$V claude --print --output-format stream-json --verbose \
  --forward-subagent-text --system-prompt-snapshot off --permission-mode bypassPermissions \
  --permission-prompts none --mcp-config /session/mcp.json "$@"; }
CONV="--input-format stream-json --include-partial-messages"
# The entrypoint gives the CLI the FIFO /tmp/mars-stdin as stdin (ADR 0034); a line reaches it the
# way the engine adapter's relay writes one. The CLI never sees EOF, so a run is ended with SIGINT.
feed() { podman exec -i mars-verify sh -c 'exec cat >/tmp/mars-stdin'; }   # echo '<line>' | feed
# 1. Conversational: one user line per turn through `feed`, the next one only after the previous
#    `result` appears in $S/log/stream.jsonl; `podman kill --signal=SIGINT mars-verify` after the third.
#    Line shape: {"type":"user","message":{"role":"user","content":[{"type":"text","text":"..."}]}}
#    Turn 1 "Read README.md and docs/CHANGELOG.md in full, then say what this project is."
#    Turn 2 "Use a subagent to grep for `main`, then edit src/app.py and commit as \"Add greeting\"."
#    Turn 3 "Run `curl -sS https://example.invalid/` once with the Bash tool. Do not retry."
run log $CONV &
# 2. SIGINT: start a long turn ("Write a 1200 word essay ..."), wait for stream_event lines, then
run log2 $CONV &  podman kill --signal=SIGINT mars-verify; wait
# 3. Resume in a second container on the same config dir, with the session_id of run 1:
run log3 $CONV --resume <session_id> &
# 4. Ephemeral: the prompt as an argument, no --input-format, nothing written to stdin:
run log3 -p "Reply with exactly the word PONG."
# 5. Scrub run 1 and install it (scrubber below), then check and test:
python3 scrub.py < $S/log/stream.jsonl > images/stub/fixtures/default.jsonl
grep -Ec 'sk-ant[-]|oauth|Bearer' images/stub/fixtures/default.jsonl   # must print 0
python3 -m unittest discover -s images/stub/tests && ENGINE=podman images/smoke-test.sh
```

## Observed on 2.1.274

- [x] Every flag accepted, including `--permission-prompts none`; no usage error in either mode.
- [x] `system`/`init` has `session_id`, `mcp_servers`, and also `model`, `tools`, `claude_code_version`, `permissionMode`, `apiKeySource`, `agents`, `skills`, `slash_commands`, `plugins`, `capabilities`, `memory_paths`, `output_style`. The CLI writes a new `init` line at the start of every turn, not once per process; the stub follows it (`ARCHITECTURE.md`, "Session image").
- [x] Under `--input-format stream-json` nothing is written until the first stdin line, `init` included: a probe that waited for `init` before writing saw no output for 120 s, while writing first produced `init` within a second (`orchestrator/tests/fixtures/claude/2.1.274/NOTES.md`, `stdin_shape`). The session therefore goes `running` on stdin attach and `cli_session_id` is stored when `init` arrives (ADR 0032); the stub writes nothing before its first stdin line either.
- [x] The unreachable MCP server is listed as `{"name":"mars-orchestrator","status":"failed","source":"dynamic"}` and the turn still runs.
- [x] The bare stdin line shape works; `session_id` and `parent_tool_use_id` are not needed on it.
- [x] All three turns produced `result` lines; closing stdin exits 0; `stderr.log` stayed empty.
- [x] With stdin on the entrypoint's read-write FIFO (ADR 0034), a line written through `podman exec -i … cat >/tmp/mars-stdin` starts a turn, and the CLI is still PID 1 and running after that exec has ended: a writer leaving is not an EOF. (Observed without credentials, on the synthetic authentication-failure turn; "closing stdin exits 0" above was observed on a plain pipe, before the FIFO.)
- [x] `result.total_cost_usd` is cumulative for the process (0.0727, 0.1448, 0.1633); `modelUsage` likewise. `num_turns` is per turn (3, 5, 2). A resumed process starts again from zero.
- [x] SIGINT mid-turn: a `user` line with the text `[Request interrupted by user]`, then a `result` with `subtype: "error_during_execution"`, `is_error: true`, `terminal_reason: "aborted_streaming"`, exit 0 within seconds; the stub writes the same shape. PID 1 is `claude`; `Init: true` is not needed on rootless Podman.
- [x] `--resume` in a second container continued the conversation (it recalled the commit message) and its `init` reported the same `session_id`; no `resumed`-like field exists in `init`.
- [x] The transcript is at `<CLAUDE_CONFIG_DIR>/projects/-session-work/<session_id>.jsonl`.
- [x] The subagent tool is named `Agent` (not `Task`) in the `tool_use` frames, while `init.tools` still lists it as `Task`; the stub therefore lists both (`ARCHITECTURE.md`, "Session image"). Its frames carry `parent_tool_use_id`, the first of them a `user` line with the subagent prompt. `system` lines `task_started`, `task_progress`, `task_updated` and `task_notification` surround it.
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

## Recording the MCP conformance fixtures

`orchestrator/tests/fixtures/mcp/<version>/` is what the pinned CLI sent the Mars MCP server, one request per file, and `orchestrator/tests/mcp_conformance.rs` replays it on every test run (`CLAUDE.md`, "Testing expectations"). Record a new directory whenever `CLAUDE_CODE_VERSION` changes; the suite's `the_pinned_cli_version_has_been_recorded` fails until you do. Last run: Claude Code 2.1.282, rootless Podman 6.1.2, 2026-09-25.

```bash
podman build -t mars-session-claude:$V images/claude     # the image under test, as above
CARGO_TARGET_DIR=$PWD/orchestrator/target-main scripts/mcp-record/record.sh   # KEEP=1 keeps the scratch directory
```

`scripts/mcp-record/record.sh` starts the `record` scenario of the conformance suite, which serves the real `mcp_router` behind a recorder on host port 7001 with one seeded running session and writes that session's `mcp.json`. It then starts the pinned image with the flags a conversational launch uses and `--add-host orchestrator:host-gateway`, so the CLI dials the default `MCP_URL`, `http://orchestrator:7001/mcp`, and the recorded `Host` is the one a compose deployment's sessions send. One stdin line starts a turn, and a `SIGINT` ends the process the way the session owner stops one. The recorder keeps the method, the path, the headers in `RECORDED_HEADERS` (`tests/common/mcp_fixtures.rs`; never `Authorization`), the JSON body and the status of every request. The script refuses to keep a directory in which `Bearer`, `sk-ant-` or `Authorization` appears.

**No model credential is needed.** The model is `scripts/mcp-record/fake_anthropic.py`, a scripted Messages API reached through `ANTHROPIC_BASE_URL` with an obviously fake key: its first answers are `tool_use` blocks for `ready`, for `get_task` on a task that does not exist and for a tool that was never listed, then a closing text turn. The CLI connects to its MCP servers without any credential; only a tool call needs a model to ask for it, and the fake one does.

Observed on 2.1.274:

- [x] The CLI negotiates protocol revision `2026-07-28` statelessly: `server/discover` (id `server-discover-probe-1`), `tools/list`, then one `tools/call` per tool use. Each is one POST with `Accept: application/json, text/event-stream`, `MCP-Protocol-Version`, `Mcp-Method`, `Mcp-Name` on a call, and the request metadata (`protocolVersion`, `clientCapabilities`, `clientInfo`) in `params._meta`. There is no `initialize`, no `Mcp-Session-Id`, no SSE `GET` and no `DELETE`. Stopping the process with `SIGINT` sends nothing either.
- [x] A call carries `_meta["claudecode/toolUseId"]` (the model's `tool_use` id) and a `progressToken`.
- [x] A tool that was not in the listing is never sent: the CLI answers the model itself with `No such tool available: mcp__mars-orchestrator__no_such_tool`. The conformance suite therefore replays the recorded `ready` call under an unknown name to check the server's error, and does not record it.
- [x] A tool failure answered as a JSON-RPC error (`get_task` → `-32004`, `task not found`) reaches the model as a `tool_result` with `is_error: true` and the message as its text.
- [x] What the CLI accepts on this revision was read from the result schemas bundled in the binary (`strings` of `claude.exe`, searching for `cacheScope`). Every result has a `resultType` that must be `complete` for the methods Mars serves. A list result needs an integer `ttlMs` of at least 0 and a `cacheScope` of `public` or `private`. A tool's `inputSchema` needs `type: "object"`. A call result needs a `content` array and allows a boolean `isError`. `tests/common/mcp_2026_07_28.rs` mirrors exactly this and quotes the definitions.
- [x] The CLI's own account of the connection is in `home/.cache/claude-cli-nodejs/-session-work/mcp-logs-mars-orchestrator/*.jsonl`, which the script prints with any bearer value replaced. It shows `protocolEra: "modern"`, `negotiatedProtocolVersion: "2026-07-28"` and one line per tool call.

Observed on 2.1.282 (2026-09-25): the same four requests on the same revision, `2026-07-28`. The one difference in what the client sends is that `clientCapabilities.elicitation` now names its two modes, `{"form":{},"url":{}}`, where 2.1.274 sent `{}`; Mars never elicits, so nothing answers it. The result schemas bundled in the binary are unchanged apart from the minifier's names, so `tests/common/mcp_2026_07_28.rs` stands.
