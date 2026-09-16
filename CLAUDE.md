# Claude Instructions — Mars

Working conventions for any agent or human changing this repository. The product itself is described in `README.md`, `ARCHITECTURE.md`, `SPEC.md` and `docs/`; this file is about how to work here.

## Mandatory rules

1. **Documentation is part of the change.** Any change to behaviour, an endpoint, a schema, a config variable, a container contract or a tool description updates the document that describes it in the same commit: `SPEC.md` for endpoints, streams, event schemas and MCP tools; `docs/data-model.md` for tables; `ARCHITECTURE.md` for lifecycle, recovery, git, secrets and engine behaviour; `README.md` for configuration and operation; a new ADR in `docs/decisions/` when a real alternative was rejected. A change that resolves an entry in `docs/open-questions.md` deletes that entry.
2. **Issue tracking**: use Bears for all issue tracking, through the `bears` MCP server (`list_ready` to find work, `start_task` to claim, `complete_task` to complete). The `bea` CLI is the fallback when the MCP server is not available. No markdown TODO lists.
3. **Planning**: break significant changes into an epic with sub-tasks (`create_task` with `type: epic`, then `create_task` with `parent`) and link dependencies with `add_dependency`. Tasks cite the section of the document they implement.
4. **Secrets never appear in code, tests, fixtures, logs, events or documentation.** Test fixtures use obviously fake values.

## Project overview

Mars runs coding-agent sessions (Claude Code in v1, behind a pluggable `AgentBackend` trait) in one container per session and exposes them in a browser, with a task tracker shared between agents (over MCP) and users (in the UI).

| Directory | Stack | Status |
| --- | --- | --- |
| `orchestrator/` | Rust 2024, Axum, SQLx, Postgres, bollard, rmcp | Not started |
| `frontend/` | Vite, React 19, TypeScript, Tailwind 4, TanStack Query, Zustand | Not started |
| `images/` | Session container images (Dockerfiles + entrypoint) | Not started |
| `nginx/` | Frontend image and reverse proxy config | Not started |

## Backend conventions

**Toolchain**: Rust stable, edition 2024, pinned by `rust-toolchain.toml`. `cargo fmt` and `cargo clippy -- -D warnings` clean at all times.

**Crates** (add with `cargo add`, never by editing versions by hand):

| Concern | Crate |
| --- | --- |
| HTTP, WebSocket, SSE | `axum` (features `ws`), `axum-extra` (`cookie`, `typed-header`), `tower-http` (`trace`, `cors`) |
| Database | `sqlx` (`postgres`, `runtime-tokio`, `uuid`, `chrono`, `json`) |
| Container engine | `bollard` |
| MCP server | `rmcp` (server, Streamable HTTP transport) |
| Async runtime | `tokio` (`full`), `tokio-stream`, `futures-util` |
| Auth | `jsonwebtoken`, `argon2`, `sha2` |
| Secrets | `aes-gcm`, `rand`, `zeroize`, `base64` |
| Serialisation | `serde`, `serde_json` |
| Errors | `thiserror` |
| Logging | `tracing`, `tracing-subscriber` (`env-filter`) |
| Email | `reqwest` against the Resend HTTP API (no SDK crate) |
| Ids, time | `uuid` (`v4`, `serde`), `chrono` (`serde`) |
| Config | `dotenvy` |
| Tests | `axum-test`, `testcontainers-modules` (`postgres`), `tempfile` |

Git is never a crate: all git operations shell out to the `git` binary through `orchestrator/src/git/` (ADR 0011).

**Layout**: see `ARCHITECTURE.md`, "Orchestrator internals". In short:

- `src/prelude/`: `AppState`, `Config`, `Claims`, `Error`, `Result`. Every module does `use crate::prelude::*`.
- `src/models/`: domain types with their validation and a per-model error enum (`UserError`, `TaskError`, ...). No SQL.
- `src/repositories/`: all SQL, one `XRepository<'a>` struct per aggregate borrowing `&PgPool`. Use `sqlx::query!`/`query_as!` (compile-time checked). Scoped mutations put the scope in the `WHERE` clause (`... WHERE id = $1 AND project_id = $2`), never mutate by id and check afterwards.
- `src/routes/`: one module per resource exporting `routes() -> Router<AppState>`, nested under `/api`. Request and response DTOs are private to the route module; response DTOs exist when the shape differs from the model.
- `src/engine/`, `src/agent/`, `src/git/`, `src/secrets/`, `src/email/`: each exposes a trait, a production implementation and a mock. `AppState` holds them as `Arc<dyn Trait>`. Mocks are compiled behind the `integration-tests` cargo feature and expose `as_any()` for downcasting in tests.
- `src/session/`: `SessionOwner` tasks, launcher, recovery, registry.
- `src/events/`: `AgentEvent`, `TaskEvent`, translation helpers, notify fan-out.
- `src/cron/`: periodic jobs, each a method on `CronService`, each failure logged and retried next interval.

**Error handling**: one `Error` enum in `src/prelude/error.rs` with `#[from]` variants for `sqlx::Error`, `ClaimsError`, each model error, `EngineError`, `GitError`, `SecretsError`, `EmailError`, plus `NotFound`, `Forbidden`, `Conflict(String)`, `BadRequest(String)`, `Internal(String)`. `impl IntoResponse for Error` maps to `{ "status": <u16>, "error": "<message>" }`; internal errors log with `tracing::error!` and return a generic message. `Result<T>` is `std::result::Result<T, Error>`. Functions return `Result`; `unwrap`/`expect` only in tests and at startup. MCP tool handlers map `Error` to MCP error codes in `src/mcp/error.rs`.

**Logging**: `tracing` with structured fields (`session_id = %id`), never string-formatted ids. Never log secret values, tokens, or event payloads at `info` or above.

**Migrations**: `sqlx migrate add -r <name>` creates a paired `.up.sql`/`.down.sql` in `orchestrator/migrations/`. Migrations run automatically at startup. Every `.down.sql` fully reverses its `.up.sql`. `docs/data-model.md` is updated in the same commit. Enum values are only added, never removed or renamed.

**SQLx offline mode**: after changing any query, run `cargo sqlx prepare` in `orchestrator/` and commit `.sqlx/`. CI and the Docker build run with `SQLX_OFFLINE=true`. A build that fails with "no cached data for this query" means `.sqlx/` is stale.

**Environment**: see `.env.example` for every variable; `Config::from_env()` fails fast with the missing variable's name.

## Frontend conventions

**Stack**: Vite, React 19, TypeScript (strict), Tailwind CSS 4, React Router 7, TanStack Query, Zustand, `@tanstack/react-virtual`, `react-markdown`, `xterm.js`, Heroicons (`@heroicons/react/24/outline`), ESLint, Playwright.

- Functional components with hooks only; named exports.
- All API calls go through `src/services/`; components never call `fetch`. Use `apiGet`/`apiPost`/`apiPut`/`apiPatch`/`apiDelete` from `services/apiClient.ts`, which attaches the access token and refreshes once on 401.
- Server state through TanStack Query; session transcript and task board state through the Zustand stores in `src/session/` and `src/tasks/`. Never keep raw event arrays in state; fold events as they arrive (`SPEC.md`, "Frontend").
- `useAuth()` for auth state, `useFormSubmit()` for form loading and error state.
- Shared layouts: `AuthLayout`, `PageLayout`. Shared UI: `FormField`, `SubmitButton`, `Alert`, `LoadingState`, `EmptyState`, `SectionHeader`. Protected routes use `ProtectedRoute`, admin routes `AdminRoute`.
- Types in `src/types/` mirror the shapes in `SPEC.md` exactly, field names in `snake_case` as the API sends them.
- Invoke the `/frontend-design` skill before creating or reshaping UI. The tone is a focused, dense operator console: dark-friendly, monospace where content is code or logs, quiet colour reserved for state (running, parked, failed, needs human).

## API conventions

- Plural nouns under `/api`; nested sub-resources for project-scoped things (`/projects/{id}/sessions`); verbs for actions (`/sessions/{id}/stop`).
- 201 with body for creates, 204 for deletes, 200 otherwise, 202 for accepted asynchronous inputs. Errors are `{ "status", "error" }`.
- Bare JSON, no envelopes. Timestamps RFC 3339. Ids UUID strings.
- Any new endpoint, message or event kind is added to `SPEC.md` in the same commit.

## Running locally

**Postgres**:

```bash
podman run -d --name mars-pg -e POSTGRES_USER=mars -e POSTGRES_PASSWORD=mars -e POSTGRES_DB=mars -p 5432:5432 postgres:18
export DATABASE_URL=postgres://mars:mars@localhost:5432/mars
```

**Podman socket** (rootless):

```bash
systemctl --user enable --now podman.socket
export DOCKER_HOST=unix://$XDG_RUNTIME_DIR/podman/podman.sock
```

With Docker instead: `export DOCKER_HOST=unix:///var/run/docker.sock`. Nothing else differs.

**Orchestrator**:

```bash
cd orchestrator
cp ../.env.example ../.env   # then edit
cargo run                    # runs migrations, listens on API_PORT and MCP_PORT
```

`DATA_DIR_HOST` must point at a directory the current user owns; when running the orchestrator directly on the host it is the same path as `DATA_DIR` (default `./data`).

**Session image**:

```bash
podman build -t mars-session-claude:dev images/claude
```

**Frontend**:

```bash
cd frontend
npm install
npm run dev                  # proxies /api and /ws to the orchestrator
```

## Code quality

After every backend change:

```bash
cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests
```

After every frontend change:

```bash
cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:e2e
```

## Testing expectations

- **Backend integration tests** use `TestApp::spawn()` from `tests/common/mod.rs`: a `testcontainers` Postgres, migrations applied, the seeded admin removed, `AppState` built with the mock engine, mock email, mock git credential provider and a fixed test master key. `TestApp` creates users directly in the database and logs them in; invite flows are tested through the mock email client's captured messages. One `#[tokio::test]` per scenario. Every new endpoint gets tests for the happy path and each error path (unauthenticated, forbidden, validation, conflict). Assertions through `response.assert_status()` and `response.json::<T>()`.
- **Engine tests** (`tests/engine.rs`) run only when `DOCKER_HOST` is set and exercise create, attach, exec, kill and label listing on whatever engine is configured; CI runs them on both Podman and Docker.
- **Git tests** create real bare repositories in temporary directories with `tempfile`; no mocking of git.
- **Event translation tests** are fixture-based: recorded native output per pinned CLI version in `tests/fixtures/claude/<version>/`, expected `AgentEvent` sequences beside them. A CLI version bump adds fixtures, never edits old ones.
- **MCP tests** drive the tool handlers through the `rmcp` server in-process with a session bearer token from `TestApp`.
- **Session owner tests** feed a transcript file line by line, kill and restart the owner mid-file, and assert that `events` has no gaps and no duplicates.
- **Frontend end-to-end tests** (Playwright, `workers: 1`) run against a real orchestrator started with `--features integration-tests`, create fresh users per test through the test-only `/api/test/users` endpoint (there is no self-registration), and use helpers from `tests/utils/test-helpers.ts`. Sessions in E2E use a stub session image that replays a fixture transcript so tests need no model credentials.

## Issue tracking (Bears)

Prefer the `bears` MCP server (registered in `.mcp.json`, backed by `bea mcp`). Tools by task:

| Need | MCP tool |
| --- | --- |
| Find unblocked work | `list_ready` |
| View a task | `get_task`, `search_tasks`, `get_graph`, `plan_epic` (an epic's children in execution order) |
| Create work | `create_task` (with `type: epic` for an epic, `parent` for a sub-task) |
| Claim / complete | `start_task`, `complete_task` |
| Dependencies | `add_dependency`, `remove_dependency` |
| Housekeeping | `update_task`, `cancel_task`, `archive_task` |

The `bea` CLI is the fallback when the MCP server is unavailable (for example in a plain shell or CI):

```bash
bea init                # Initialise .bears/ in the repo
bea ready --json        # Find unblocked work
bea show <id> --json    # View task details
bea create "Title" --priority P2 --json
bea start <id> --json   # Claim
bea done <id> --json    # Complete
bea dep add <task-id> <depends-on-id> --json
```

Types: `bug`, `feature`, `task`, `epic`, `chore`. Priorities: `P0` critical, `P1` high, `P2` medium, `P3` low. Newly discovered work becomes a new task linked to the current one. Commit messages include the task id.

## Git workflow

- Feature branches from `main`; rebase before merging, no merge commits; `gh pr create`.
- Conventional Commits: `<type>(<scope>): <description>` with types `feat`, `fix`, `docs`, `chore`, `refactor`, `test`, `style`, `perf`, `ci`, `build` and scopes `orchestrator`, `frontend`, `images`, `infra`, `docs`.
- A PR that changes behaviour without touching the corresponding document is not mergeable (rule 1).

## CI

| Workflow | Triggers on | Checks |
| --- | --- | --- |
| Orchestrator CI | `orchestrator/**` | fmt, clippy, tests with `SQLX_OFFLINE=true` |
| Frontend CI | `frontend/**` | lint, typecheck, build |
| E2E | `orchestrator/**` or `frontend/**` | Playwright against a real orchestrator, Postgres and the stub session image |
| Images | `images/**` | Build session images; smoke-run the entrypoint |
