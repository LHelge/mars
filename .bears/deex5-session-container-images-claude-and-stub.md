---
id: deex5
title: "Session container images: claude and stub"
type: epic
status: open
priority: P1
created: "2026-09-16T20:12:25.829216888Z"
updated: "2026-09-16T20:15:17.258514024Z"
tags:
  - images
depends_on:
  - sywed
---

## Scope

The two session images and their CI.

- `images/claude/Dockerfile`: unprivileged `agent` user (uid 1000, `HOME=/session/home`), Claude Code CLI pinned to a version recorded in the image tag (2.1.259 or later for `--permission-prompts none`), `git` and a base toolchain, `/usr/local/bin/mars-entrypoint` that `cd`s to `/session/work`, sets up the environment and `exec`s the orchestrator's command with stdout appended to `/session/log/stream.jsonl` and stderr to `/session/log/stderr.log`, so the CLI is PID 1.
- `images/stub/`: same contract; its "CLI" emits a `system`/`init` line, replays a fixture transcript (one turn per stdin line, or the whole file under `-p`), exits cleanly on `SIGINT`. Used by Playwright and session-owner tests.
- Images CI workflow: build both images on `images/**` changes and smoke-run the entrypoint.
- `README.md` "Session image" build instructions verified; `SESSION_IMAGE_DEFAULT` default documented.

## Documents

`ARCHITECTURE.md` "Session image"; `README.md` "Development", "CI"; ADR 0010, 0012.

## Acceptance criteria

- [ ] Both images build; the entrypoint smoke test shows stdout landing in `stream.jsonl` and `SIGINT` reaching the process.
- [ ] The stub replays a fixture end to end under both `--input-format stream-json` and `-p` modes.
- [ ] Images CI is green.

## Out of scope

Per-project toolchain setup scripts (roadmap).