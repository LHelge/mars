# 0035. Compose engine variants as override files

Status: accepted.

## Context

Mars targets rootless Podman and supports Docker (ADR 0004): the only deployment difference is the engine socket, which `DOCKER_HOST` and the bind-mount source already carry through `.env`. One thing is not expressible as a variable. Under rootless Podman the orchestrator container needs `userns_mode: keep-id`, so its uid inside the container matches the service user that owns `DATA_DIR_HOST`; under Docker there is no user-namespace mapping and the service must instead run as `user: "1000:1000"` (`ARCHITECTURE.md`, "Uid contract"). Each key is invalid or harmful on the other engine, and compose has no conditional.

Options considered:

- **One compose file with the other engine's line commented out.** Nothing to select, but every operator edits the file Mars ships, so an upgrade is a merge and a wrong deployment is one uncommented line away.
- **Two complete compose files.** Each is self-contained, and every service definition exists twice; a change to the orchestrator service has to be made in both, and a divergence is invisible until one engine breaks.
- **An override file per engine.** Compose's own merge mechanism, supported by `docker compose` and `podman-compose` alike — though only through `-f` on both; `COMPOSE_FILE` in `.env` reaches just one of them (see "Consequences").

## Decision

`compose.yml` holds all three services and both networks. `compose.podman.yml` contains only `services.orchestrator.userns_mode: keep-id`; `compose.docker.yml` carries `services.orchestrator.user: "1000:1000"` and the `group_add` that gives that uid the host `docker` group, which is what the root-owned `0660` socket needs (`ARCHITECTURE.md`, "Uid contract"). The engine is selected by naming both files, as `-f compose.yml -f compose.<engine>.yml` or through `COMPOSE_FILE`.

## Consequences

- One source of truth for the services; the engine difference is two short files, and a reviewer sees the whole difference at a glance.
- `COMPOSE_FILE` becomes a compose-only variable in `.env` and the `README.md` "Configuration" table, alongside `ENGINE_SOCKET_HOST`, `HTTP_PORT` and the Docker-only `DOCKER_GID`.
- `podman-compose` 1.6.0 does ignore `COMPOSE_FILE` read from `.env` — it honours the variable only from the process environment — so `-f compose.yml -f compose.podman.yml` is not a fallback but the form `README.md`, "Start", documents for both implementations. Selecting nothing is not a benign default: `compose.yml` alone gives the orchestrator no `userns_mode`, it runs as a sub-uid under rootless Podman, cannot open the bind-mounted engine socket and restart-loops. `COMPOSE_FILE` stays in `.env` and in the `README.md` "Configuration" table, because `docker compose` does read it there.
- Selecting the wrong override is not caught by compose or by the engine at all — only by the orchestrator, at startup. `userns_mode` is a valid Compose-specification key, so `docker compose config` renders `keep-id`, and Docker 29.8.0 then accepts it at `create`: it stores `HostConfig.UsernsMode: "keep-id"` verbatim, applies no mapping, and starts the container. What fails is the first thing the orchestrator does — with the Podman override on Docker there is no `group_add`, so uid 1000 cannot open the socket and it restart-loops on `the container engine is unreachable; refusing to start` (`README.md`, "Start"). The variable is in `.env` beside `ENGINE_SOCKET_HOST` and `DOCKER_GID`, which have to match the engine anyway, so they are set together. Defaulting `.env.example` to the Docker pair instead was considered and rejected: the Docker failure is not opaque — it is a named log line the README lists, and `DOCKER_GID` has no default, so a Docker operator who has edited `.env` at all has already met the Docker paragraph. The default follows ADR 0004 and stays Podman.
- A third engine is a third override file, not a third copy of the services.
