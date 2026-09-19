# Mars orchestrator image (ARCHITECTURE.md, "Trust boundaries"; ADR 0012).
#
# The orchestrator container is the high-value target: it holds the engine
# socket, so the runtime image carries the binary, `git` and CA certificates and
# nothing else, runs as an unprivileged user and works under a read-only root
# filesystem with a tmpfs at /tmp.
#
# Build (README.md, "Development" → "Deployment images"), from the repository
# root:
#   podman build -t mars-orchestrator:dev orchestrator
#
# ---------------------------------------------------------------------------
# Builder
# ---------------------------------------------------------------------------
# `orchestrator/rust-toolchain.toml` pins the plain `stable` channel, so this
# tag names the stable release the crate is built and tested against today. The
# two must move together: bump this tag when the toolchain the project builds on
# moves, and if the `rust` image does not carry the tag yet the build fails
# here, at `FROM`, rather than somewhere subtler. rust-toolchain.toml itself is
# deliberately *not* copied into the builder: `stable` is not an installed
# toolchain name in the `rust` image, so rustup would download a second
# toolchain on every build.
#
# The Debian release in this tag must match the runtime stage's: the builder is
# where the git tests run in CI, and that is only a guard on the shipped git if
# the two stages are the same release. Both are trixie.
FROM rust:1.98.1-trixie AS builder

# No DATABASE_URL at build time: every `sqlx::query!` is resolved from the
# committed `.sqlx/` cache (CLAUDE.md, "Backend conventions").
ENV SQLX_OFFLINE=true

WORKDIR /build

# Dependency layer: build the crate's dependencies against the manifests alone,
# with placeholder sources, so a source-only change recompiles only this crate.
# `--locked` enforces Cargo.lock; nothing but the crate downloads touches the
# network. No `pkg-config`/`libssl-dev` is installed: `reqwest` is on rustls and
# no dependency links OpenSSL — if that ever changes, fix the crate feature
# rather than adding a native library here and, worse, to the runtime image.
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src \
    && echo 'fn main() {}' > src/main.rs \
    && : > src/lib.rs \
    && cargo build --release --locked --bin mars-orchestrator \
    && rm -rf src

# `migrations/` is needed *here* because `sqlx::migrate!()` embeds the SQL at
# compile time; it is deliberately absent from the runtime image.
COPY .sqlx ./.sqlx
COPY migrations ./migrations
COPY src ./src
RUN touch src/main.rs src/lib.rs \
    && cargo build --release --locked --bin mars-orchestrator

# Separate layer so it does not invalidate the compile above. Stripping halves
# the binary and is what keeps the runtime image under 200 MB; the release
# profile carries no debug info anyway, so only symbol names for backtrace
# symbolication are lost, and errors are reported through `tracing::error!` with
# their own context rather than through backtraces.
RUN strip --strip-all target/release/mars-orchestrator

# ---------------------------------------------------------------------------
# Runtime
# ---------------------------------------------------------------------------
FROM debian:trixie-slim

LABEL org.opencontainers.image.title="mars-orchestrator" \
      org.opencontainers.image.description="Mars orchestrator: API, MCP server and session owners" \
      org.opencontainers.image.source="https://github.com/LHelge/mars"

# `git` is the binary Mars shells out to (ADR 0011) and CA certificates are for
# https remotes and the email API. Nothing else: no curl, wget, procps or
# editors. apt pulls git's own dependencies transitively (on trixie that is
# `libcurl3t64-gnutls`, the time_t-64 rename of bookworm's `libcurl3-gnutls`;
# the package is never named here, so the rename costs nothing); the lists go
# in the same layer. `tzdata` is not installed:
# timestamps are RFC 3339 in UTC. Versions are unpinned for the same reason as
# the session images (hadolint DL3008): the image is rebuilt on every change and
# pinning would rot faster than it protects.
# hadolint ignore=DL3008
RUN apt-get update \
    && apt-get install --no-install-recommends -y \
        ca-certificates \
        git \
    && rm -rf /var/lib/apt/lists/*

# uid/gid 1000 is the uid contract (ARCHITECTURE.md, "Uid contract"): under
# Docker the orchestrator runs as uid 1000, under rootless Podman `keep-id` maps
# the service user onto some other uid, which is why HOME is set explicitly —
# git falls back to $HOME when the running uid is absent from /etc/passwd — and
# why nothing here is mode 0700 or otherwise readable only by its owner. No
# global gitconfig is baked in: the git wrapper passes identity and credentials
# through GIT_CONFIG_GLOBAL/GIT_AUTHOR_* (ARCHITECTURE.md, "Git model" →
# "Credentials"), writing a temporary 0600 file, which is why a writable /tmp is
# required under a read-only root filesystem. `safe.directory` is not set:
# ownership matches by the uid contract.
# `useradd -m` leaves skeleton files in /home/mars; they are harmless, and the
# explicit chmod keeps the directory traversable by a foreign uid.
RUN groupadd -g 1000 mars \
    && useradd -u 1000 -g 1000 -m -d /home/mars -s /usr/sbin/nologin mars \
    && chmod 0755 /home/mars \
    && mkdir -p /data \
    && chown 1000:1000 /data \
    && chmod 0755 /data

# /data is DATA_DIR in compose (README.md, "Configuration"). It is created so a
# bind mount over it never fails on a missing mount point, but there is no
# VOLUME instruction: an anonymous volume would silently paper over a
# DATA_DIR_HOST mistake.
ENV HOME=/home/mars \
    DATA_DIR=/data

WORKDIR /home/mars

# Documentation only; compose never publishes these (ARCHITECTURE.md,
# "Components").
EXPOSE 7000 7001

# Last layer, so a rebuild of the binary invalidates nothing above it.
COPY --from=builder --chmod=0755 /build/target/release/mars-orchestrator /usr/local/bin/mars-orchestrator

# Numeric, so the image default holds even where the name does not resolve.
USER 1000:1000

# No init process: the orchestrator installs its own SIGTERM handler
# (`shutdown_signal`), so PID 1 signal handling is covered without `tini`.
# No HEALTHCHECK either: compose declares it, so interval and command live in
# one place.
# The empty CMD keeps the subcommands (`mars-orchestrator healthcheck`,
# `mars-orchestrator rotate-secrets`) available as `podman exec`/`docker exec`
# arguments or a compose `command:` override.
ENTRYPOINT ["/usr/local/bin/mars-orchestrator"]
CMD []
