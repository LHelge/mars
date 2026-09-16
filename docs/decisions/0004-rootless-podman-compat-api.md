# 0004. Rootless Podman through the Docker-compatible API

Status: accepted

## Context

The orchestrator creates one container per session and must run unprivileged. Candidates: rootful or rootless Docker, rootless Podman via its native libpod API, rootless Podman via its Docker-compatible API.

A rootful Docker socket is root-equivalent and defeats "unprivileged". The libpod API is richer but has no mature Rust client and would lock out Docker. The Docker-compatible API is served by `podman system service` and is what `bollard` speaks.

## Decision

Rootless Podman is the target engine, reached through its Docker-compatible socket. Docker must keep working; the only deployment difference is `DOCKER_HOST`. The engine adapter uses only the compatible subset: create, start, attach, exec, resize, wait, inspect, logs, list-by-label, kill, remove, networks, bind mounts.

## Consequences

- Exotic `HostConfig` fields may silently no-op on Podman. The adapter keeps `HostConfig` minimal; any new field is verified on both engines before use.
- Under rootless Podman, files written to a bind mount are owned by a sub-uid unless the container runs with `--userns=keep-id`. The adapter sets `keep-id` when the engine reports Podman so the orchestrator and sessions agree on ownership under `/data`; Docker ignores it. It is a hard requirement verified with a probe container at startup. Running the CLI as container root under Podman's root-to-user mapping, or fixing ownership afterwards with a privileged exec, were rejected: they reopen running the CLI as root or leave a window where the orchestrator cannot read what a session wrote.
- A sandboxed runtime (gVisor, Kata) is selectable per profile through `HostConfig.Runtime`; installing it is an operator concern.
- `testcontainers` tests run against whatever engine `DOCKER_HOST` names, so CI exercises both.
