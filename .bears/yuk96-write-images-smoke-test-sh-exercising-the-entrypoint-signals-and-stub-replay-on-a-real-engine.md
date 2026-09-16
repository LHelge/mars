---
id: yuk96
title: Write images/smoke-test.sh exercising the entrypoint, signals and stub replay on a real engine
status: open
priority: P1
created: "2026-09-16T20:29:45.029954126Z"
updated: "2026-09-16T20:29:45.029954126Z"
tags:
  - images
  - tests
depends_on:
  - "64jkc"
  - nfz7m
parent: deex5
---

## Summary
Write `images/smoke-test.sh`, a bash script that runs both built images through the engine named by `ENGINE` (`docker` or `podman`) and proves the image contract: stdout lands in `/session/log/stream.jsonl`, stderr in `stderr.log`, files append, the process is uid 1000 with the right home and cwd, the CLI is PID 1, `SIGINT` reaches it and produces a clean `result`, `SIGTERM` gives 143, and the stub replays a fixture under both `--input-format stream-json` and `-p`. It runs locally and in the images CI.

## Documents
- `ARCHITECTURE.md` "Session image" (contract; "If the engine tests show a signal not reaching PID 1 on either engine, the container is created with `Init: true`"), "Stop semantics" (SIGINT ends the turn and the CLI writes `result`; SIGTERM exit 143), "Session container specification" (User `1000:1000`, `UsernsMode: keep-id:uid=1000,gid=1000` on Podman only, `CapDrop ALL`, `no-new-privileges`), "Uid contract"
- `README.md` "CI" (Images: build session images; smoke-run the entrypoint)
- `docs/open-questions.md` item 7 (owned by the engine adapter epic; this script provides corroborating evidence only)
- ADR 0004, ADR 0010

## Acceptance criteria
- [ ] `images/smoke-test.sh` is bash, `set -euo pipefail`, `shellcheck` clean, executable, and takes `ENGINE` (default `docker`), `CLAUDE_IMAGE` (default `mars-session-claude:dev`) and `STUB_IMAGE` (default `mars-session-stub:dev`) from the environment; `--help` prints usage.
- [ ] Each check prints `ok: <name>` or `FAIL: <name>` and the script exits non-zero if any check fails, after cleaning up every container and temp directory it created (`trap` on EXIT).
- [ ] Containers are created with the session-equivalent host config: `--user 1000:1000`, `--workdir /session/work`, `-e HOME=/session/home`, `--cap-drop ALL`, `--security-opt no-new-privileges`, binds `<tmp>/work:/session/work`, `<tmp>/home:/session/home`, `<tmp>/log:/session/log`, and `--userns=keep-id:uid=1000,gid=1000` only when `ENGINE=podman`. Under Docker the temp directories are `chmod 0777` so uid 1000 can write them on a runner whose uid is not 1000.
- [ ] Check `entrypoint-redirect` (both images): run `sh -c 'echo "{\"type\":\"smoke\"}"; echo oops >&2; id -u; echo "$HOME"; pwd'` → `stream.jsonl` contains the JSON line, `1000`, `/session/home`, `/session/work` in that order; `stderr.log` contains `oops`; the container's own stdout (`$ENGINE logs`) is empty.
- [ ] Check `entrypoint-append`: run the same command again against the same `log` dir → `stream.jsonl` has two JSON lines.
- [ ] Check `entrypoint-noargs`: running with no command exits 2 and `stderr.log` contains `no command given`.
- [ ] Check `claude-version`: claude image runs `claude --version`; `stream.jsonl` contains the version read from `ARG CLAUDE_CODE_VERSION` in `images/claude/Dockerfile`; `git --version` and `bash -lc 'echo ok'` also succeed.
- [ ] Check `stub-oneshot`: stub runs `claude -p "hello" --output-format stream-json --verbose --permission-mode bypassPermissions --permission-prompts none` → exit 0, first line is `system`/`init`, the number of `"type":"result"` lines equals the count in `/opt/mars-stub/fixtures/default.jsonl` (read the fixture from the repo checkout: `images/stub/fixtures/default.jsonl`).
- [ ] Check `stub-interactive`: stub runs the full conversational argv from `ARCHITECTURE.md` "Claude Code invocation" (including `--mcp-config /session/mcp.json` with a bind-mounted temp `mcp.json` naming `mars-orchestrator`) with `-i` and stdin fed from a FIFO; after the `init` line appears, write one user line, wait for a `result`, write a second, wait for a second `result`; `init` lists `mars-orchestrator` as `connected`; close the FIFO → exit 0.
- [ ] Check `stub-sigint`: start the stub detached (`-d -i`, `MARS_STUB_LINE_DELAY_MS=300`), write one user line, wait until at least one non-init line is present, `$ENGINE exec <c> cat /proc/1/cmdline` contains `claude`, then `$ENGINE kill --signal=SIGINT <c>`; `$ENGINE wait <c>` returns 0 within 10 s and the last line of `stream.jsonl` is a `result`.
- [ ] Check `stub-sigterm`: same start, `$ENGINE kill --signal=SIGTERM <c>`; wait returns 143 and no `result` was appended after the kill.
- [ ] Real-CLI `SIGINT` behaviour is deliberately not checked here (it needs credentials); the claude image is only exercised through `claude --version`, `git --version` and `bash -l`. The credentialed verification task of this epic covers SIGINT against the real CLI.
- [ ] `SMOKE_KEEP_DIR=<path>`, when set, makes the script create its temp directories under that path and skip deleting them, so CI can upload `log/*` on failure.
- [ ] Total runtime under 3 minutes per engine on a GitHub runner.

## Implementation notes
- File: `images/smoke-test.sh`. Helper functions: `run_once <image> <logdir> <cmd...>`, `wait_for_line <file> <pattern> <timeout>`, `check <name> <cmd>`.
- Use `mktemp -d` under `${TMPDIR:-/tmp}`; on Podman the temp dir must be under the runner user's home for rootless bind mounts to work in CI (`$HOME/smoke-XXXX`).
- Read the pinned version: `sed -n 's/^ARG CLAUDE_CODE_VERSION=//p' images/claude/Dockerfile`.
- `$ENGINE wait` prints the exit code; on Podman use `podman wait` likewise. `podman kill --signal SIGINT` and `docker kill --signal=SIGINT` both accept the named signal.
- Stdin via FIFO: `mkfifo "$tmp/in"; $ENGINE run -i ... < "$tmp/in" & exec 3>"$tmp/in"; printf '%s\n' "$line" >&3; ...; exec 3>&-`.
- The user line: `{"type":"user","message":{"role":"user","content":[{"type":"text","text":"hello"}]}}`.
- Never print environment or the contents of `mcp.json` in the script output; the token in the smoke `mcp.json` is the literal `smoke-test-token`.

## Edge cases
- Rootless Podman on the CI runner may need `podman system migrate` or `XDG_RUNTIME_DIR`; if `--userns=keep-id:uid=1000,gid=1000` is rejected, fail with a clear message naming the flag (the engine adapter epic owns the startup probe that enforces this in production).
- `$ENGINE logs` may print an empty line; compare after trimming.
- If `SIGINT` does not reach PID 1 on one engine, the `stub-sigint` check fails and the finding is recorded as a new task linked to the engine adapter epic (the `Init: true` fallback), not worked around in this script.
- A leftover container name from an aborted run must not break the next run: name containers with a random suffix and `rm -f` in the trap.

## Testing
- The script is its own test; it must pass locally on rootless Podman and on Docker with both images built at `:dev`.
- `shellcheck images/smoke-test.sh` passes (CI lint job).

## Documentation
- None in this task (the README "CI" row already says "smoke-run the entrypoint"; the docs task mentions how to run the script locally).

## Assumes from other epics
- none