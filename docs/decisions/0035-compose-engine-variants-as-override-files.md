# 0035. Compose engine variants as override files

Status: accepted.

## Context

Mars targets rootless Podman and supports Docker (ADR 0004): the only deployment difference is the engine socket, which `DOCKER_HOST` and the bind-mount source already carry through `.env`. One thing is not expressible as a variable. Under rootless Podman the orchestrator container needs `userns_mode: keep-id`, so its uid inside the container matches the service user that owns `DATA_DIR_HOST`; under Docker there is no user-namespace mapping and the service must instead run as `user: "1000:1000"` (`ARCHITECTURE.md`, "Uid contract"). Each key is invalid or harmful on the other engine, and compose has no conditional.

Options considered:

- **One compose file with the other engine's line commented out.** Nothing to select, but every operator edits the file Mars ships, so an upgrade is a merge and a wrong deployment is one uncommented line away.
- **Two complete compose files.** Each is self-contained, and every service definition exists twice; a change to the orchestrator service has to be made in both, and a divergence is invisible until one engine breaks.
- **An override file per engine, selected by `COMPOSE_FILE`.** Compose's own merge mechanism, supported by `docker compose` and `podman-compose` alike.

## Decision

`compose.yml` holds all three services and both networks. `compose.podman.yml` contains only `services.orchestrator.userns_mode: keep-id`, `compose.docker.yml` only `services.orchestrator.user: "1000:1000"`. The engine is selected by `COMPOSE_FILE` in `.env`.

## Consequences

- One source of truth for the services; the engine difference is two files of one line each, and a reviewer sees the whole difference at a glance.
- `COMPOSE_FILE` becomes a compose-only variable in `.env` and the `README.md` "Configuration" table, alongside `ENGINE_SOCKET_HOST` and `HTTP_PORT`.
- If a compose implementation ignores `COMPOSE_FILE` read from `.env`, the operator passes `-f compose.yml -f compose.podman.yml` instead; the README documents that fallback.
- Selecting the wrong override is caught late, not at `compose config`: `userns_mode` is a valid Compose-specification key, so `docker compose config` renders `keep-id` happily and only the Docker daemon rejects it, when the container is created. The variable is in `.env` beside `ENGINE_SOCKET_HOST`, which has to match the engine anyway, so the two are set together.
- A third engine is a third override file, not a third copy of the services.
