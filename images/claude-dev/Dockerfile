# Mars session dev image (ARCHITECTURE.md, "Session image"; ADR 0039).
#
# A layer on top of the contract base `images/claude`: a pinned Rust toolchain
# and the Node tooling a development session needs. It inherits the whole
# session image contract — the `agent` user at uid 1000, HOME=/session/home,
# the pinned Claude Code CLI, /usr/local/bin/mars-entrypoint as the entrypoint,
# no CMD — and adds nothing to it.
#
# Build (README.md, "Session image"); the base must already exist under the tag
# BASE_IMAGE names, and the version tag is derived from the one
# `ARG CLAUDE_CODE_VERSION=` line of images/claude/Dockerfile so the CLI is
# pinned in exactly one place:
#   podman build -t mars-session-claude-dev:$(sed -n 's/^ARG CLAUDE_CODE_VERSION=//p' images/claude/Dockerfile) -t mars-session-claude-dev:latest images/claude-dev
ARG BASE_IMAGE=mars-session-claude:latest
FROM ${BASE_IMAGE}

# Pinned toolchain versions. RUST_VERSION is the stable release a session gets
# by default; a repository's own rust-toolchain.toml is installed by rustup at
# runtime, which is why /opt/rustup is writable by `agent` below.
ARG RUST_VERSION=1.98.1
ARG RUSTUP_VERSION=1.29.1
ARG CARGO_BINSTALL_VERSION=1.23.0

# The CLI version is not pinned again here: `org.opencontainers.image.version`
# and `mars.claude_code_version` are inherited from the base image, which is
# the single source of truth for it (images/claude/Dockerfile).
LABEL org.opencontainers.image.title="mars-session-claude-dev" \
      org.opencontainers.image.description="Mars session dev image: Claude Code CLI plus a Rust and Node toolchain as the unprivileged agent user" \
      org.opencontainers.image.source="https://github.com/LHelge/mars"
LABEL mars.rust_version=$RUST_VERSION

# Installation needs root; the image ends on `agent` again below.
# DL3066 warns that a non-numeric user id may not resolve on the host; `root`
# is uid 0 in every image this can be built on.
# hadolint ignore=DL3066
USER root

# `pipefail` so the assertions further down fail when the command feeding a
# pipe does, as they do in the base image.
SHELL ["/bin/bash", "-o", "pipefail", "-c"]

# System packages, because a session has no root and cannot add them later
# (ARCHITECTURE.md, "Session container specification"): pkg-config and
# libssl-dev for the crates that link against system OpenSSL, clang and lld for
# crates with C or bindgen build scripts and for a faster link step, cmake for
# the build scripts that use it, unzip and xz-utils because release tarballs of
# user-installed tools come in those formats, ripgrep and fd-find because the
# agent's own search tools shell out to them.
RUN apt-get update \
    && apt-get install --no-install-recommends -y \
        clang \
        cmake \
        fd-find \
        libssl-dev \
        lld \
        pkg-config \
        ripgrep \
        unzip \
        xz-utils \
    && rm -rf /var/lib/apt/lists/*

# Rust, outside /session/home, which is a bind mount at runtime and would
# shadow anything baked under it. Both directories belong to `agent` so that
# rustup can install a repository's pinned toolchain and `cargo install` can
# write, neither of which has root available. rustup-init is fetched from the
# pinned-version archive URL and checked against the published digest rather
# than piped from the installer into a shell.
RUN set -eux; \
    mkdir -p /opt/rustup /opt/cargo; \
    chown 1000:1000 /opt/rustup /opt/cargo; \
    case "$(dpkg --print-architecture)" in \
        amd64) rustup_target=x86_64-unknown-linux-gnu; \
               rustup_sha=dda7234360b7f578ca8b0ddcb80145646fa61a67c1720a5abc7051b35c9fcb71 ;; \
        arm64) rustup_target=aarch64-unknown-linux-gnu; \
               rustup_sha=15f6e4ce9f583b929c996c91562bad6d4454f3281de858b02cdfdef615fac433 ;; \
        *) echo "unsupported architecture $(dpkg --print-architecture)" >&2; exit 1 ;; \
    esac; \
    curl -fsSL -o /tmp/rustup-init \
        "https://static.rust-lang.org/rustup/archive/${RUSTUP_VERSION}/${rustup_target}/rustup-init"; \
    echo "${rustup_sha}  /tmp/rustup-init" | sha256sum -c -; \
    chmod 0755 /tmp/rustup-init; \
    su agent -s /bin/bash -c \
        "RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo /tmp/rustup-init -y \
         --no-modify-path --profile minimal \
         --default-toolchain ${RUST_VERSION} -c rustfmt -c clippy"; \
    rm -f /tmp/rustup-init

# cargo-binstall, so an agent that wants cargo-nextest or sqlx-cli downloads a
# release binary instead of spending minutes compiling one. Same treatment as
# rustup-init: a pinned release URL and a digest, not an installer script.
RUN set -eux; \
    case "$(dpkg --print-architecture)" in \
        amd64) binstall_target=x86_64-unknown-linux-musl; \
               binstall_sha=64bf954c68bb558431deeabecaec7687edd5541c2189ee263bb8bc18bc4fdf55 ;; \
        arm64) binstall_target=aarch64-unknown-linux-musl; \
               binstall_sha=ba9b7bf426c7b7375825cd3fa367c3f8a632ca7c6c546fdcb738114b167f4103 ;; \
        *) echo "unsupported architecture $(dpkg --print-architecture)" >&2; exit 1 ;; \
    esac; \
    curl -fsSL -o /tmp/cargo-binstall.tgz \
        "https://github.com/cargo-bins/cargo-binstall/releases/download/v${CARGO_BINSTALL_VERSION}/cargo-binstall-${binstall_target}.tgz"; \
    echo "${binstall_sha}  /tmp/cargo-binstall.tgz" | sha256sum -c -; \
    tar -xzf /tmp/cargo-binstall.tgz -C /opt/cargo/bin cargo-binstall; \
    chown 1000:1000 /opt/cargo/bin/cargo-binstall; \
    rm -f /tmp/cargo-binstall.tgz

# Node 24 and npm come from the base image. corepack installs the pnpm and yarn
# shims, and the global npm prefix moves under the session home so that
# `npm install -g` works without root. The prefix is on a bind mount at
# runtime, so what is created here is only the empty directory; npm recreates
# it when the session first installs into it.
RUN set -eux; \
    corepack enable; \
    mkdir -p /session/home/.npm-global/bin; \
    chown -R 1000:1000 /session/home/.npm-global

# PATH twice over, on purpose. ENV covers the CLI process and every exec the
# orchestrator starts; /etc/profile.d covers the terminal, which runs
# `/bin/bash -l` and so passes through Debian's /etc/profile, which resets PATH.
ENV RUSTUP_HOME=/opt/rustup \
    CARGO_HOME=/opt/cargo \
    NPM_CONFIG_PREFIX=/session/home/.npm-global \
    PATH=/opt/cargo/bin:/session/home/.npm-global/bin:$PATH

# SC2016 sees the unexpanded `$PATH` below; that is the point — the profile
# script must expand it when the login shell sources it, not now.
# hadolint ignore=SC2016
RUN set -eux; \
    { \
        echo '# Mars session toolchains (ARCHITECTURE.md, "Session image").'; \
        echo 'export RUSTUP_HOME=/opt/rustup'; \
        echo 'export CARGO_HOME=/opt/cargo'; \
        echo 'export NPM_CONFIG_PREFIX=/session/home/.npm-global'; \
        echo 'PATH="/opt/cargo/bin:/session/home/.npm-global/bin:$PATH"'; \
        echo 'export PATH'; \
    } > /etc/profile.d/mars-toolchains.sh; \
    chmod 0644 /etc/profile.d/mars-toolchains.sh

# Assert the contract at build time rather than discovering it in a session.
# `su -p` keeps the environment, so these see the PATH that ENV sets, the way
# the CLI process does; the one `su agent -l` is the terminal's login shell.
RUN set -eux; \
    [ "$(id -u agent)" = "1000" ]; \
    [ "$(getent passwd agent | cut -d: -f6)" = "/session/home" ]; \
    [ "$(getent passwd agent | cut -d: -f7)" = "/bin/bash" ]; \
    [ -x /usr/local/bin/mars-entrypoint ]; \
    su agent -p -s /bin/bash -c 'command -v claude git cargo rustc rustfmt cargo-binstall node npm corepack rg fdfind'; \
    su agent -p -s /bin/bash -c 'rustc --version' | grep -F "${RUST_VERSION}"; \
    su agent -p -s /bin/bash -c 'cargo --version' | grep -F "${RUST_VERSION}"; \
    su agent -p -s /bin/bash -c 'cargo fmt --version'; \
    su agent -p -s /bin/bash -c 'cargo clippy --version'; \
    su agent -p -s /bin/bash -c 'cargo binstall -V'; \
    su agent -p -s /bin/bash -c 'node --version'; \
    su agent -p -s /bin/bash -c 'npm --version'; \
    su agent -p -s /bin/bash -c 'corepack --version'; \
    su agent -l -c 'command -v cargo' | grep -qx /opt/cargo/bin/cargo; \
    su agent -l -c 'command -v claude' >/dev/null; \
    su agent -p -s /bin/bash -c 'touch /opt/rustup/.mars-write-check /opt/cargo/.mars-write-check'; \
    rm -f /opt/rustup/.mars-write-check /opt/cargo/.mars-write-check; \
    su agent -p -s /bin/bash -c 'cargo new --quiet --bin --vcs none /tmp/mars-build-check'; \
    su agent -p -s /bin/bash -c 'cargo build --quiet --offline --manifest-path /tmp/mars-build-check/Cargo.toml'; \
    rm -rf /tmp/mars-build-check

# HOME, WORKDIR and the entrypoint are the base image's and are left untouched;
# no CMD, because a session without a command is a launcher bug.
# DL3066 warns that a non-numeric user id may not resolve on the host.
# `agent` is created in the base image at uid 1000 and the launcher sets
# User `1000:1000` on the container anyway (ARCHITECTURE.md, "Session
# container specification"), so the name is resolvable by construction.
# hadolint ignore=DL3066
USER agent
