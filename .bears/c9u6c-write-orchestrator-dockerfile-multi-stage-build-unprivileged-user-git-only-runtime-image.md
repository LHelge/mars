---
id: c9u6c
title: "Write orchestrator/Dockerfile: multi-stage build, unprivileged user, git-only runtime image"
status: open
priority: P1
created: "2026-09-16T20:41:16.123072042Z"
updated: "2026-09-16T20:51:52.557185799Z"
tags:
  - infra
  - orchestrator
depends_on:
  - s52qg
  - "2f5u2"
parent: "5czwa"
---

## Summary
Produce the orchestrator container image: a multi-stage `orchestrator/Dockerfile` that compiles the crate offline (`SQLX_OFFLINE=true`) in a Rust builder stage and copies the single binary into a minimal runtime image containing only `git`, CA certificates and the binary, running as an unprivileged user and compatible with a read-only root filesystem. This is the image the compose file builds for the `orchestrator` service on both rootless Podman and Docker.

## Documents
- `ARCHITECTURE.md` "Trust boundaries" ("The orchestrator container is the high-value target. It runs as an unprivileged user, its root filesystem is read-only where the engine allows, it contains `git` and nothing else beyond the binary, and the engine socket is the only privileged thing it holds."), "Components" table (orchestrator: "Runs as an unprivileged user. Holds the engine socket. Attached to both networks."), "Session container specification" → "Uid contract" (Docker: orchestrator runs as uid 1000; Podman: `keep-id`), "Git model" → "Credentials" (git reads `GIT_CONFIG_GLOBAL`; the wrapper writes a temporary `0600` config file, so a writable temp directory is required), "Storage" (`/data` is `DATA_DIR` in compose).
- `README.md` "Deployment shape" (orchestrator: Rust, unprivileged user), "Podman setup" (`userns_mode: keep-id`), "Configuration" (`DATA_DIR` = `/data` in compose), "Development" layout.
- ADR 0012 (orchestrator kept minimal: unprivileged user, no shell tools beyond `git`, read-only root filesystem where the engine allows it), ADR 0011 (git is the binary, never a crate).
- `CLAUDE.md` "Backend conventions" (`SQLX_OFFLINE=true`, `.sqlx/` committed; `rust-toolchain.toml` pins stable).

## Acceptance criteria
- [ ] `orchestrator/Dockerfile` has a builder stage `FROM rust:<version>-bookworm` whose version matches `orchestrator/rust-toolchain.toml` (read it; if the toolchain file pins plain `stable`, use the current stable tag and add a comment that the two must move together), sets `ENV SQLX_OFFLINE=true`, builds with `cargo build --release --locked --bin mars-orchestrator`, and caches dependency compilation in a separate layer (either `cargo chef` installed in the builder or a manual "copy manifests, build a dummy `main.rs`, then copy sources" step) so a source-only change does not recompile every crate.
- [ ] The runtime stage is `FROM debian:bookworm-slim` with exactly `apt-get install --no-install-recommends git ca-certificates` (plus what apt pulls transitively, notably `libcurl3-gnutls` for `git` over HTTPS), apt lists removed in the same layer, no `curl`, `wget`, `procps` or editors. `/usr/bin/git --version` works in the final image.
- [ ] A user and group `mars` with uid/gid 1000 are created (`groupadd -g 1000 mars && useradd -u 1000 -g 1000 -m -d /home/mars -s /usr/sbin/nologin mars`), `USER 1000:1000` is the image default, `ENV HOME=/home/mars` and `/home/mars` exists with mode 0755 owned by 1000. `WORKDIR /home/mars`. `/data` is created (`mkdir -p /data && chown 1000:1000 /data`) so a bind mount over it never fails on a missing mount point; no `VOLUME` instruction (anonymous volumes would hide `DATA_DIR_HOST` mistakes).
- [ ] `ENV DATA_DIR=/data`, `EXPOSE 7000 7001` (documentation only; ports are never published by compose), `ENTRYPOINT ["/usr/local/bin/mars-orchestrator"]`, `CMD []` so `mars-orchestrator healthcheck` and `mars-orchestrator rotate-secrets` work as `podman exec`/`docker exec` commands or as compose `command:` overrides.
- [ ] The binary and `git` run correctly under `--read-only` with a `tmpfs` at `/tmp`: `podman run --rm --read-only --tmpfs /tmp:rw,mode=1777 mars-orchestrator:dev healthcheck` fails only with `healthcheck: connection refused` (no EROFS or permission errors), and `podman run --rm --read-only --tmpfs /tmp --user 4242:4242 ...` also starts (a `keep-id` uid other than 1000 must be able to execute the binary and `git`; nothing in the image may be `0700` or rely on `/etc/passwd` containing the running uid).
- [ ] `orchestrator/.dockerignore` excludes `target/`, `.env`, `data/`, `tests/fixtures/**` is **kept** (fixtures are not needed at runtime but harmless; exclude only what bloats or leaks: `target/`, `.env*`, `*.log`).
- [ ] The image builds with both `podman build -t mars-orchestrator:dev orchestrator` and `docker build -t mars-orchestrator:dev orchestrator` from a clean checkout with no `DATABASE_URL` set (offline sqlx data), and the resulting image is under 200 MB.
- [ ] `README.md` "Development" gains a "Deployment images" line under "Session image" or a new subsection showing the two build commands (`podman build -t mars-orchestrator:dev orchestrator` and the nginx one from its task, or a forward reference to `compose build`).

## Implementation notes
- Files: `orchestrator/Dockerfile`, `orchestrator/.dockerignore`, `README.md`.
- Builder stage needs `pkg-config`/`libssl-dev` only if a crate links OpenSSL; the scaffolding task pinned `reqwest` to `rustls-tls`, so the builder should need nothing beyond the `rust` image. If the build fails on a native library, fix the crate feature rather than adding the library to the runtime image.
- `--locked` enforces `Cargo.lock`; the build must not touch the network beyond crate downloads in the builder.
- Copy `migrations/` is **not** needed at runtime: `sqlx::migrate!()` embeds migrations at compile time. Copy `.sqlx/` into the builder only.
- `git` under a uid that is not in `/etc/passwd` (Podman `keep-id` with a service user uid other than 1000) falls back to `$HOME`; that is why `HOME` is set explicitly. The git wrapper (Git epic) passes identity through `GIT_CONFIG_GLOBAL`/`GIT_AUTHOR_*`, so no global gitconfig is baked into the image. Do not add `safe.directory=*`; ownership matches by the uid contract.
- Keep the layer order: base packages → user → binary copy, so a binary change invalidates only the last layer.
- Do not add a `HEALTHCHECK` instruction; the compose task declares the health check so the interval and command live in one place.

## Edge cases
- Rust image tag drift: if `rust-toolchain.toml` pins a version the `rust` image does not have yet, the build fails at `FROM`; the comment in the Dockerfile tells the maintainer to bump both.
- `useradd -m` under BuildKit creates `/home/mars` with skeleton files; they are harmless but must be readable by others (`chmod 755 /home/mars`).
- `debian:bookworm-slim` has no `tini`; the orchestrator handles SIGTERM itself (scaffolding epic's `shutdown_signal`), so PID 1 signal handling is already covered; add a comment saying why no init is used.
- Timezone data: `chrono` formats RFC 3339 in UTC and needs no `tzdata`; do not install it.

## Testing
- Build on both engines from a clean checkout (`git clean -xfd` in a scratch clone), run the read-only and foreign-uid smoke commands above, `podman run --rm mars-orchestrator:dev git --version`, and `podman run --rm --entrypoint /bin/sh mars-orchestrator:dev -c 'command -v curl wget; id'` shows no curl/wget and `uid=1000(mars)`.
- `podman image inspect mars-orchestrator:dev --format '{{.Size}}'` under 200 MB; record the size in the PR.
- The backend test chain is unaffected but must still pass: `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- `README.md` "Development" (build command), same commit. `ARCHITECTURE.md` needs no change: the image implements "Trust boundaries" as written.

## Assumes from other epics
- "Repository scaffolding, tooling and CI": `orchestrator/` crate with `rust-toolchain.toml`, binary name `mars-orchestrator`, `reqwest` on rustls.
- "Database schema, models, repositories and test harness": `.sqlx/` committed so `SQLX_OFFLINE=true` builds.
- The `healthcheck` subcommand task of this epic (used only in the smoke commands above; the image builds without it).