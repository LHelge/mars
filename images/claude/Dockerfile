# Mars v1 session image (ARCHITECTURE.md, "Session image").
#
# Honours the four-point session image contract: an unprivileged `agent` user
# (uid 1000) with HOME=/session/home, the Claude Code CLI on PATH pinned to the
# version recorded in the image tag, `git` plus a modest base toolchain, and
# /usr/local/bin/mars-entrypoint as the entrypoint.
#
# Build (README.md, "Session image"), tagged with the pinned CLI version and
# the `latest` alias that SESSION_IMAGE_DEFAULT names:
#   podman build -t mars-session-claude:$(sed -n 's/^ARG CLAUDE_CODE_VERSION=//p' images/claude/Dockerfile) -t mars-session-claude:latest images/claude
#
# The CLI is installed from npm rather than the curl installer: that installer
# puts the binary under ~/.local, which is a mount at runtime.
FROM node:22-bookworm-slim

# The single source of truth for the pinned CLI version. `--permission-prompts
# none` (ARCHITECTURE.md, "Claude Code invocation") requires 2.1.259 or later.
# CI and the README derive the value with:
#   sed -n 's/^ARG CLAUDE_CODE_VERSION=//p' images/claude/Dockerfile
# so this line stays exactly `ARG CLAUDE_CODE_VERSION=<x.y.z>`, unquoted.
ARG CLAUDE_CODE_VERSION=2.1.274

LABEL org.opencontainers.image.title="mars-session-claude" \
      org.opencontainers.image.description="Mars v1 session image: Claude Code CLI as the unprivileged agent user" \
      org.opencontainers.image.source="https://github.com/LHelge/mars" \
      org.opencontainers.image.version=$CLAUDE_CODE_VERSION
LABEL mars.claude_code_version=$CLAUDE_CODE_VERSION

# Non-ASCII tool output must not break the stream-json lines the orchestrator
# tails out of /session/log/stream.jsonl.
ENV LANG=C.UTF-8

# `pipefail` so the version assertion below fails when `claude --version` does.
SHELL ["/bin/bash", "-o", "pipefail", "-c"]

# The base toolchain. Deliberately modest: this image is a base that profiles
# and per-project images build on (README.md, "Roadmap after v1").
RUN apt-get update \
    && apt-get install --no-install-recommends -y \
        bash \
        build-essential \
        ca-certificates \
        curl \
        git \
        jq \
        less \
        openssh-client \
        procps \
        python3 \
    && rm -rf /var/lib/apt/lists/*

# The node base image ships a `node` user at uid 1000; the uid contract
# (ARCHITECTURE.md, "Uid contract") reserves 1000 for `agent`, so remove it
# first. `userdel -r` reports the missing home directory it is asked to delete,
# which is not a failure here.
RUN userdel -r node 2>/dev/null || userdel node \
    && groupadd -g 1000 agent \
    && useradd -u 1000 -g 1000 -d /session/home -s /bin/bash -M agent \
    && mkdir -p /session/work /session/home /session/log \
    && chown 1000:1000 /session/work /session/home /session/log

# Pinned CLI. The npm global bin on this base image is /usr/local/bin, which is
# already on PATH for every user, so nothing needs a writable /usr/local at
# runtime.
RUN npm install -g "@anthropic-ai/claude-code@${CLAUDE_CODE_VERSION}" \
    && npm cache clean --force \
    && claude --version | grep -F "${CLAUDE_CODE_VERSION}"

COPY --chown=root:root --chmod=0755 mars-entrypoint /usr/local/bin/mars-entrypoint

# Assert the contract at build time rather than discovering it in a session.
RUN set -eux; \
    [ "$(id -u agent)" = "1000" ]; \
    [ "$(id -g agent)" = "1000" ]; \
    [ "$(getent passwd 1000 | cut -d: -f1)" = "agent" ]; \
    [ "$(getent passwd | cut -d: -f3 | grep -cx 1000)" = "1" ]; \
    [ "$(getent passwd agent | cut -d: -f6)" = "/session/home" ]; \
    [ "$(getent passwd agent | cut -d: -f7)" = "/bin/bash" ]; \
    [ "$(getent shadow agent | cut -d: -f2)" = "!" ]; \
    [ -x /usr/local/bin/mars-entrypoint ]; \
    su agent -s /bin/bash -c 'command -v claude git bash curl python3 jq less ps ssh'; \
    su agent -s /bin/bash -c 'claude --version' | grep -F "${CLAUDE_CODE_VERSION}"; \
    su agent -s /bin/bash -c 'git --version'; \
    su agent -l -c 'echo ok' | grep -qx ok

# No CLAUDE_CONFIG_DIR and no model credential: the launcher sets the config
# directory per project and injects exactly one of ANTHROPIC_API_KEY or
# CLAUDE_CODE_OAUTH_TOKEN (ARCHITECTURE.md, "Claude Code invocation").
ENV HOME=/session/home
WORKDIR /session/work
# DL3066 warns that a non-numeric user id may not resolve on the host.
# `agent` is created in this image at uid 1000 and the launcher sets
# User `1000:1000` on the container anyway (ARCHITECTURE.md, "Session
# container specification"), so the name is resolvable by construction.
# hadolint ignore=DL3066
USER agent

# The orchestrator passes the CLI and its flags as the container command; the
# entrypoint execs them with stdout redirected to /session/log/stream.jsonl.
# No CMD: a session without a command is a launcher bug and must fail loudly.
ENTRYPOINT ["/usr/local/bin/mars-entrypoint"]
