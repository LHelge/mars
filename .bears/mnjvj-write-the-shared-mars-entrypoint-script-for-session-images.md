---
id: mnjvj
title: Write the shared mars-entrypoint script for session images
status: open
priority: P0
created: "2026-09-16T20:26:57.940000686Z"
updated: "2026-09-16T20:51:51.736512363Z"
tags:
  - images
depends_on:
  - sywed
parent: deex5
---

## Summary
Write `/usr/local/bin/mars-entrypoint`, the script every session image must ship. It `cd`s to `/session/work`, sets up the environment, and `exec`s the command the orchestrator passes as the container command with stdout appended to `/session/log/stream.jsonl` and stderr appended to `/session/log/stderr.log`, so the agent CLI becomes PID 1 and receives `SIGINT`/`SIGTERM` from `kill` directly. This is the contract both `images/claude` and `images/stub` build on (ADR 0010's `tee` realised as a redirect).

## Documents
- `ARCHITECTURE.md` "Session image" (the four-point image contract, entrypoint bullet, `Init: true` fallback note)
- `ARCHITECTURE.md` "Storage" (`sessions/<sid>/log/stream.jsonl`, `stderr.log`)
- `ARCHITECTURE.md` "Session container specification" (User `1000:1000`, working directory `/session/work`, `HOME=/session/home`, stdin attached, stdout/stderr not attached)
- `ARCHITECTURE.md` "Secrets", "Injection" (the entrypoint is where the tmpfs-file export pattern goes later)
- ADR 0010, ADR 0012

## Acceptance criteria
- [ ] `images/claude/mars-entrypoint` exists, is POSIX `sh` (`#!/bin/sh`, no bashisms), executable, and `shellcheck` clean.
- [ ] With no arguments it prints `mars-entrypoint: no command given` to stderr and exits 2.
- [ ] It runs `cd /session/work` (exit 1 with a stderr message if the directory does not exist), `mkdir -p /session/log`, exports `HOME=${HOME:-/session/home}`, and then `exec "$@" >>/session/log/stream.jsonl 2>>/session/log/stderr.log`.
- [ ] After `exec`, `/proc/1/cmdline` inside the container is the passed command, not `sh` (verified by the smoke-test task).
- [ ] Both log files are opened in append mode: running the entrypoint twice against the same `/session/log` yields both runs' output in order.
- [ ] The script never echoes environment variables or arguments to either log or to the container's own stdout (secrets rule 3 in `CLAUDE.md`); the only diagnostics are the two error cases above.
- [ ] A comment block at the top states the contract and marks where the future `/run/secrets` export-and-delete step is inserted (before the `exec`), without implementing it.

## Implementation notes
- File: `images/claude/mars-entrypoint` (canonical copy). The stub image task copies it byte-for-byte to `images/stub/mars-entrypoint`; the CI task asserts the two are identical with `cmp`. The build context per `README.md` "Session image" is `images/claude`, which is why the file cannot live one level up.
- Shape:
  ```sh
  #!/bin/sh
  set -eu
  [ "$#" -gt 0 ] || { echo "mars-entrypoint: no command given" >&2; exit 2; }
  cd /session/work || { echo "mars-entrypoint: /session/work missing" >&2; exit 1; }
  mkdir -p /session/log
  export HOME="${HOME:-/session/home}"
  # future: export /run/secrets/* into the environment and delete the files (ARCHITECTURE.md, "Secrets", "Injection")
  exec "$@" >>/session/log/stream.jsonl 2>>/session/log/stderr.log
  ```
- Do not use `tee`, a subshell, or a wrapper process: nothing reads the container's stdout, and a wrapper would make the CLI PID 2 and swallow signals.
- Do not set `PATH` beyond what the image already provides; the Dockerfile owns `PATH`.
- Do not `chown` or fix permissions on `/session/*`; the uid contract (`ARCHITECTURE.md` "Uid contract") makes the mounts already writable by uid 1000.

## Edge cases
- `/session/log` is normally a bind mount and already exists; `mkdir -p` covers the smoke test and ad-hoc `docker run` use.
- The entrypoint may be invoked with the CLI plus many flags including values with spaces (`--append-system-prompt "..."`); `exec "$@"` preserves them. Never build a string and `eval` it.
- Stdin must pass through untouched: the orchestrator attaches stdin and writes JSON lines to it.
- PID-1 signal handling belongs to the CLI (or the stub) itself; if the engine tests (Container engine adapter epic) find a signal not reaching PID 1, the container gets `Init: true` and this script does not change.

## Testing
- `shellcheck images/claude/mars-entrypoint` passes (run in the images CI lint job).
- Behaviour is exercised in a real container by the smoke-test task (stdout to `stream.jsonl`, stderr to `stderr.log`, append semantics, `id -u` = 1000, `pwd` = `/session/work`, `$HOME` = `/session/home`, PID 1 is the command).
- No cargo or frontend chain applies to this task.

## Documentation
- None: implements `ARCHITECTURE.md` "Session image" as written. (The tag convention and stub details are written back by the documentation task of this epic.)

## Assumes from other epics
- "Repository scaffolding, tooling and CI" has created the repository skeleton; nothing else.