# mars-session-stub — the credential-free session image for end-to-end tests
# (ARCHITECTURE.md, "Session image"; ADR 0010). It honours the same contract as
# images/claude: uid 1000 `agent`, HOME=/session/home, the /session/* mount
# points, /usr/local/bin/mars-entrypoint as ENTRYPOINT and git plus bash for the
# terminal escape hatch (SPEC.md, "WebSocket: session stream"). Its
# /usr/local/bin/claude is a Python replay of a recorded stream-json transcript,
# so no model credentials are needed.
FROM python:3.13-slim

LABEL mars.stub=true
LABEL org.opencontainers.image.title="mars-session-stub"

# bash for the terminal exec and the agent user's login shell, git so an
# end-to-end test can commit on the session branch through that terminal,
# ca-certificates because the mounted repository may carry https remotes.
# Versions are deliberately unpinned (hadolint DL3008): this image is rebuilt on
# every images/** change and pinning would rot faster than it protects.
RUN apt-get update \
    && apt-get install --no-install-recommends -y \
        bash \
        ca-certificates \
        git \
    && rm -rf /var/lib/apt/lists/*

# The user and directory stanza is byte-identical to images/claude so the two
# images cannot drift on the uid contract (ARCHITECTURE.md, "Uid contract").
# python:*-slim ships no uid-1000 user, but the guard stays for the same reason.
# DL4006: the only pipe sits inside a command substitution the surrounding `if`
# already guards, so `pipefail` would change nothing but the stanza's text.
# hadolint ignore=DL4006
RUN if getent passwd 1000 >/dev/null; then userdel -r "$(getent passwd 1000 | cut -d: -f1)" || true; fi \
    && groupadd -g 1000 agent \
    && useradd -u 1000 -g 1000 -d /session/home -s /bin/bash -M agent \
    && mkdir -p /session/work /session/home /session/log \
    && chown -R 1000:1000 /session

# /session/mcp.json is a read-only bind mount at runtime and is never created
# here; CLAUDE_CONFIG_DIR likewise points at a mount, not at image content.
COPY --chmod=0755 mars-entrypoint /usr/local/bin/mars-entrypoint
COPY --chmod=0755 claude /usr/local/bin/claude
COPY fixtures/*.jsonl /opt/mars-stub/fixtures/
# The modes are set here rather than with COPY --chmod because BuildKit
# applies --chmod to the directories it creates as well, which would leave
# /opt/mars-stub untraversable for the agent user under Docker.
RUN chmod 0755 /opt/mars-stub /opt/mars-stub/fixtures \
    && chmod 0644 /opt/mars-stub/fixtures/*.jsonl

ENV HOME=/session/home \
    LANG=C.UTF-8 \
    PYTHONUNBUFFERED=1

WORKDIR /session/work
# DL3066: `agent` is uid 1000, created by the stanza above, and the session
# container specification names it by name (ARCHITECTURE.md, "Uid contract").
# hadolint ignore=DL3066
USER agent

# No CMD: the orchestrator always passes the backend's launch command, which the
# entrypoint execs as PID 1 with stdout appended to /session/log/stream.jsonl.
ENTRYPOINT ["/usr/local/bin/mars-entrypoint"]
