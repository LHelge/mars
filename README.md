# Mars

Mars is a self-hosted web application that runs coding-agent sessions in isolated containers and shows them in a browser. Each session is one agent (Claude Code in v1; the backend is pluggable) working on its own clone of a git repository. Sessions are autonomous: they keep running when nobody is connected, and whoever connects later sees the full history. Agents coordinate through a shared task tracker that the orchestrator exposes to them over MCP, and the same tracker is visible and editable in the web UI.

The repository holds the skeleton — the orchestrator crate, the frontend application, `.env.example` and the CI workflows — and the documents below describe the system being built on it:

| Document | What it covers |
| --- | --- |
| [ARCHITECTURE.md](ARCHITECTURE.md) | Components, trust boundaries, session lifecycle, durability and recovery, git model, secrets, MCP, engine adapter. |
| [SPEC.md](SPEC.md) | v1 functional spec: features, REST/WebSocket/SSE endpoints, `AgentEvent` and `TaskEvent` schemas, MCP tool contracts, frontend structure, non-goals. |
| [docs/data-model.md](docs/data-model.md) | Every table, column, constraint, index and enum. |
| [docs/decisions/](docs/decisions/README.md) | Architecture decision records. |
| [docs/open-questions.md](docs/open-questions.md) | What is undecided, with options and recommendations. |
| [CLAUDE.md](CLAUDE.md) | Working conventions for agents (and humans) contributing to this repository. |

## Why

Running an agent on a laptop ties it to a terminal, a person, and a machine. Running many agents that way does not scale past one person's attention. Mars moves the agent into a container that outlives any browser tab, gives it a branch of its own, and gives a team one place to see what every agent is doing, steer it, and hand work between agents and people through a tracker that both sides use.

The design rests on a few commitments:

- **The container is the intended permission boundary.** Users are authenticated and trusted; agents run with full auto-permissions inside a container that has its own clone and receives no git credentials or engine socket. v1 accepts a known gap in orchestrator-side git operations, described under "Operating notes" below.
- **Orchestrator actions are audited.** Pushes, merges and task changes through the supported API and MCP tools are attributed to a session or a user. This does not cover exploitation of the known git-execution vulnerability.
- **Sessions run independently of the browser.** The agent's output is written to disk in the container and folded into an append-only event log in Postgres; the UI is a subscriber, never the owner of state. Delivery of incoming messages across orchestrator restarts has an accepted v1 limitation (ADR 0020).
- **One event schema.** The frontend never sees a CLI's native output; each backend is translated into the same `AgentEvent` stream.
- **Task state routes work.** The tracker is modelled on Beads, backed by Postgres instead of a synced store. A task's state is the queue it waits in, each agent profile says which states it serves, and agents hand work to each other by moving tasks between states. A planner turns `backlog` into `ready`, an implementer turns `ready` into `review`, a reviewer sends it on to `merge` or back. States are configured per project; the roles are profiles and prompts, not code.

## Deployment shape

Four kinds of container run under compose on one host:

```mermaid
flowchart LR
    B[Browser] -->|https| N[nginx]
    N -->|/api, /ws| O[orchestrator]
    O --> P[(postgres)]
    O -->|engine API| E[(podman / docker socket)]
    E -.creates.-> S1[session container]
    E -.creates.-> S2[session container]
    S1 -->|MCP| O
    S2 -->|MCP| O
```

- **orchestrator**: Rust (`axum`, `bollard`, `sqlx`, `rmcp`). Serves the API, owns every session, holds the engine socket, runs `git`. Unprivileged user. The image ships only the binary and `git`, so the compose health check is `mars-orchestrator healthcheck`, a subcommand of that binary: it asks `GET /api/health` on the local `API_PORT` and exits 0 on 200, 1 on anything else.
- **postgres**: the only system of record.
- **nginx**: serves the built React frontend and proxies `/api` and `/ws`. The MCP endpoint is not proxied.
- **session containers**: one per session, created by the orchestrator, on an internal network for MCP and a separate egress network for the internet, never given the engine socket.

Rootless Podman is the target engine, reached through its Docker-compatible socket. Docker works with a different `DOCKER_HOST`.

## Running it

These steps have been walked through end to end on both engines — rootless Podman 6.1.2 with `podman-compose` 1.6.0, and rootful Docker 29.8.0 with Docker Compose v5.5.1 — on Arch Linux: the stack comes up, the login page is reached through nginx, the forced first-login password change completes, and a session on the stub image runs, replays its transcript, takes a message, stops to `parked` and resumes after `compose down` and `up`.

### Prerequisites

- A Linux host with rootless Podman 4.9+ (or Docker 24+); the engine tests run on Podman 4.9.3 and 6.1.2 and on Docker 28.0.4, and the walkthrough above on Podman 6.1.2 and Docker 29.8.0.
- `podman-compose` or `docker compose`. They differ in one place that matters here — whether `COMPOSE_FILE` is read from `.env`; `docker compose` does, `podman-compose` 1.6.0 does not — so "Start" gives the command that works on both.
- A directory for persistent data, for example `/srv/mars/data`, owned by the service user (uid 1000 under Docker). It is `DATA_DIR_HOST` and must exist before the first start: compose bind-mounts it, and the orchestrator refuses to start if it is missing or not writable by the uid it runs as.
- On Docker, the service user in the `docker` group, and that group's gid in `DOCKER_GID` — see the Docker paragraph at the end of "Podman setup".
- `git` is **not** needed on the host: the orchestrator image ships it, and it is the only thing that runs `git`. The one exception is the host-run fallback under "Podman setup", where the orchestrator is a host process and uses the host's `git`.
- For private repositories: a fine-grained GitHub personal access token scoped to the repository.
- Model credentials: an Anthropic API key, or a subscription token from `claude setup-token` (requires a Pro or Max subscription). They are entered in the UI after the first login, not in `.env` — see "Start".

### Podman setup (once, as the service user)

```bash
# Let the user's services run without an active login session.
# No sudo when it is your own account: systemd's polkit rule lets an active
# session enable linger for itself. Use `sudo loginctl enable-linger <user>`
# to enable it for a service account you are not logged in as.
loginctl enable-linger "$USER"

# Start the Docker-compatible API socket with socket activation
systemctl --user enable --now podman.socket

# The socket the orchestrator will use: this is what ENGINE_SOCKET_HOST (the
# path) and DOCKER_HOST (the unix:// URL) get. Do not assume /run/user/1000;
# the socket lives under the service user's own XDG_RUNTIME_DIR.
echo "unix://$XDG_RUNTIME_DIR/podman/podman.sock"
```

`systemctl --user` and `$XDG_RUNTIME_DIR` need a real session for that user. `sudo -u <user> …` does not give you one, so run the block from a login shell of the service user (`machinectl shell <user>@` or `ssh <user>@localhost`).

The compose file runs the orchestrator with `userns_mode: keep-id` and mounts that socket, so the orchestrator's uid inside the container matches the service user on the host. Session containers run with `keep-id:uid=1000,gid=1000`, which maps the service user to the image's `agent` user (uid 1000) whatever the service user's uid is; the orchestrator verifies this at startup with a probe container and refuses to start if Podman does not honour it. That form of `keep-id` needs Podman 4.3 or newer, and the engine tests have verified it through the compatibility API on Podman 4.9.3 and 6.1.2. Podman resolves `keep-id` through the non-thread-safe `libsubid`, so it cannot do it for two containers at once: with twenty creates in flight on Podman 6.1.2 about one container in fourteen comes out with a broken uid mapping and then fails to start (`doesn't map UID 0`, `write to uid_map: Operation not permitted`), and the API service occasionally crashes in that call and is restarted by its socket unit. No fixed version is known, so the orchestrator creates containers one at a time per host: its engine adapter holds a lock across each container creation and releases it again before starting the container, so two sessions launched at once simply take turns creating their containers. The `podman` CLI is unaffected, because each invocation is its own process. If the compatibility API cannot apply `keep-id` to the orchestrator container, run the orchestrator on the host instead — "Running the orchestrator on the host" below. Session containers still require `keep-id:uid=1000,gid=1000` support and must pass the startup probe either way; that fallback is about the orchestrator's own container and nothing else.

`podman-compose` 1.6.0 does apply `userns_mode: keep-id` to the orchestrator container. To check it on your own host, note that `podman inspect` does not echo the word back — it reports the namespace Podman ended up creating:

```bash
podman inspect <project>_orchestrator_1 --format '{{.HostConfig.UsernsMode}} {{json .HostConfig.IDMappings}}'
# keep-id applied:  private {"UidMap":["0:1:1000","1000:0:1","1001:1001:64536"],…}
# override missing: <empty> null
podman exec <project>_orchestrator_1 id   # uid must equal the service user's uid
```

The `1000:0:1` entry is the mapping that matters: container uid 1000 is the service user. Without it the container runs as a sub-uid, cannot open the bind-mounted engine socket, and the orchestrator exits with `the container engine is unreachable; refusing to start`.

#### Running the orchestrator on the host

The fallback for a host where the compose implementation does not apply the override: the orchestrator runs as a user systemd service and compose starts only postgres and nginx around it. `deploy/mars-orchestrator.service` is that unit — a **user** unit, so the process keeps the service user's uid and the `keep-id` question never arises for it. The host needs `git` in this mode, which the compose deployment does not.

```bash
# One binary, no runtime beyond git and CA certificates.
(cd orchestrator && cargo build --release)     # -> orchestrator/target/release/mars-orchestrator

mkdir -p ~/mars/bin ~/mars/data
install -m 0755 orchestrator/target/release/mars-orchestrator ~/mars/bin/
install -m 0600 .env ~/mars/.env
install -Dm 0644 deploy/mars-orchestrator.service ~/.config/systemd/user/mars-orchestrator.service
systemctl --user daemon-reload
systemctl --user enable --now mars-orchestrator

# postgres and nginx around it -- one of the two, whichever you have. This
# pair of files renders on podman-compose 1.6.0 and on docker compose alike.
podman-compose -f compose.yml -f compose.hostrun.yml up -d
docker compose  -f compose.yml -f compose.hostrun.yml up -d
```

`compose.hostrun.yml` replaces the engine override: it disables the `orchestrator` service (a `profiles` entry, plus `depends_on: !reset {}` on nginx so the dependency goes with it), points nginx at `host.containers.internal` and publishes postgres on `127.0.0.1:5432` for the host-run process. `compose.podman.yml`/`compose.docker.yml` are **not** named alongside it — the single line they carry applies to the orchestrator service, which is not started. `loginctl enable-linger` from the top of this section is what keeps the unit running without an active login session.

The `.env` is the same file, with five values this mode needs (`Configuration`):

| Variable | Value on the host |
| --- | --- |
| `DATABASE_URL` | `postgres://<POSTGRES_USER>:<POSTGRES_PASSWORD>@127.0.0.1:5432/<POSTGRES_DB>` — compose no longer sets it, so this one is read as written. |
| `DATA_DIR`, `DATA_DIR_HOST` | The same absolute host path, `~/mars/data` written out in full (`%h` is not expanded inside `.env`). The unit's `ReadWritePaths=` names that path; change it too if you use another. |
| `DOCKER_HOST` | `unix://$XDG_RUNTIME_DIR/podman/podman.sock`, resolved. The unit already sets it from `%U`, so it can be left out. |
| `MCP_URL` | `http://host.containers.internal:7001/mcp`: sessions reach the listener through the host gateway, not through compose DNS. |
| `SESSION_EXTRA_HOSTS` | `host.containers.internal:host-gateway`. The internal session network has no gateway, so the name has to come from the egress network's; Podman does not add it by itself there. |

`ENGINE_SOCKET_HOST` is unused in this mode — nothing mounts the socket any more — and `HTTP_PORT` still belongs to nginx.

**Firewall the MCP port.** The compose deployment publishes nothing but `HTTP_PORT`; a host-run orchestrator binds `API_PORT` and `MCP_PORT` on all of the host's interfaces, so on a machine with a public interface the MCP listener is exposed. It answers an unauthenticated request with a bearer challenge and nothing more, but that is one layer, not two: restrict both ports to the Podman bridge with the host firewall. (Binding the listeners to a single address instead is not a v1 option; the orchestrator takes no bind address.) `HOSTRUN=1 scripts/verify-deployment.sh` checks this deployment: it expects the two ports open on the loopback address, probes the orchestrator from `mars-frontend` through the host gateway, and skips the `mars-sessions` check, which has no orchestrator on it in this mode.

**The trade-off.** The orchestrator is no longer in a container, so the read-only root filesystem, the tmpfs `/tmp` and the minimal image are gone; what is left is the systemd hardening in the unit — `ProtectSystem=strict` with `ReadWritePaths=` for the data directory only, `PrivateTmp=yes` (the git wrapper needs a writable `/tmp`) and `NoNewPrivileges=yes`. That is a partial replacement, not an equal one, which is why this is the fallback and the compose deployment is the supported shape. `TimeoutStopSec=40` in the unit must stay above `STOP_GRACE_SECS` (default 20) plus margin, or systemd kills the orchestrator in the middle of stopping its sessions.

#### Docker instead of Podman

Use the daemon's socket (`unix:///var/run/docker.sock`) and a user in the `docker` group, and set four variables in `.env` ("Configuration"):

```bash
DOCKER_HOST=unix:///var/run/docker.sock
ENGINE_SOCKET_HOST=/var/run/docker.sock
DOCKER_GID=$(getent group docker | cut -d: -f3)   # write the number out; .env does not expand
COMPOSE_FILE=compose.yml:compose.docker.yml       # docker compose does read this from .env
```

Docker applies no user-namespace mapping, so the orchestrator itself runs as uid 1000 (`user: "1000:1000"` in `compose.docker.yml`) and `DATA_DIR_HOST` must be owned by uid 1000. The session uid and data-directory requirements still apply.

**`DOCKER_GID` is not optional.** `/var/run/docker.sock` is `root:docker` mode `0660`, and the `docker` group membership that lets you reach it belongs to the *host* user, not to uid 1000 inside the orchestrator container. `compose.docker.yml` therefore also carries `group_add: ["${DOCKER_GID}"]`. The gid is host-specific — 967 on the Arch host this was walked through on, commonly 999 on Debian and Ubuntu — so nothing can default it. Without it compose refuses to render the file at all (`set DOCKER_GID: getent group docker | cut -d: -f3`); with the wrong number the orchestrator restart-loops on `the container engine is unreachable; refusing to start`.

**The Docker socket is root-equivalent.** Anything that can write to it can start a privileged container and own the host, and the orchestrator container holds it. Under rootless Podman the same socket carries only the service user's own authority; under Docker it carries root's. The read-only root filesystem, dropped capabilities and uid 1000 are still worth having, but "the orchestrator runs unprivileged" is a weaker claim here than on the target engine (ADR 0004), and `docker` group membership is root on the host by another name. This is why rootless Podman is the supported target and Docker the supported alternative.

Check the result:

```bash
docker inspect <project>-orchestrator-1 \
  --format 'User={{.Config.User}} Userns="{{.HostConfig.UsernsMode}}" GroupAdd={{.HostConfig.GroupAdd}}'
# User=1000:1000 Userns="" GroupAdd=[967]
```

`UsernsMode` is empty on Docker, for the orchestrator and for session containers alike: the adapter sets `keep-id` only when the engine reports Podman (`ARCHITECTURE.md`, "Engine adapter"). Docker's embedded DNS resolves the `orchestrator` alias on `mars-sessions` for containers the orchestrator creates outside compose, so `MCP_URL` keeps its default; `scripts/verify-deployment.sh` check 4 is what proves it.

### Configuration

Copy `.env.example` to `.env` and set:

| Variable | Meaning |
| --- | --- |
| `PUBLIC_URL` | The URL users open, used for cookies and email links. |
| `JWT_SECRET` | Secret for signing access tokens. |
| `DATABASE_URL` | Postgres connection string (compose sets it for the orchestrator). |
| `POSTGRES_USER`, `POSTGRES_PASSWORD`, `POSTGRES_DB` | Database bootstrap. Compose interpolates all three into the `DATABASE_URL` it gives the orchestrator, so `POSTGRES_PASSWORD` must use URL-safe characters (letters, digits, `-`, `_`, `.`, `~`); setting `DATABASE_URL` in `.env` does not help, because compose's `environment:` overrides it. |
| `DOCKER_HOST` | Engine socket, `unix://$XDG_RUNTIME_DIR/podman/podman.sock` for rootless Podman — the path "Podman setup" printed, which is `/run/user/<uid>/…` for the service user's own uid and not necessarily 1000. Compose overrides it inside the orchestrator container to the mounted socket path (`unix:///run/engine.sock`), so the value here is what `podman-compose`/`docker compose` itself and a host-run orchestrator use. |
| `ENGINE_SOCKET_HOST` | **Compose only.** Host path of the engine socket, bind-mounted into the orchestrator at `/run/engine.sock`; `$XDG_RUNTIME_DIR/podman/podman.sock` for rootless Podman (again the service user's own uid, not necessarily 1000), `/var/run/docker.sock` for Docker. A path, not a `unix://` URL, and compose does not expand `$XDG_RUNTIME_DIR` for you: write the resolved path. |
| `DATA_DIR_HOST` | Host path of the data directory; mounted at `/data` in the orchestrator and used as the source of session bind mounts. A bind-mount source must be absolute, so a relative path is resolved against the orchestrator's working directory at startup. |
| `DATA_DIR` | Path at which the orchestrator itself sees the data directory: `/data` in compose, the same as `DATA_DIR_HOST` when running on the host. |
| `MCP_URL` | URL written into each session's MCP config; default `http://orchestrator:7001/mcp`. On a development host: `http://host.containers.internal:7001/mcp`. Must have a scheme and a host: the MCP listener answers only requests whose `Host` is this URL's host (and port, when it names one) or a loopback name, and refuses any other with 403. |
| `SESSION_NETWORK_INTERNAL`, `SESSION_NETWORK_EGRESS` | Names of the two session networks (default `mars-sessions`, `mars-egress`); created at startup if missing. |
| `SESSION_EXTRA_HOSTS` | Optional comma-separated `host:ip` entries added to session containers, e.g. `host.containers.internal:host-gateway` for development. |
| `SECRETS_MASTER_KEYS` | One or more `<version>=<base64 32-byte key>` entries, comma separated. Or `SECRETS_MASTER_KEY_FILE`, a file with the same content, which should be readable only by its owner — a broader mode is accepted with a warning, because a mounted container secret's mode is not always the operator's to set. Back this up separately from the database; without it every stored secret is unrecoverable. The orchestrator refuses to start when a `key_version` present in `secrets` has no configured key or cannot be unwrapped with the one configured for it. |
| `GIT_BOT_NAME`, `GIT_BOT_EMAIL` | Identity for commits the orchestrator creates (merges). |
| `API_PORT` | Port of the API listener nginx proxies to (default 7000). |
| `MCP_PORT` | Port of the MCP listener on the sessions network (default 7001). |
| `HTTP_PORT` | **Compose only.** Host port nginx publishes (default 8080). Under rootless Podman a port below 1024 fails to bind unless `net.ipv4.ip_unprivileged_port_start` is lowered; keep 8080 and put any reverse proxy in front of it. A bare port binds every interface; the whole `ip:port` left-hand side of a compose port mapping is accepted here, so `HTTP_PORT=127.0.0.1:8080` publishes on the loopback address only — see "Operating notes". `scripts/verify-deployment.sh` wants the bare port, so pass it one (`HTTP_PORT=8080 scripts/verify-deployment.sh`) when `.env` carries the `ip:port` form. |
| `STOP_GRACE_SECS` | Seconds between SIGINT and SIGTERM when stopping a session (default 20). |
| `MIRROR_FETCH_INTERVAL_SECS` | How often project mirrors are fetched (default 600). |
| `DISPATCHER_INTERVAL_SECS` | How often the dispatcher sweeps for claimable work (default 60). The dispatcher launches an ephemeral session for the best claimable task of each `auto_launch` profile (`ARCHITECTURE.md`, "Dispatcher"); this timer is its fallback, behind the `task_events` wake-up, so the value bounds how long a missed wake-up goes unnoticed rather than how promptly a queue is picked up. Raise it on an instance with many projects and little automation; there is no need to lower it to make automation react faster. To stop automated launches in a project, pause automation on its page. |
| `SESSION_IMAGE_DEFAULT` | Image used by the profiles a new project is seeded with and by the startup probe (default `mars-session-claude-dev:latest`, the toolchain image, so a new project can build Rust and Node out of the box). Both pull it, so it has to exist on the engine before the first start; build the base and then the dev image as described under "Session image". The value is copied into a profile's `image` when the project is created and the row is then the user's (ADR 0038), so a project created before this default changed keeps the image it was seeded with until someone edits the profile. Upgrading: build the dev image, then either leave the variable unset or point it at the dev tag. |
| `AUTOMATION_MAX_SESSIONS` | Instance-wide ceiling on live (`creating` or `running`) sessions an unattended launch may happen under (default 4, which suits one developer-sized host: each live session is a container with a checkout and an agent process in it). It counts every live session on the instance, whoever launched it, and it holds automation back only — a launch a person makes is never refused by it, and neither are the per-profile and per-project caps in the database (`ARCHITECTURE.md`, "Task tracker" → "Unattended launches"). A value below 1 is refused at startup naming the variable: to stop automated launches entirely, pause the project on its page, which is reversible and needs no restart. |
| `RESEND_API_KEY`, `MAIL_FROM` | Email delivery through Resend, used for invites, password resets and task escalations. `MAIL_FROM` is required once `RESEND_API_KEY` is set. Without an API key, full usable links including their tokens are intentionally written to the orchestrator log at `info` instead of sent. This supports local development without email configuration; no extra flag is required (ADR 0026). |
| `RUST_LOG` | Log filter, `info` by default and whenever the given filter is unusable, such as the bare non-level word `verbose`. |
| `COMPOSE_FILE` | **Compose only.** Which compose files make up the deployment, and so which engine it runs on: `compose.yml:compose.podman.yml` or `compose.yml:compose.docker.yml` (ADR 0035). `docker compose` reads it from `.env` — verified on Compose v5.5.1 — and `podman-compose` 1.6.0 does **not**; see "Start". |
| `DOCKER_GID` | **Compose only, Docker only.** Numeric gid of the host `docker` group (`getent group docker \| cut -d: -f3`), added to the orchestrator container so uid 1000 can open the bind-mounted `/var/run/docker.sock`, which is `root:docker` mode `0660`. Required whenever `compose.docker.yml` is selected, and unused under Podman. There is no default, because the gid differs per distribution. |

Generate a master key with `openssl rand -base64 32`.

### Start

Build the session images first: compose does not build them, and the startup probe pulls `SESSION_IMAGE_DEFAULT` and refuses to start when it is not there. Run the two build commands of "Session image" — the base and then the dev image, in that order — before the first `up -d`.

The deployment is one `compose.yml` plus a short override file per engine (ADR 0035). Name both files on the command line:

```bash
podman-compose -f compose.yml -f compose.podman.yml up -d   # rootless Podman
docker compose  -f compose.yml -f compose.docker.yml up -d  # Docker
```

Pass the same `-f` pair to every later `ps`, `logs`, `build`, `exec` and `down`; `export COMPOSE_FILE=compose.yml:compose.podman.yml` in the shell instead if you would rather not repeat them.

`COMPOSE_FILE` in `.env` also selects the engine, but only for `docker compose`: **`podman-compose` 1.6.0 reads `COMPOSE_FILE` from the process environment and not from `.env`**, so with the variable set only in `.env` it silently starts `compose.yml` alone. The orchestrator then has no `userns_mode: keep-id`, runs as a sub-uid, cannot open the bind-mounted engine socket, and restart-loops on `the container engine is unreachable; refusing to start`. The `-f` form above avoids the difference, which is why it is the documented one. `.env.example` still carries `COMPOSE_FILE`, and it is what a `docker compose` deployment uses.

After `up -d`, `scripts/verify-deployment.sh` checks health, network isolation and log hygiene against the running stack: it prints one `ok`/`warn`/`FAIL` line per check and exits non-zero on any failure. It finds the compose command itself; set `COMPOSE_CMD` when yours needs the `-f` pair.

The first start builds both images, the orchestrator and nginx, which takes a while; a build failure leaves nothing running. After pulling changes, rebuild explicitly with `compose build` — and then recreate, because `up -d --build` rebuilds the image but leaves an already-running container on the old one (`up -d --force-recreate orchestrator`). nginx then listens on `HTTP_PORT` (default 8080) and nothing else is published: neither the API (`API_PORT`) nor the MCP listener (`MCP_PORT`) is reachable from the host. The host-run fallback ("Podman setup" → "Running the orchestrator on the host") is the exception, and says what to do about it.

`compose ps` reports a health state for postgres and the orchestrator. nginx has no healthcheck, so it shows none; `Up` is all you get for it, and `scripts/verify-deployment.sh` is what actually proves it serves.

TLS is terminated in front of nginx by the operator — a host reverse proxy or a load balancer — and `PUBLIC_URL` must be the `https://` URL users actually open, because the session cookie is marked `Secure` exactly when `PUBLIC_URL` is https.

nginx sends a Content-Security-Policy with every response (`nginx/default.conf.template`, rationale in `ARCHITECTURE.md`, "Content-Security-Policy"): scripts, styles, images, fonts and connections come from the deployment's own origin and nowhere else, so nothing an agent writes into a transcript, a task or a comment can make an operator's browser fetch a remote address. The WebSocket origin it allows is derived from the `Host` header the browser sent, so no configuration variable carries it and no reverse proxy setup needs to name it. Check it with `curl -sI http://localhost:8080/ | grep -i content-security-policy`. Two things change it. A reverse proxy in front that adds a CSP of its own sends a second header, and a browser then enforces both intersected — the strictest wins, and a directive missing from this one but present in the other still applies; prefer removing the outer one over loosening this. And an operator who embeds the console in another page, or serves anything else from this origin, is editing the policy: `frame-ancestors 'none'` forbids the frame, and anything loaded from a third-party origin needs that origin named in the matching directive. Keep `script-src` free of `'unsafe-inline'` and `'unsafe-eval'` in either case — the access token is mirrored to `localStorage` (`SPEC.md`, "Frontend"), which those two would expose.

If the orchestrator restarts in a loop, check `compose logs orchestrator`. Three lines account for nearly all of it, and every one of them appears once per restart:

- **`the container engine refused a startup step … Permission denied (os error 13)`** — `DATA_DIR_HOST` is not writable by the uid the orchestrator runs as, so it cannot even create the directory the startup probe needs. On Docker that is uid 1000 and the fix is `chown 1000:1000` on the data directory; on Podman it is the service user. (Writability is what is actually required: a directory owned by another uid but world-writable gets past this and the probe then passes, which is not a configuration to rely on.)
- **`startup probe failed; refusing to start`** — the probe container ran but the file it wrote is owned by another uid than the orchestrator's, or could not be written or appended to. Under rootless Podman this is `keep-id` not being honoured; it is the uid contract failing rather than the directory being wrong (`ARCHITECTURE.md`, "Uid contract").
- **`the container engine is unreachable; refusing to start`** — it cannot use the socket at `/run/engine.sock`. Either `ENGINE_SOCKET_HOST` does not point at a live socket; or, on Docker, `DOCKER_GID` is wrong or the Podman override was selected by mistake, so uid 1000 has no group that may open `root:docker 0660`; or, on Podman, the engine override was not applied and the container is not running as the service user (see "Podman setup").

Selecting the wrong override is caught here and nowhere earlier. `userns_mode` is a valid Compose-specification key, so `docker compose config` renders it, and Docker 29.8.0 accepts it at `create` as well — it stores `HostConfig.UsernsMode: "keep-id"` verbatim and applies no mapping at all. The container starts; only the orchestrator notices, with the third line above.

The orchestrator applies database migrations on startup; the first migration seeds an administrator:

| Username | Password |
| --- | --- |
| `admin` | `changeme` |

The fixed bootstrap credentials are intentional for v1: the operator controls initial setup and completes the first-login password change before making the instance available to other users. No separate initial-password setting or setup wizard is required (ADR 0024).

Changing or resetting a user's password invalidates their previous logins. Changing your own password keeps the current browser signed in with new credentials. Deleted users lose access and administrator-role changes apply on subsequent requests; open connections check for revoked logins at their heartbeat ticks (ADR 0025). Running agent sessions continue independently of user logins.

Open `PUBLIC_URL`, log in as `admin`, and you are required to set a new password before anything else works. Then add an agent credential on the Secrets page: pick the subscription token or the API key, paste it, and choose whether it applies to you, to one project or to everyone. Every session picks up the most specific one that applies to whoever launches it — yours before the project's before the shared one — so no profile needs editing, and the launch form says which credential it will use, or that there is none (ADR 0036). Then invite your team from the admin page (each invite is a 7-day link sent by email), create a project from a remote URL, and launch a session from one of the four profiles it starts with. A new project comes with a `planner` over its `backlog` column, an `implementer` over `ready` (the one a launch defaults to), a `reviewer` over `review` and a `merger` over `merge`, each with a prompt for its role that is yours to edit from the project's Profiles tab (`SPEC.md`, "Role profile templates"). There is no self-registration.

Work flows through the project's task board. Search across its columns by title or exact task number (`42` or `#42`). Use `Copy link` in a task or session header to reference it in comments or share its direct URL with teammates; opening it requires login. Create a task (it lands in `backlog`), open it in a planning session to break it down, and the resulting `ready` tasks are what an implementer session picks up with its `ready` and `claim` tools. In v1 every session is started by a person, optionally for one task, either as a conversation or as a one-shot run of an ephemeral profile; a task that agents keep failing on ends up in `needs_human` after `max_attempts` (project setting, default 3) with the agents' comments explaining why.

Code hand-offs keep the producing session, branch, exact commit and a comment together. Opening the next session on that task defaults to the handed-over commit, so reviewers see the implementation they were asked to review. Review approval belongs to that commit; submitting a revised commit starts a new review. The task's merge action merges the approved revision, even if the original session branch has since changed.

### Operating notes

- **Known v1 vulnerability:** git commands run by the orchestrator against an agent-controlled checkout can execute helpers configured by that agent, with the orchestrator's access to secrets, project data and the engine socket. This risk is explicitly accepted for v1; isolating those git operations is deferred. See [ADR 0019](docs/decisions/0019-defer-isolation-of-git-checkout-operations.md). Session containers are not a complete containment guarantee while this remains unresolved.
- `compose down` while sessions are running cannot remove `mars-sessions`, because session containers are still attached to it: the services are gone but the network removal reports an error, and the session containers themselves are left running. Stop or delete the sessions first, or remove them and the network by hand afterwards.
- `mars-egress` is not declared in `compose.yml` — the orchestrator creates it at startup — so `compose down` never removes it. Removing it by hand is safe once no session container is attached.
- **`HTTP_PORT` binds every interface** unless you say otherwise. Compose publishes `${HTTP_PORT}:80`, so a bare `8080` becomes `0.0.0.0:8080` and, on Docker, the daemon's own iptables rules sit in front of a host firewall such as ufw and will happily expose it. The left-hand side of a compose port mapping may carry an address, and `HTTP_PORT` is substituted whole, so `HTTP_PORT=127.0.0.1:8080` publishes on the loopback address only — verified on Docker Compose v5.5.1 (`ss -ltn` then shows `127.0.0.1:8080`). Use that form whenever the TLS terminator runs on the same host.
- **Docker: `all predefined address pools have been fully subnetted`.** The orchestrator creates `mars-egress` at startup, and on a Docker host whose default address pools are used up — a developer machine with dozens of leftover compose networks is enough — the daemon refuses, the orchestrator logs `the container engine refused a startup step` and restart-loops. Free a pool (`docker network prune` after checking what would go), widen `default-address-pools` in `/etc/docker/daemon.json`, or create the network once with an explicit subnet: `docker network create --subnet 10.212.7.0/24 mars-egress`. An existing network is left exactly as it is at startup.
- Session working copies, project mirrors and transcripts live under `DATA_DIR_HOST`. Back it up with the database.
- v1 does not automatically redact secrets from agent/tool output or user messages. Transcripts, event history and their backups may contain credentials printed by commands or pasted into messages; encryption of stored secrets does not cover these copies (ADR 0027).
- Git fetches refresh the project's upstream-tracking branches (`origin/main`, for example). Mars keeps its integration branches (`main`) separately, so a background fetch cannot discard a merge waiting to be pushed. Merge `origin/main` into `main` explicitly to incorporate upstream changes, then push when ready. A push rejected because upstream changed leaves local work intact. Session reference clones still share history through the read-only project repository.
- Rotating the secrets master key: add a new `<version>=<key>` entry with a higher version, restart, and let the hourly rotation job re-wrap existing rows; remove the old entry once `GET /api/secrets` shows no row on the old version. To re-wrap immediately instead of waiting for the job, run `podman-compose exec orchestrator mars-orchestrator rotate-secrets` (`cargo run -- rotate-secrets` on a development host). It prints one line, `rewrapped=<n> skipped=<n> remaining=<n>`, and exits **0** when nothing is left behind, **2** when `remaining` is above zero and the sweep has to be run again before the old key can be dropped, and **1** when the configuration, the startup key verification or the sweep itself failed. Running it while the orchestrator is serving is safe: it starts no listener and no background job, and each row is re-wrapped under a condition on its current key version, so a secret written at the same moment keeps the new value and is counted as skipped rather than overwritten.
- Restarting the orchestrator does not intentionally stop running containers: they are re-adopted and their transcripts resumed from the last committed offset. **Known v1 issue:** queued incoming messages can be lost, and messages already shown in history may not have reached the agent. Inputs are not automatically resent after restart; inspect the conversation before resubmitting, since the agent may already have acted on a message. This restart-related behavior is accepted for v1 ([ADR 0020](docs/decisions/0020-defer-durable-input-delivery.md)).
- Each session accumulates cost and token counts from the CLI's result messages; they are shown on the session and can be summed per project from the session-list API responses.
- Escalations to `needs_human` email the task's assignee, or every admin when there is none; each user can opt out under settings. Without `RESEND_API_KEY` these go to the log like invites.
- Removing a project removes its mirror, its CLI state directory, its shared directories and every session directory under it.
- Sessions of one project share the agent CLI's state directory (transcripts, auto memory, installed skills and plugins), so what one session learns is available to the next. Nothing is shared between projects.
- Shared directories (project page, "Shared directories") mount one directory read-write into every session of a project, so build output is kept once instead of once per session. Which directories are safe to share is per ecosystem: a content-addressed download cache almost always is, build output inside the checkout usually is not. Cargo is the exception because it locks its build directory, so concurrent builds from several sessions queue instead of corrupting each other; the same working-directory path in every session means artifacts are reused across sessions. Starting points:

  | Ecosystem | Share (name → container path) | Keep per session |
  | --- | --- | --- |
  | Rust | `target` → `/session/work/target`; `cargo-registry` → `/session/home/.cargo/registry` | |
  | Node | `npm-cache` → `/session/home/.npm`, or the pnpm store | `node_modules` (rewritten in place; branches disagree on lockfiles) |
  | Go | `go-mod` → `/session/home/go/pkg/mod`; `go-build` → `/session/home/.cache/go-build` | |
  | Python | `uv-cache` → `/session/home/.cache/uv` (or the pip cache) | virtualenvs |
  | JVM | `m2` → `/session/home/.m2`; `gradle` → `/session/home/.gradle` | `build/` |

  A shared directory grows across branches; empty it from the project page when disk gets tight. Both emptying and removing are refused while a session of the project is running.

## Development

The layout:

```
mars/
├── orchestrator/       Rust crate (axum API, MCP server, session owners)
├── frontend/           Vite + React + TypeScript
├── docs/               data model, decisions, open questions
├── .github/workflows/  CI: orchestrator, frontend, e2e, images
├── .env.example
├── images/             session container images (claude/, claude-dev/, stub/)
├── nginx/              nginx.conf, default.conf.template and Dockerfile for the frontend image
├── deploy/             mars-orchestrator.service, the user unit for the host-run fallback
├── scripts/            verify-deployment.sh, the smoke test for a started stack
├── compose.yml         the deployment: postgres, orchestrator, nginx
├── compose.podman.yml, compose.docker.yml   the engine overrides
└── compose.hostrun.yml override for the host-run orchestrator ("Podman setup")
```

Working conventions, code-quality commands and test expectations are in `CLAUDE.md`.

### Running locally

The orchestrator, the frontend, the session images, the deployment images, `.env.example` and the compose files are all in the repository; these steps are for developing on the host instead of running the compose stack. The socket commands assume Linux.

Unlike the packaged stack, a host run and the git tests use the host's `git`. The minimum supported version is **git 2.39**, Debian bookworm's (`ARCHITECTURE.md`, "Git model", Supported git); newer is fine, but a newer git accepts argv forms 2.39 does not, so Orchestrator CI reruns the git tests on 2.39.5 as well as on the git the orchestrator image ships.

**Postgres**:

```bash
podman run -d --name mars-pg -e POSTGRES_USER=mars -e POSTGRES_PASSWORD=mars -e POSTGRES_DB=mars -p 5432:5432 postgres:18
export DATABASE_URL=postgres://mars:mars@localhost:5432/mars
```

That database is also what backs the offline query cache: after changing any `sqlx::query!`/`query_as!` call, run `cargo sqlx prepare -- --all-targets --features integration-tests` in `orchestrator/` and commit `.sqlx/` (`CLAUDE.md`, "Backend conventions"). Orchestrator CI runs the same command with `--check` against its own Postgres service, so a stale `.sqlx/` fails the build.

**Podman socket** (rootless):

```bash
systemctl --user enable --now podman.socket
export DOCKER_HOST=unix://$XDG_RUNTIME_DIR/podman/podman.sock
```

With Docker instead: `export DOCKER_HOST=unix:///var/run/docker.sock`. The supported Docker uid contract also requires running the orchestrator as uid 1000 with a data directory owned by that uid; see `ARCHITECTURE.md`, "Uid contract".

The backend tests reach this socket too: the first process of a test run to want a database starts one `postgres:18` container, every process of that run gives its tests a database of their own on that one server (`CLAUDE.md`, "Testing expectations"), and the last process out removes the container.

**Test runner**: the backend suite runs under [`cargo nextest`](https://nexte.st), which the "Code quality" chain in `CLAUDE.md` calls and which is not part of a Rust toolchain (ADR 0037):

```bash
cargo install cargo-nextest --locked
```

**Orchestrator**:

```bash
cd orchestrator
cp ../.env.example ../.env   # then edit
cargo run                    # runs migrations, listens on API_PORT and MCP_PORT
```

`Config::from_env` looks for `.env` in the current directory and then in `../.env`, and nowhere further up, so running from `orchestrator/` picks up the repository-root file and a process started deeper in the tree picks up none. `dotenvy` never overrides a variable that is already set, so the `DATABASE_URL` exported above wins over the compose-oriented `postgres` host in `.env.example`.

`DATA_DIR_HOST` must point at a directory the current user owns; when running the orchestrator directly on the host it is the same path as `DATA_DIR` (default `./data`, made absolute at startup). Set `MCP_URL=http://host.containers.internal:7001/mcp` (Docker: `host.docker.internal`, plus `SESSION_EXTRA_HOSTS=host.docker.internal:host-gateway`) so session containers can reach the MCP listener on the host. On macOS the data directory must lie under a path the Podman machine shares with its VM (the home directory by default).

**Session image**, built from the repository root:

```bash
podman build -t mars-session-claude:$(sed -n 's/^ARG CLAUDE_CODE_VERSION=//p' images/claude/Dockerfile) -t mars-session-claude:latest images/claude
podman build -t mars-session-claude-dev:$(sed -n 's/^ARG CLAUDE_CODE_VERSION=//p' images/claude/Dockerfile) -t mars-session-claude-dev:latest images/claude-dev
podman build -t mars-session-stub:latest images/stub
```

The order matters: the dev image is built `FROM` the base (`ARG BASE_IMAGE=mars-session-claude:latest`), so the base must already exist under that tag. Both take their version tag from the one `ARG CLAUDE_CODE_VERSION=` line in `images/claude/Dockerfile`, so the CLI is pinned in a single place; `:latest` is an alias for the same build, and `SESSION_IMAGE_DEFAULT` points at one of them — by default at `mars-session-claude-dev:latest`. The dev image adds a pinned Rust toolchain (rustup and cargo under `/opt`, owned by `agent`), `cargo-binstall`, the system libraries a session cannot install without root, and `corepack` with an unprivileged npm global prefix (`ARCHITECTURE.md`, "Session image"; ADR 0039); it is roughly 1.9 GB against the base's 850 MB, so expect a slow first pull. A repository that needs a system library the dev image does not carry builds its own image `FROM mars-session-claude-dev` and names it in its profiles. The stub image replays a recorded transcript instead of calling a model, so tests run on it without credentials. `ENGINE=podman images/smoke-test.sh` checks the base and the stub; adding `DEV_IMAGE=mars-session-claude-dev:latest` checks the dev image too — the same entrypoint and uid contract, plus that `cargo`, `rustc`, `cargo clippy`, `rustfmt`, `cargo binstall`, `node` and `npm` resolve over an empty `/session/home` mount both for the agent's command and in `/bin/bash -l`, and that an offline `cargo build` succeeds. With `DEV_IMAGE` unset those checks print a skip line. With Docker, run the same commands with `docker build`.

`orchestrator/tests/session_e2e.rs` runs the session lifecycle on real containers and needs the stub image; it takes the tag from `MARS_STUB_IMAGE` and defaults to `localhost/mars-session-stub:dev`, so either build it under that tag (`podman build -t localhost/mars-session-stub:dev images/stub`) or point the variable at the tag you have. `orchestrator/tests/engine.rs` reads the same variable for its one scenario that needs a real session image — the terminal running `/bin/bash -l` as `agent` — but has no default for it: with `MARS_STUB_IMAGE` unset that scenario prints a line and passes, and the rest of the engine suite runs on a plain `alpine` image. Like the rest of the engine suite both run only with `DOCKER_HOST` set, and both run under `cargo test --test engine --test session_e2e` rather than under `cargo nextest`, which excludes them (ADR 0037). The test suite starts its own Postgres through testcontainers; on an engine that cannot publish a port to it, set `MARS_TEST_POSTGRES_URL` (`postgres://user:password@host:port`, no database name) to a throw-away server and the suite uses that instead, building its template database there.

**Deployment images**, built from the repository root. The orchestrator image is a multi-stage build that compiles the crate offline (`SQLX_OFFLINE=true`, from the committed `.sqlx/`, so no `DATABASE_URL` is needed) and ships the binary, `git` and CA certificates on `debian:trixie-slim`, running as uid 1000 under a read-only root filesystem with a tmpfs at `/tmp` (`ARCHITECTURE.md`, "Trust boundaries"; ADR 0012):

```bash
podman build -t mars-orchestrator:dev orchestrator
```

The builder stage's `rust:<version>-trixie` tag and `orchestrator/rust-toolchain.toml` must move together, and the builder's Debian release must stay the same as the runtime stage's, because Orchestrator CI runs the git tests in that builder image to check the git the runtime ships; the Dockerfile says both at the `FROM` line.

The nginx image builds the frontend with `npm ci && npm run build` on `node:22-alpine` and serves the result from `nginx:1.27-alpine`, with `nginx/nginx.conf` and `nginx/default.conf.template` installed so the official entrypoint renders the server block from `ORCHESTRATOR_HOST` and `API_PORT`. Its build context is the repository root, because it needs both `frontend/` and `nginx/`:

```bash
podman build -f nginx/Dockerfile -t mars-nginx:dev .
```

With Docker, run either command with `docker build`. `compose build` builds both images under the tags `compose.yml` names, `mars-orchestrator:latest` and `mars-nginx:latest`.

**Frontend**:

```bash
cd frontend
npm install
npm run dev                  # proxies /api and /ws to the orchestrator
```

The proxy targets `http://localhost:7000` (the `API_PORT` default); set `VITE_API_TARGET` to point the dev server at a different orchestrator.

End-to-end tests need a browser once per machine:

```bash
npx playwright install --with-deps chromium
```

The suite itself runs against a real stack, which "End-to-end tests" below brings up. `PLAYWRIGHT_BASE_URL` points Playwright at the frontend under test (default `http://localhost:5173`, the Vite dev server, which Playwright starts itself or reuses) and `PLAYWRIGHT_API_URL` at the orchestrator its helpers call directly; the stack writes both, and a helper that finds `PLAYWRIGHT_API_URL` unset fails naming `npm run test:e2e:up`.

### End-to-end tests

`frontend/tests/e2e-stack.sh` brings up everything the Playwright scenarios need, the same way on a developer machine and in CI: a fresh Postgres container, the stub session image, and an orchestrator built with `--features integration-tests` running **on the host** exactly as "Running locally" describes, with its log captured to a file:

```bash
cd frontend
npm run test:e2e:up          # postgres, stub image, cargo build, orchestrator, health
npm run test:e2e             # the Playwright suite
npm run test:e2e:down        # stops everything and removes the run's state
npm run test:e2e:status      # what is up right now
```

`up` writes `frontend/.e2e/env`, which `playwright.config.ts` reads and copies into the environment for any variable that is not already set, so the suite needs no hand-set variables. The file carries `PLAYWRIGHT_BASE_URL`, `PLAYWRIGHT_API_URL`, `PLAYWRIGHT_ORCHESTRATOR_LOG` (the log the invite scenarios read the `LogEmailClient` links from — `RUST_LOG=info`, no `RESEND_API_KEY`), `PLAYWRIGHT_DATA_DIR` (`DATA_DIR`, equal to `DATA_DIR_HOST`, so tests can read the session work clones and the mirrors at `projects/<id>/repo.git`), `PLAYWRIGHT_REPOS_DIR` (scratch bare repositories), `PLAYWRIGHT_STUB_IMAGE` and `PLAYWRIGHT_ENGINE`. The rest of `frontend/.e2e/` is the orchestrator log, its pid and the data directory; the whole directory is git-ignored and `down` deletes it.

The orchestrator it starts carries the test-only routes of `SPEC.md`, "Test-only routes", because it is built with `--features integration-tests`: `POST /api/test/users` is where every scenario's user comes from, `GET /api/test/stream-whoami` is the stream-authentication probe, and `POST /api/test/scheduler-tick` runs the scheduled-agent job once with the two instants of its window in the body — a `started_at` floor a couple of minutes before `now` makes `* * * * *` due immediately, which is how the schedule scenarios fire a tick without waiting for a minute boundary.

`frontend/tests/README.md` is the suite's own document: what each fixture gives a scenario, how to add one, the wall-clock of a whole run and the coverage table over `SPEC.md`, "User-facing features" and "Frontend" — with the exclusions and why each one is excluded. `npm run test:e2e` checks that table before the run and the run's skips after it (`node frontend/tests/coverage-check.mjs`).

The stack sets every orchestrator variable itself and ignores the repository's `.env`: its orchestrator starts with an emptied environment in `frontend/.e2e/run`, below the two directories the `.env` lookup reads, and `RESEND_API_KEY`, `MAIL_FROM`, `SECRETS_MASTER_KEY_FILE` and the two `SESSION_NETWORK_*` variables are passed empty, which the orchestrator reads as unset and no `.env` overrides. `up` then checks the orchestrator's startup line for the logging email client and stops it with an error if it chose Resend, because the scenarios that follow an invitation, reset or escalation link read it from that log, and a run must never send the suite's mail through a real account. Its database always starts empty, so the seeded `admin` / `changeme` of "Start" exists again on every `up` — and the one scenario that signs in as that account consumes it, so a second `npm run test:e2e` against the same stack skips it. A third consecutive full run against one stack fails outright: the suite's deliberate wrong-password scenarios cross the login throttle's ten failures in fifteen minutes and every login is then answered 429. Bring the stack down and up between full runs; the throttle is in the orchestrator's memory and a restart clears it. The knobs:

| Variable | Meaning |
| --- | --- |
| `E2E_ENGINE` | `podman` (default) or `docker`; sets `DOCKER_HOST` unless one is already exported, and selects `host.containers.internal` or `host.docker.internal` for `MCP_URL` and `SESSION_EXTRA_HOSTS`. |
| `E2E_PG_PORT`, `E2E_PG_CONTAINER` | Host port and name of the Postgres container (defaults `5433` and `mars-e2e-pg`, so the `mars-pg` of "Running locally" can stay up). Two stacks on one engine need different names and ports; their state is already separate, because each lives in its own checkout's `frontend/.e2e/`. |
| `E2E_API_PORT`, `E2E_MCP_PORT` | `API_PORT` and `MCP_PORT` (defaults `7000` and `7001`). The Playwright `webServer` passes `VITE_API_TARGET` so the dev server proxies to the API port actually in use. |
| `E2E_BASE_URL` | The frontend origin, used for both `PUBLIC_URL` and `PLAYWRIGHT_BASE_URL` (default `http://localhost:5173`). |
| `E2E_STUB_IMAGE` | Tag built from `images/stub` and passed as `SESSION_IMAGE_DEFAULT`, so a new project's seeded profiles run the stub (default `localhost/mars-session-stub:dev`). |
| `E2E_SKIP_IMAGE_BUILD`, `E2E_SKIP_CARGO_BUILD` | `1` reuses the image or the binary already built. |
| `E2E_KEEP_LOG` | `1` keeps `frontend/.e2e/orchestrator.log` when `down` deletes the rest. |

`up` checks its preconditions first and fails with one line each: the engine socket answers `info`, `cargo`, `git`, `openssl` and `curl` are on `PATH`, the three ports are free, and rootless Podman has the subordinate ids `keep-id` needs. On Docker with a uid other than 1000 it warns that the uid contract is not met (`ARCHITECTURE.md`, "Uid contract") and continues. If `GET /api/health` never reaches 200 within 180 seconds it prints the health JSON — `engine: false` means the socket is wrong — and the last 50 lines of the log.

`down` stops the orchestrator (SIGTERM, then SIGKILL after 10 seconds), removes `mars-e2e-pg`, and removes the session and probe containers this stack started: they are recognised by the data directory bind-mounted into them, because the `mars.session_id` label and the `mars-session-<id>` name are the same for any orchestrator on the engine, so a developer's own sessions are left alone.

### CI

| Workflow | Triggers on | Checks |
| --- | --- | --- |
| Orchestrator CI | `orchestrator/**` | fmt, clippy (plain and with `integration-tests`), tests under `cargo nextest` plus the doctests, with `SQLX_OFFLINE=true`; a second job reruns the git tests that need no database or engine inside two older gits rather than the runner's — `rust:1.98.1-trixie`, the Dockerfile's builder base and so the git the orchestrator image ships, and `rust:1.98.1-bookworm`, which carries the documented minimum, git 2.39.5; a third job checks `orchestrator/.sqlx/` for staleness with `cargo sqlx prepare --check` against a `postgres:18` service |
| Engine | `orchestrator/**` or `images/**` | `tests/engine.rs` against the runner's Docker daemon and against rootless Podman via its compatible socket; then the stub session image is built with that engine and `tests/session_e2e.rs` runs the session lifecycle on real containers, with a `postgres:18` service container as its database through `MARS_TEST_POSTGRES_URL` (on Docker the runner's uid is not 1000, so that binary reports the uid contract and returns) |
| Frontend CI | `frontend/**` | lint, typecheck, unit tests, build; `npm run build` ends in `scripts/check-entry-chunk.mjs`, which fails if the chunks a first paint fetches carry feature UI or exceed the first-paint byte budget (`SPEC.md`, "Frontend", "Code splitting") |
| E2E | `orchestrator/**`, `frontend/**` or `images/**` | Playwright against a real orchestrator, Postgres and the stub session image on rootless Podman, all brought up by `frontend/tests/e2e-stack.sh`; the report, traces and orchestrator log are uploaded on failure |
| Images | `images/**` | Lint the entrypoint, Dockerfiles and stub; build all three session images — base, dev and stub — on Docker and Podman; run `images/smoke-test.sh` over them |
| Deploy | Dockerfiles, `nginx/`, compose files | Build orchestrator and nginx images on Docker and Podman; `nginx -t`; the Content-Security-Policy on real responses from the nginx image; compose config for both overrides |

## Roadmap after v1

Address the accepted git-execution vulnerability by isolating operations on agent-controlled checkouts from orchestrator privileges (ADR 0019); a restricted git helper container is the current candidate.

Add durable input delivery and recovery handling for messages interrupted by orchestrator restarts, including deduplication and ambiguous delivery (ADR 0020).

An agent that turns GitHub issues into backlog tasks, with the GitHub App credentials and webhooks it needs; egress restriction for session containers; sandboxed runtimes (gVisor, Kata) per profile; per-project toolchain setup scripts for session images; a second agent backend (GitHub Copilot CLI is the candidate, pending a spike to learn its structured output, stdin protocol and container authentication).

## License

MIT.
