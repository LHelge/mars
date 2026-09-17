---
id: ajyxd
title: Implement the stub CLI script that replays a fixture transcript
status: done
priority: P1
created: "2026-09-16T20:27:34.420418373Z"
updated: "2026-09-17T22:14:43.257478454Z"
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
Write the stub "CLI" installed as `/usr/local/bin/claude` in `images/stub`: a dependency-free Python 3 script that accepts the same argv the orchestrator's Claude backend builds, emits a `system`/`init` line, replays a `stream-json` fixture one turn per stdin line (or the whole file under `-p`), and exits cleanly on `SIGINT`. It needs no model credentials, so the session-owner tests and Playwright run against real containers.

## Documents
- `ARCHITECTURE.md` "Session image" (stub paragraph), "Claude Code invocation" (the exact argv it must accept), "Input encoding" (the stdin user-message shape), "Stop semantics" (SIGINT ends the turn and lets the CLI write `result`; SIGTERM is exit 143), "MCP design" (`mars-orchestrator` server name; `launch_warning` when `init` does not list it as connected), "Storage" (transcript at `CLAUDE_CONFIG_DIR/projects/-session-work/<cli_session_id>.jsonl`)
- `SPEC.md` "AgentEvent" (translation rules: which native shapes matter), "Test-only routes" (Playwright uses the stub)
- ADR 0003, ADR 0010

## Acceptance criteria
- [ ] `images/stub/claude` is a Python 3 script (`#!/usr/bin/env python3`, standard library only, no third-party imports), executable, and passes `python3 -m py_compile`.
- [ ] Accepts and ignores every flag from `ARCHITECTURE.md` "Claude Code invocation": value-less `--print`/`-p`, `--verbose`, `--forward-subagent-text`, `--include-partial-messages`, `--strict-mcp-config`, `--bare`; valued `--output-format`, `--input-format`, `--model`, `--append-system-prompt`, `--mcp-config`, `--permission-mode`, `--permission-prompts`, `--system-prompt-snapshot`, `--resume` (both `--flag value` and `--flag=value`). An unknown `--flag` is warned about on stderr once and ignored. `--version` prints `0.0.0-stub (Claude Code stub)` and exits 0. `--help` prints a one-paragraph usage and exits 0.
- [ ] Mode selection: `--input-format stream-json` selects interactive mode; otherwise the first positional argument is the prompt and the run is one-shot; with neither, all of stdin is read as the prompt and the run is one-shot.
- [ ] First output line is always `{"type":"system","subtype":"init","cwd":"/session/work","session_id":"<id>","tools":["Bash","Read","Edit","Write","Glob","Grep","Task"],"mcp_servers":[...],"model":"stub","permissionMode":"bypassPermissions"}` where `<id>` is the `--resume` value when given, else a fresh UUID v4, and `mcp_servers` lists every key of `mcpServers` in the `--mcp-config` file as `{"name": <key>, "status": "connected"}` (empty list if the flag is absent or the file unreadable, with a stderr warning).
- [ ] Fixture: read from `MARS_STUB_FIXTURE` (default `/opt/mars-stub/fixtures/default.jsonl`); a missing or unreadable file prints `stub: fixture not found: <path>` to stderr and exits 1 before any output. Fixture lines of type `system` with subtype `init` are skipped (the stub emits its own). `stream_event` lines are emitted only when `--include-partial-messages` was passed. Every emitted line has its `session_id` field set to the current session id.
- [ ] A turn is the fixture lines up to and including the next line with `"type":"result"`. Interactive mode: for each JSON line read from stdin, emit the next turn; when the fixture is exhausted, synthesise a turn: one `assistant` message with a single text block `Stub reply to: <text of the user message, or the raw line if not parseable>` followed by a `result` line (`subtype: "success"`, `is_error: false`, `num_turns: 1`, `duration_ms`, `total_cost_usd: 0.001`, `usage: {"input_tokens":10,"output_tokens":5}`, `permission_denials: []`). EOF on stdin exits 0. One-shot mode: emit every turn back to back, then exit 0.
- [ ] `MARS_STUB_LINE_DELAY_MS` (default `0`) sleeps that long between emitted lines; every line is flushed immediately after writing.
- [ ] `MARS_STUB_EXIT_AFTER_TURNS=<n>` (default unset) exits with `MARS_STUB_EXIT_CODE` (default `1`) immediately after emitting the n-th `result`, so the lifecycle tests can produce a `failed` session.
- [ ] Signals: handlers are installed explicitly (PID 1 gets no default dispositions). `SIGINT` mid-turn: stop emitting, write a `result` line with `subtype: "success"`, `is_error: false` and the counters of the interrupted turn, then exit 0. `SIGINT` while waiting for stdin: exit 0. `SIGTERM`: exit 143 at once without a `result`.
- [ ] When `CLAUDE_CONFIG_DIR` is set, every emitted line is also appended to `$CLAUDE_CONFIG_DIR/projects/-session-work/<session_id>.jsonl` (directories created as needed); `--resume <id>` with no such file prints a stderr warning and continues. Failure to write the transcript copy is a stderr warning, never fatal.
- [ ] Never prints anything but JSON lines to stdout; all diagnostics go to stderr.

## Implementation notes
- File: `images/stub/claude` (installed by the stub Dockerfile task as `/usr/local/bin/claude`). Keep it in one file under ~300 lines; structure as `parse_args`, `load_turns`, `emit`, `run_interactive`, `run_oneshot`, `main`.
- Argument parsing: hand-rolled loop over `sys.argv[1:]`; do not use `argparse` (its unknown-option handling and `-p` semantics fight the real CLI's grammar).
- Stdin reading in interactive mode uses `sys.stdin.readline()` in a loop; `select`/threads are unnecessary because SIGINT handling is done by raising a custom exception from the signal handler that the emit loop and the readline loop both catch.
- Use `sys.stdout.write(json.dumps(obj, separators=(",", ":")) + "\n"); sys.stdout.flush()` for every line; set `sys.stdout.reconfigure(line_buffering=True)` at start.
- The stdin user-message shape to expect: `{"type":"user","message":{"role":"user","content":[{"type":"text","text":"..."}]}}` (`ARCHITECTURE.md` "Input encoding"); extract `text` defensively (also accept `content` as a plain string).
- Keep the `mcp_servers` name `mars-orchestrator` exactly as in `ARCHITECTURE.md` "MCP design"; the launcher warns when it is missing, and tests for that warning use a config without it.

## Edge cases
- A fixture whose last turn lacks a `result` line: treat the trailing lines as a turn and synthesise the closing `result`.
- An empty fixture (zero turns): interactive mode goes straight to synthesised replies; one-shot mode emits `init` and one synthesised `result` then exits 0.
- Malformed JSON in a fixture line: stderr warning, line skipped.
- A blank or malformed stdin line still consumes a turn (the real CLI would answer something); do not hang.
- Delay sleeps must be interruptible by SIGINT (sleep in small slices or check the interrupt flag after `time.sleep`).
- Output pipe closed (`BrokenPipeError`): exit 0 quietly.

## Testing
- `images/stub/tests/test_claude.py` with `unittest` (run with `python3 -m unittest discover -s images/stub/tests`), executing the script as a subprocess with a temporary fixture and asserting: init line first and its `session_id` reuse under `--resume`; `mcp_servers` from a temp `--mcp-config`; one-shot emits all turns and exits 0; interactive emits one turn per stdin line, synthesises after exhaustion, exits 0 on EOF; `stream_event` lines gated by `--include-partial-messages`; `MARS_STUB_EXIT_AFTER_TURNS`/`MARS_STUB_EXIT_CODE`; SIGINT mid-turn (use `MARS_STUB_LINE_DELAY_MS=200`) yields a trailing `result` and exit 0; SIGTERM yields 143; transcript copy written under a temp `CLAUDE_CONFIG_DIR`; missing fixture exits 1.
- These tests run on the host without a container and are wired into the images CI lint job by the CI task.

## Documentation
- None in this task; the stub's env knobs and fixture path are written into `ARCHITECTURE.md` "Session image" by the documentation task of this epic.

## Assumes from other epics
- none