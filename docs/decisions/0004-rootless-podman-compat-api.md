# 0004. Rootless Podman through the Docker-compatible API

Status: accepted

## Context

The orchestrator creates one container per session and must run as an unprivileged user. Candidates: Docker (rootful daemon, or rootless mode), rootless Podman via its libpod REST API, rootless Podman via its Docker-compatible API.

A rootful Docker socket is root-equivalent; handing it to the orchestrator defeats "unprivileged". Podman's native libpod API is richer but has no mature Rust client and would lock out Docker. The Docker-compatible API is served by `podman system service` and is what `bollard` speaks.

## Decision

Rootless Podman is the target engine, reached through its Docker-compatible socket (`podman system service`, a user systemd socket unit, `loginctl enable-linger` on the service user). Docker must keep working; the only deployment difference is `DOCKER_HOST`. The engine adapter (`orchestrator/src/engine/`) is written against the compatible subset only: create, start, attach, exec, resize, wait, inspect, logs, list-by-label, kill, remove, networks, bind mounts.

## Consequences

- Attach, exec and resize work on both engines. Exotic `HostConfig` fields (some cgroup, device and security options) may silently no-op on Podman; the adapter keeps `HostConfig` minimal and any new field must be verified on both engines before use.
- Under rootless Podman, files a container writes to a bind mount are owned by a sub-uid unless the container runs with `--userns=keep-id`. The adapter sets `UsernsMode: "keep-id"` when the engine reports itself as Podman so the orchestrator and session containers agree on file ownership under `/data`. On Docker the field is ignored. `keep-id` is a hard requirement: the orchestrator verifies it with a probe container at startup and refuses to run if it is not honoured. The alternatives (running the CLI as container root and relying on Podman's root-to-user mapping, or fixing ownership after the fact with a privileged exec) were rejected because they either reopen the question of running the CLI as root or leave a window where the orchestrator cannot read what a session wrote.
- A sandboxed runtime (gVisor `runsc`, Kata) is selectable per profile through `HostConfig.Runtime`; it is an operator concern to install it.
- `testcontainers`-based tests run against whichever engine `DOCKER_HOST` points at, so CI can exercise both.
