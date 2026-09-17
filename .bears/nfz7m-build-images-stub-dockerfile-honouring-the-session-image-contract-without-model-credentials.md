---
id: nfz7m
title: Build images/stub/Dockerfile honouring the session image contract without model credentials
status: done
priority: P1
created: "2026-09-16T20:29:06.261713901Z"
updated: "2026-09-17T22:25:57.393478513Z"
tags:
  - images
  - tests
depends_on:
  - mnjvj
  - ajyxd
  - t2ecg
parent: deex5
assignee: claude-opus-subagent
attempts: 1
---

## Summary
Write `images/stub/Dockerfile`: a small image with the same contract as the claude image (`agent` uid 1000, `HOME=/session/home`, `/session/*` directories, `mars-entrypoint`, `git`, `bash`) whose `/usr/local/bin/claude` is the stub script and which ships the default fixtures under `/opt/mars-stub/fixtures/`. The orchestrator launches it with the unchanged Claude launch command, so Playwright and the session-owner tests run against real containers without credentials.

## Documents
- `ARCHITECTURE.md` "Session image" (contract and stub paragraph), "Session container specification", "Uid contract"
- `README.md` "CI" (E2E runs against the stub image; Images builds both images)
- `SPEC.md` "Test-only routes" (Playwright sessions use the stub image), "WebSocket: session stream" (terminal exec `/bin/bash -l`)
- ADR 0010

## Acceptance criteria
- [ ] `podman build -t mars-session-stub:dev images/stub` and `docker build` succeed with `images/stub` as the only build context.
- [ ] Base `python:3.13-slim` (or the current stable 3.x slim); `bash`, `git` and `ca-certificates` installed; no Node, no Claude Code package, no network access needed at runtime.
- [ ] User `agent` uid 1000 gid 1000, shell `/bin/bash`, home `/session/home`; `/session/work`, `/session/home`, `/session/log` owned by `1000:1000`; `WORKDIR /session/work`; `USER agent`; `ENV HOME=/session/home LANG=C.UTF-8 PYTHONUNBUFFERED=1`.
- [ ] `images/stub/mars-entrypoint` is a byte-identical copy of `images/claude/mars-entrypoint` (the CI task enforces this with `cmp`), installed at `/usr/local/bin/mars-entrypoint` mode 0755; `ENTRYPOINT ["/usr/local/bin/mars-entrypoint"]`, no `CMD`.
- [ ] `images/stub/claude` installed at `/usr/local/bin/claude` mode 0755; `claude --version` as `agent` prints `0.0.0-stub (Claude Code stub)`.
- [ ] `images/stub/fixtures/*.jsonl` copied to `/opt/mars-stub/fixtures/`, world-readable; `LABEL mars.stub=true` and `LABEL org.opencontainers.image.title="mars-session-stub"`.
- [ ] Running the container as uid 1000 with the documented conversational argv (`claude --print --output-format stream-json --input-format stream-json --verbose --forward-subagent-text --system-prompt-snapshot off --permission-mode bypassPermissions --permission-prompts none --mcp-config /session/mcp.json`) and two stdin lines writes an `init` line and two turns to `/session/log/stream.jsonl` and exits 0 on EOF; the same with `-p "hello"` replays every turn and exits 0 (verified by the smoke-test task).
- [ ] `images/stub/.dockerignore` excludes `tests/` and `*.md`.

## Implementation notes
- Files: `images/stub/Dockerfile`, `images/stub/.dockerignore`, `images/stub/mars-entrypoint` (copy). Script and fixtures come from their tasks.
- Mirror the claude Dockerfile's user and directory stanza verbatim so the two images cannot drift on the contract; `python:*-slim` has no uid-1000 user, so no `userdel` is needed, but keep the `getent passwd 1000` guard.
- `git` is included so the terminal escape hatch (`/bin/bash -l`) can be used by end-to-end tests to make a commit on the session branch for hand-off scenarios; the stub itself executes nothing.
- The launcher passes `--mcp-config /session/mcp.json`; the stub reads it to populate `mcp_servers`. Nothing in the image may pre-create `/session/mcp.json` (it is a read-only bind mount at runtime).
- Do not install `CLAUDE_CONFIG_DIR` or fixtures under `/session`; those paths are mounts.

## Edge cases
- Image must work when `/session/mcp.json` is absent (ad-hoc runs): the stub warns and reports an empty `mcp_servers` list.
- `PYTHONUNBUFFERED=1` plus the script's own flushing guarantees the owner's tail sees lines promptly; without it a slow test could time out waiting for `init`.
- Keep the image small (< 200 MB) because the E2E CI builds it on every `orchestrator/**` or `frontend/**` change.

## Testing
- Build on both engines in the images CI; `hadolint` in the lint job.
- Smoke-test task covers: entrypoint redirects, uid/home/cwd, `-p` replay count equals the fixture's `result` count, interactive two-turn run, SIGINT → `result` + exit 0, SIGTERM → 143, PID 1 is the stub.
- No cargo or frontend chain applies.

## Documentation
- None in this task; the docs task adds the stub build command to `README.md` "Session image" and the stub's knobs to `ARCHITECTURE.md` "Session image".

## Assumes from other epics
- "End-to-end tests with Playwright" and "Session lifecycle" epics consume this image by the name `mars-session-stub:<tag>` through a profile's `image` (or `SESSION_IMAGE_DEFAULT` in their test configuration); they do not build it themselves beyond calling the documented build command.