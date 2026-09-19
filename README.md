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

- **orchestrator**: Rust (`axum`, `bollard`, `sqlx`, `rmcp`). Serves the API, owns every session, holds the engine socket, runs `git`. Unprivileged user.
- **postgres**: the only system of record.
- **nginx**: serves the built React frontend and proxies `/api` and `/ws`. The MCP endpoint is not proxied.
- **session containers**: one per session, created by the orchestrator, on an internal network for MCP and a separate egress network for the internet, never given the engine socket.

Rootless Podman is the target engine, reached through its Docker-compatible socket. Docker works with a different `DOCKER_HOST`.

## Running it

These are the intended steps once the implementation exists.

### Prerequisites

- A Linux host with rootless Podman 4.9+ (or Docker 24+); the engine tests run on Podman 4.9.3 and 6.1.2 and on Docker 28.0.4.
- `podman-compose` or `docker compose`.
- A directory for persistent data, for example `/srv/mars/data`, owned by the service user.
- For private repositories: a fine-grained GitHub personal access token scoped to the repository.
- Model credentials: an `ANTHROPIC_API_KEY`, or a `CLAUDE_CODE_OAUTH_TOKEN` from `claude setup-token` (requires a Pro or Max subscription). Never both for the same session.

### Podman setup (once, as the service user)

```bash
# Let the user's services run without an active login session
sudo loginctl enable-linger "$USER"

# Start the Docker-compatible API socket with socket activation
systemctl --user enable --now podman.socket

# The socket the orchestrator will use
echo "unix://$XDG_RUNTIME_DIR/podman/podman.sock"
```

The compose file runs the orchestrator with `userns_mode: keep-id` and mounts that socket, so the orchestrator's uid inside the container matches the service user on the host. Session containers run with `keep-id:uid=1000,gid=1000`, which maps the service user to the image's `agent` user (uid 1000) whatever the service user's uid is; the orchestrator verifies this at startup with a probe container and refuses to start if Podman does not honour it. That form of `keep-id` needs Podman 4.3 or newer, and the engine tests have verified it through the compatibility API on Podman 4.9.3 and 6.1.2. Podman resolves `keep-id` through the non-thread-safe `libsubid`, so it cannot do it for two containers at once: with twenty creates in flight on Podman 6.1.2 about one container in fourteen comes out with a broken uid mapping and then fails to start (`doesn't map UID 0`, `write to uid_map: Operation not permitted`), and the API service occasionally crashes in that call and is restarted by its socket unit. No fixed version is known, so the orchestrator creates containers one at a time per host: its engine adapter holds a lock across each container creation and releases it again before starting the container, so two sessions launched at once simply take turns creating their containers. The `podman` CLI is unaffected, because each invocation is its own process. If the compatibility API cannot apply `keep-id` to the orchestrator container, it can run as a plain user systemd service. Session containers still require `keep-id:uid=1000,gid=1000` support and must pass the startup probe.

For Docker, use the daemon's socket (`unix:///var/run/docker.sock`) and a user in the `docker` group. The supported Docker deployment uses the default uid mapping, so the orchestrator service runs as uid 1000 (`user: "1000:1000"` in the compose file) and the data directory must be owned by uid 1000. The session uid and data-directory ownership requirements still apply.

### Configuration

Copy `.env.example` to `.env` and set:

| Variable | Meaning |
| --- | --- |
| `PUBLIC_URL` | The URL users open, used for cookies and email links. |
| `JWT_SECRET` | Secret for signing access tokens. |
| `DATABASE_URL` | Postgres connection string (compose sets it for the orchestrator). |
| `POSTGRES_USER`, `POSTGRES_PASSWORD`, `POSTGRES_DB` | Database bootstrap. |
| `DOCKER_HOST` | Engine socket, `unix:///run/user/1000/podman/podman.sock` for rootless Podman. |
| `DATA_DIR_HOST` | Host path of the data directory; mounted at `/data` in the orchestrator and used as the source of session bind mounts. A bind-mount source must be absolute, so a relative path is resolved against the orchestrator's working directory at startup. |
| `DATA_DIR` | Path at which the orchestrator itself sees the data directory: `/data` in compose, the same as `DATA_DIR_HOST` when running on the host. |
| `MCP_URL` | URL written into each session's MCP config; default `http://orchestrator:7001/mcp`. On a development host: `http://host.containers.internal:7001/mcp`. |
| `SESSION_NETWORK_INTERNAL`, `SESSION_NETWORK_EGRESS` | Names of the two session networks (default `mars-sessions`, `mars-egress`); created at startup if missing. |
| `SESSION_EXTRA_HOSTS` | Optional comma-separated `host:ip` entries added to session containers, e.g. `host.containers.internal:host-gateway` for development. |
| `SECRETS_MASTER_KEYS` | One or more `<version>=<base64 32-byte key>` entries, comma separated. Or `SECRETS_MASTER_KEY_FILE`, a file with the same content, which should be readable only by its owner — a broader mode is accepted with a warning, because a mounted container secret's mode is not always the operator's to set. Back this up separately from the database; without it every stored secret is unrecoverable. The orchestrator refuses to start when a `key_version` present in `secrets` has no configured key or cannot be unwrapped with the one configured for it. |
| `GIT_BOT_NAME`, `GIT_BOT_EMAIL` | Identity for commits the orchestrator creates (merges). |
| `API_PORT` | Port of the API listener nginx proxies to (default 7000). |
| `MCP_PORT` | Port of the MCP listener on the sessions network (default 7001). |
| `STOP_GRACE_SECS` | Seconds between SIGINT and SIGTERM when stopping a session (default 20). |
| `MIRROR_FETCH_INTERVAL_SECS` | How often project mirrors are fetched (default 600). |
| `SESSION_IMAGE_DEFAULT` | Image used by the default profile of new projects and by the startup probe (default `mars-session-claude:latest`). Both pull it, so it has to exist on the engine before the first start; build it as described under "Session image". |
| `RESEND_API_KEY`, `MAIL_FROM` | Email delivery through Resend, used for invites, password resets and task escalations. `MAIL_FROM` is required once `RESEND_API_KEY` is set. Without an API key, full usable links including their tokens are intentionally written to the orchestrator log at `info` instead of sent. This supports local development without email configuration; no extra flag is required (ADR 0026). |
| `RUST_LOG` | Log filter, `info` by default and whenever the given filter is unusable, such as the bare non-level word `verbose`. |

Generate a master key with `openssl rand -base64 32`.

### Start

```bash
podman-compose up -d        # or: docker compose up -d
```

The orchestrator applies database migrations on startup; the first migration seeds an administrator:

| Username | Password |
| --- | --- |
| `admin` | `changeme` |

The fixed bootstrap credentials are intentional for v1: the operator controls initial setup and completes the first-login password change before making the instance available to other users. No separate initial-password setting or setup wizard is required (ADR 0024).

Changing or resetting a user's password invalidates their previous logins. Changing your own password keeps the current browser signed in with new credentials. Deleted users lose access and administrator-role changes apply on subsequent requests; open connections check for revoked logins at their heartbeat ticks (ADR 0025). Running agent sessions continue independently of user logins.

Open `PUBLIC_URL`, log in as `admin`, and you are required to set a new password before anything else works. Then invite your team from the admin page (each invite is a 7-day link sent by email), create a project from a remote URL, and launch a session from its default profile. There is no self-registration.

Work flows through the project's task board. Search across its columns by title or exact task number (`42` or `#42`). Use `Copy link` in a task or session header to reference it in comments or share its direct URL with teammates; opening it requires login. Create a task (it lands in `backlog`), open it in a planning session to break it down, and the resulting `ready` tasks are what an implementer session picks up with its `ready` and `claim` tools. In v1 every session is started by a person, optionally for one task, either as a conversation or as a one-shot run of an ephemeral profile; a task that agents keep failing on ends up in `needs_human` after `max_attempts` (project setting, default 3) with the agents' comments explaining why.

Code hand-offs keep the producing session, branch, exact commit and a comment together. Opening the next session on that task defaults to the handed-over commit, so reviewers see the implementation they were asked to review. Review approval belongs to that commit; submitting a revised commit starts a new review. The task's merge action merges the approved revision, even if the original session branch has since changed.

### Operating notes

- **Known v1 vulnerability:** git commands run by the orchestrator against an agent-controlled checkout can execute helpers configured by that agent, with the orchestrator's access to secrets, project data and the engine socket. This risk is explicitly accepted for v1; isolating those git operations is deferred. See [ADR 0019](docs/decisions/0019-defer-isolation-of-git-checkout-operations.md). Session containers are not a complete containment guarantee while this remains unresolved.
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
├── images/             session container images (claude/, stub/)
├── nginx/              (planned) nginx.conf and Dockerfile for the frontend image
└── compose.yml         (planned)
```

Working conventions, code-quality commands and test expectations are in `CLAUDE.md`.

### Running locally

The orchestrator, the frontend, the session images and `.env.example` are in the repository; `compose.yml` and the nginx image are not, so the steps that need them are marked as planned. The socket commands assume Linux.

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

The backend tests reach this socket too: each test binary starts one `postgres:18` container of its own and gives every test a database on it (`CLAUDE.md`, "Testing expectations"), and removes the container when the process exits.

**Orchestrator**:

```bash
cd orchestrator
cp ../.env.example ../.env   # then edit
cargo run                    # runs migrations, listens on API_PORT and MCP_PORT
```

`Config::from_env` looks for `.env` in the current directory and then in `../.env`, so running from `orchestrator/` picks up the repository-root file. `dotenvy` never overrides a variable that is already set, so the `DATABASE_URL` exported above wins over the compose-oriented `postgres` host in `.env.example`.

`DATA_DIR_HOST` must point at a directory the current user owns; when running the orchestrator directly on the host it is the same path as `DATA_DIR` (default `./data`, made absolute at startup). Set `MCP_URL=http://host.containers.internal:7001/mcp` (Docker: `host.docker.internal`, plus `SESSION_EXTRA_HOSTS=host.docker.internal:host-gateway`) so session containers can reach the MCP listener on the host. On macOS the data directory must lie under a path the Podman machine shares with its VM (the home directory by default).

**Session image**, built from the repository root:

```bash
podman build -t mars-session-claude:$(sed -n 's/^ARG CLAUDE_CODE_VERSION=//p' images/claude/Dockerfile) -t mars-session-claude:latest images/claude
podman build -t mars-session-stub:latest images/stub
```

The claude image's version tag is the CLI version pinned in `images/claude/Dockerfile` and `mars-session-claude:latest` is an alias for that same build, which is what `SESSION_IMAGE_DEFAULT` points at; the stub image replays a recorded transcript instead of calling a model, so tests run on it without credentials. `ENGINE=podman images/smoke-test.sh` checks both. With Docker, run the same two commands with `docker build`.

`orchestrator/tests/session_e2e.rs` runs the session lifecycle on real containers and needs the stub image; it takes the tag from `MARS_STUB_IMAGE` and defaults to `localhost/mars-session-stub:dev`, so either build it under that tag (`podman build -t localhost/mars-session-stub:dev images/stub`) or point the variable at the tag you have. Like the rest of the engine suite it runs only with `DOCKER_HOST` set.

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
npm run test:e2e             # starts the dev server itself, or reuses a running one
```

`PLAYWRIGHT_BASE_URL` points Playwright at the frontend under test (default `http://localhost:5173`, the Vite dev server) and `PLAYWRIGHT_API_URL` at the orchestrator its helpers call directly (default `http://localhost:7000`).

### CI

| Workflow | Triggers on | Checks |
| --- | --- | --- |
| Orchestrator CI | `orchestrator/**` | fmt, clippy (plain and with `integration-tests`), tests with `SQLX_OFFLINE=true`; a second job checks `orchestrator/.sqlx/` for staleness with `cargo sqlx prepare --check` against a `postgres:18` service |
| Engine | `orchestrator/**` or `images/**` | `tests/engine.rs` against the runner's Docker daemon and against rootless Podman via its compatible socket; then the stub session image is built with that engine and `tests/session_e2e.rs` runs the session lifecycle on real containers (on Docker the runner's uid is not 1000, so that binary reports the uid contract and returns) |
| Frontend CI | `frontend/**` | lint, typecheck, unit tests, build |
| E2E | `orchestrator/**`, `frontend/**` or `images/**` | Playwright; the real orchestrator, Postgres and stub session image are added by their own epics |
| Images | `images/**` | Lint the entrypoint, Dockerfiles and stub; build both session images on Docker and Podman; run `images/smoke-test.sh` |

## Roadmap after v1

Address the accepted git-execution vulnerability by isolating operations on agent-controlled checkouts from orchestrator privileges (ADR 0019); a restricted git helper container is the current candidate.

Add durable input delivery and recovery handling for messages interrupted by orchestrator restarts, including deduplication and ambiguous delivery (ADR 0020).

A dispatcher that launches ephemeral sessions when a served task state has claimable work, bounded per profile; scheduled agents (a profile run on a cron expression, such as a daily tech-debt scan that files tasks, or an agent that turns GitHub issues into backlog tasks); GitHub App credentials and webhooks; egress restriction for session containers; sandboxed runtimes (gVisor, Kata) per profile; per-project toolchain setup scripts for session images; a second agent backend (GitHub Copilot CLI is the candidate, pending a spike to learn its structured output, stdin protocol and container authentication).

## License

MIT.
