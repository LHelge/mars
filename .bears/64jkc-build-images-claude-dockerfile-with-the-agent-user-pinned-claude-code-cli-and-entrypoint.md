---
id: "64jkc"
title: Build images/claude/Dockerfile with the agent user, pinned Claude Code CLI and entrypoint
status: done
priority: P0
created: "2026-09-16T20:28:43.345672045Z"
updated: "2026-09-17T22:23:14.038987600Z"
tags:
  - images
depends_on:
  - mnjvj
parent: deex5
assignee: claude-opus-subagent
attempts: 1
---

## Summary
Write `images/claude/Dockerfile`, the v1 session image: an unprivileged `agent` user (uid 1000, gid 1000, `HOME=/session/home`), the Claude Code CLI pinned to one version (2.1.259 or later, required by `--permission-prompts none`) recorded in the image tag, `git`, `bash` and a base toolchain, and `mars-entrypoint` as the exec-form `ENTRYPOINT`. Profiles reference this image by name and projects may build their own on top of it.

## Documents
- `ARCHITECTURE.md` "Session image" (four-point contract), "Session container specification" (User `1000:1000`, workdir, env, terminal exec `/bin/bash -l` as `agent`), "Uid contract", "Claude Code invocation" (`--permission-prompts none` needs 2.1.259+; the version is recorded in the image tag), "Storage" (`CLAUDE_CONFIG_DIR` is a mount, not baked in)
- `README.md` "Session image" (`podman build -t mars-session-claude:dev images/claude`), "Configuration" (`SESSION_IMAGE_DEFAULT`)
- `SPEC.md` "WebSocket: session stream" (terminal is `/bin/bash -l` as `agent`)
- ADR 0012, ADR 0004

## Acceptance criteria
- [ ] `podman build -t mars-session-claude:dev images/claude` and the same with `docker build` succeed from a clean checkout; the build context is `images/claude` only.
- [ ] `ARG CLAUDE_CODE_VERSION=<x.y.z>` near the top of the Dockerfile is the single source of the pinned version (≥ 2.1.259); `LABEL org.opencontainers.image.version=$CLAUDE_CODE_VERSION` and `LABEL mars.claude_code_version=$CLAUDE_CODE_VERSION` are set; the build fails if `claude --version` does not contain that version (`RUN claude --version | grep -F "$CLAUDE_CODE_VERSION"`).
- [ ] User `agent` exists with uid 1000, gid 1000, login shell `/bin/bash`, home `/session/home`, no password; no other account has uid 1000 (the `node` base user is removed or renamed first).
- [ ] `/session/work`, `/session/home`, `/session/log` exist and are owned by `1000:1000`; `WORKDIR /session/work`; `USER agent`; `ENV HOME=/session/home`.
- [ ] `ENTRYPOINT ["/usr/local/bin/mars-entrypoint"]` with no `CMD`; the file is copied from `images/claude/mars-entrypoint` with mode 0755 and owned by root.
- [ ] `claude`, `git`, `bash`, `curl`, `ca-certificates`, `python3`, `build-essential`, `jq`, `less`, `procps` and `openssh-client` are on `PATH` for the `agent` user; `git --version`, `bash -lc 'echo ok'` and `claude --version` work when run as uid 1000.
- [ ] Running as uid 1000 with `-v <tmp>:/session/work`, the container runs `claude --version` through the entrypoint and the output lands in `/session/log/stream.jsonl` (checked by the smoke-test task).
- [ ] No credential, token or `.claude` configuration is baked in; `CLAUDE_CONFIG_DIR` is not set in the image (the launcher sets it per project).
- [ ] `images/claude/.dockerignore` excludes everything not needed (`tests/`, `*.md`).

## Implementation notes
- Files: `images/claude/Dockerfile`, `images/claude/.dockerignore`. `images/claude/mars-entrypoint` comes from the entrypoint task.
- Base: `node:22-bookworm-slim` (the CLI is an npm package needing Node 18+). Install with `npm install -g @anthropic-ai/claude-code@${CLAUDE_CODE_VERSION}` and `npm cache clean --force`. Do not use the curl installer: it installs per user under `~/.local`, which is a mount at runtime.
- The `node` image ships user `node` with uid 1000: run `userdel -r node` (ignore the missing-home error) before `groupadd -g 1000 agent && useradd -u 1000 -g 1000 -d /session/home -s /bin/bash -M agent`.
- Apt packages in one `RUN` with `--no-install-recommends` and `rm -rf /var/lib/apt/lists/*`. Keep the toolchain modest: the image is a base; per-project toolchains are a roadmap item (`README.md` "Roadmap after v1").
- `PATH` must include the npm global bin (`/usr/local/bin` on the node image) for the `agent` user; verify with `su agent -c 'command -v claude'` in a build-time `RUN` if in doubt.
- Do not set `ENV ANTHROPIC_API_KEY` or `CLAUDE_CODE_OAUTH_TOKEN` placeholders: the launcher injects exactly one of them (`ARCHITECTURE.md` "Claude Code invocation", credentials paragraph).
- Tag convention (documented by the docs task): `mars-session-claude:<CLAUDE_CODE_VERSION>` plus `mars-session-claude:latest`; CI derives the version with `sed -n 's/^ARG CLAUDE_CODE_VERSION=//p' images/claude/Dockerfile`.
- Choosing the version: take the newest published `@anthropic-ai/claude-code` ≥ 2.1.259 at implementation time. The adapter epic's live probe is the final authority and bumps this ARG if needed; a bump is a one-line change here plus the README example.

## Edge cases
- Under rootless Podman with `keep-id:uid=1000,gid=1000` and under Docker with `user: 1000:1000`, nothing in the image may depend on writing outside `/session/*`, `/tmp` or the writable root; `npm` and `claude` must not need a writable `/usr/local`. Verify `claude --version` as `agent` at build time.
- `/session/home` is empty at first launch: the CLI must not require a pre-existing `~/.claude` there (it uses `CLAUDE_CONFIG_DIR`). If the CLI writes anything under `HOME` regardless, that is fine, the directory is a writable mount.
- Locale: set `LANG=C.UTF-8` so tool output with non-ASCII does not break the JSON stream.
- Keep image size reasonable (< 1.5 GB); do not install language toolchains beyond `build-essential` and `python3`.

## Testing
- Build on both engines in the images CI (CI task); `hadolint` (via the `hadolint/hadolint-action` or a container) runs in the lint job with `DL3008` (unpinned apt versions) allowed.
- The smoke-test task verifies uid, home, cwd, `claude --version` matches the ARG, `git --version`, `bash -l`, and the entrypoint redirects.
- No cargo or frontend chain applies.

## Documentation
- None in this task; `README.md` "Session image" and the tag convention are updated by the documentation task once the version and tag names are final.

## Assumes from other epics
- none (the engine adapter epic's startup probe pulls this image by the `SESSION_IMAGE_DEFAULT` name; the name itself is fixed by the documentation task).