# Claude Instructions — Mars

Working conventions for any agent or human changing this repository. The product is described in `README.md`, `ARCHITECTURE.md`, `SPEC.md` and `docs/`; this file is about how to work here.

## Mandatory rules

1. **Documentation is part of the change.** Any change to behaviour, an endpoint, a schema, a config variable, a container contract or a tool description updates the document that describes it in the same commit: `SPEC.md` for endpoints, streams, event schemas and MCP tools; `docs/data-model.md` for tables; `ARCHITECTURE.md` for lifecycle, recovery, git, secrets and engine behaviour; `README.md` for configuration and operation; a new ADR in `docs/decisions/` when a real alternative was rejected. The rule always lives in the main document; read an ADR only when changing or questioning the decision it records. A change that resolves an entry in `docs/open-questions.md` deletes that entry.
2. **Issue tracking is Bears**, through the `bears` MCP server: `list_ready` to find work, `start_task` to claim, `complete_task` to finish; `create_task` with `type: epic` and then `parent` to break work down, `add_dependency` to order it. Priorities run `P0` critical to `P3` low. Tasks cite the document section they implement, newly discovered work becomes a new task linked to the current one, and commit messages include the task id. In a plain shell or CI the `bea` CLI is the fallback (`bea ready --json`, `bea show <id> --json`, `bea start <id> --json`, `bea done <id> --json`). No markdown TODO lists.
3. **No real credentials** in code, tests, fixtures or documentation; fixtures use obviously fake values. The orchestrator never puts secret values or tokens into its own logs, diagnostics or generated events. Two documented exceptions: agent output and user messages are stored and displayed unredacted (ADR 0027), and when `RESEND_API_KEY` is unset `LogEmailClient` logs complete invitation and password-reset links at `info` so local development works without email (ADR 0026). `ResendClient` and ordinary request logs never log those links.

## Project overview

Mars runs coding-agent sessions (Claude Code in v1, behind a pluggable `AgentBackend` trait) in one container per session and exposes them in a browser, with a task tracker shared between agents (over MCP) and users (in the UI). `orchestrator/` is Rust (Axum, SQLx, Postgres, bollard, rmcp), `frontend/` is Vite, React and TypeScript, `images/` holds the session container images and `nginx/` the frontend image and reverse proxy.

## Backend conventions

- Rust stable, edition 2024, pinned by `rust-toolchain.toml`. `cargo fmt` and `cargo clippy --all-targets -- -D warnings` clean at all times. Crates are added with `cargo add`; the crate chosen for each concern is listed in `ARCHITECTURE.md`, "Orchestrator internals". Git is never a crate: everything shells out to the `git` binary through `src/git/` (ADR 0011).
- Module layout and the `Error` enum contract are in `ARCHITECTURE.md`, "Orchestrator internals". Every module does `use crate::prelude::*`. Models hold validation and a per-model error enum, never SQL. Repositories hold all SQL through `sqlx::query!`/`query_as!`, one `XRepository<'a>` per aggregate borrowing `&PgPool`, and put the scope in the `WHERE` clause (`... WHERE id = $1 AND project_id = $2`) instead of checking after mutating. Routes are one module per resource exporting `routes() -> Router<AppState>`, DTOs private to the module. Engine, agent, git, secrets and email each expose a trait, a production implementation and a mock behind the `integration-tests` feature with `as_any()` for downcasting.
- Locking and notification discipline is specified in `ARCHITECTURE.md`, "Task tracker" and "Event delivery": one project row lock per tracker mutation, one session row lock per event batch, any git lock before any database lock, `pg_notify` inside the writing transaction and never as a separate post-commit write (ADR 0021, 0028). Repository helpers accept the caller's transaction; tracker helpers accept the `TrackerMutation`'s locked connection.
- Functions return `Result`; `unwrap`/`expect` only in tests and at startup. Internal errors log with `tracing::error!` and return a generic message.
- Logging is `tracing` with structured fields (`session_id = %id`), never string-formatted ids. Never log event payloads at `info` or above. Secrets: rule 3.
- Migrations: `sqlx migrate add -r <name>`; every `.down.sql` fully reverses its `.up.sql`; enum values are only added, never removed or renamed; `docs/data-model.md` changes in the same commit.
- After changing any query, run `cargo sqlx prepare` in `orchestrator/` and commit `.sqlx/`. CI builds with `SQLX_OFFLINE=true`; "no cached data for this query" means `.sqlx/` is stale.
- `README.md`, "Configuration", is the variable contract and `.env.example` carries the same variables. `Config::from_env()` fails fast naming any missing required variable.

## Frontend conventions

**Stack**: Vite, React 19, TypeScript (strict), Tailwind CSS 4, React Router 7, TanStack Query, Zustand, `@tanstack/react-virtual`, `react-markdown`, `xterm.js`, Heroicons (`@heroicons/react/24/outline`), ESLint, Vitest, Playwright.

- Functional components with hooks only; named exports.
- All API calls go through `src/services/`; components never call `fetch`. Use `apiGet`/`apiPost`/`apiPut`/`apiPatch`/`apiDelete` from `services/apiClient.ts`, which attaches the access token and refreshes once on 401.
- Server state through TanStack Query; session transcript and task board state through the Zustand stores in `src/session/` and `src/tasks/`. Never keep raw event arrays in state. Fold session transcript events as they arrive; task events invalidate the board's REST snapshot and trigger its coalesced refresh path (ADR 0022; `SPEC.md`, "Frontend").
- `useAuth()` for auth state, `useFormSubmit()` for form loading and error state.
- Shared layouts: `AuthLayout`, `PageLayout`. Shared UI: `FormField`, `SubmitButton`, `Alert`, `LoadingState`, `EmptyState`, `SectionHeader`. Protected routes use `ProtectedRoute`, admin routes `AdminRoute`.
- Types in `src/types/` mirror the shapes in `SPEC.md` exactly, field names in `snake_case` as the API sends them.
- Invoke the `/frontend-design` skill before creating or reshaping UI. The tone is a focused, dense operator console: dark-friendly, monospace where content is code or logs, quiet colour reserved for state (running, parked, failed, needs human).

## API conventions

- Plural nouns under `/api`; nested sub-resources for project-scoped things (`/projects/{id}/sessions`); verbs for actions (`/sessions/{id}/stop`).
- Default to 201 with body for creates, 204 for deletes, 200 otherwise, and 202 for accepted asynchronous inputs; explicit endpoint contracts in `SPEC.md` take precedence. Errors are `{ "status", "error" }`; git conflicts additionally include `conflicts`.
- Bare JSON, no envelopes. Timestamps RFC 3339. Ids UUID strings.
- Any new endpoint, message or event kind is added to `SPEC.md` in the same commit.

## Running locally

The commands are in `README.md`, "Development": Postgres in a container, `DOCKER_HOST` pointed at the Podman or Docker socket, `cargo run` in `orchestrator/`, `npm run dev` in `frontend/`. The CI workflows are described there too.

## Code quality

After every backend change:

```bash
cd orchestrator && cargo fmt && cargo clippy --all-targets -- -D warnings && cargo clippy --all-targets --features integration-tests -- -D warnings && cargo nextest run --features integration-tests && cargo test --features integration-tests --doc && cargo test --features integration-tests --test engine --test session_e2e
```

After every frontend change:

```bash
cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:unit && npm run test:e2e:up && npm run test:e2e; npm run test:e2e:down
```

`test:e2e` runs against the stack `test:e2e:up` brings up (`README.md`, "Development", "End-to-end tests"): Postgres, the stub image and an orchestrator built with `--features integration-tests`, so it needs a container engine and one orchestrator build. The `E2E_*` port knobs keep it off a developer's own orchestrator on the same machine.

The two clippy invocations are what the Orchestrator CI workflow runs, so a lint in a test file or behind the `integration-tests` feature is caught locally.

The test runner is `cargo nextest` (ADR 0037; install it with `cargo install cargo-nextest --locked`, `README.md`, "Development"). It runs every test in a process of its own and schedules those processes across all the test binaries at once, which is the difference between a suite that is the sum of its binaries and one that is their maximum. Its configuration is `orchestrator/.config/nextest.toml`. Three consequences, all of them in that one command above:

- `cargo test --doc` runs beside it, because nextest does not run doctests.
- `tests/engine.rs` and `tests/session_e2e.rs` are excluded from the nextest run (`default-filter`) and run under `cargo test --test engine --test session_e2e`, one process per binary. They serialise their container work with in-process mutexes — rootless Podman cannot resolve `keep-id` for two containers at once — and process-per-test would take that away. They are the one part of the chain that needs `DOCKER_HOST`; with it unset they print a line each and pass, which is how Orchestrator CI sees them.
- A suite that serialises itself with a `static` is unserialised by nextest. Such a suite joins that exclusion or gets a nextest test group.

`tests/health.rs` still needs a container engine, as CI has: the first process of a run to want a database starts one `postgres:18` container and records it under `target/tmp/`, every other process of the same run clones its own database from the same server, and the last process out removes the container (`tests/common/db.rs`).

## Testing expectations

- **Backend integration tests** use `TestApp::spawn()` from `tests/common/mod.rs` (one testcontainers Postgres shared by every process of a run, one database per test cloned from a template the migrations were applied to once, seeded admin removed, mock engine, email and git credentials, fixed test master key). Passwords hash with a deliberately cheap Argon2 parameter set under the `integration-tests` feature (`models::user::hash_password`), so a suite that creates users is not mostly key derivation; verification takes its parameters from the stored hash and so is unchanged. One `#[tokio::test]` per scenario. Every new endpoint gets tests for the happy path and each error path (unauthenticated, forbidden, validation, conflict). Assert with `response.assert_status()` and `response.json::<T>()`. Invite flows are asserted through the mock email client's captured messages. The auth and users suites arrange and assert through HTTP or the `auth::Credentials` module — the `TestApp` helpers, not `UserRepository` or `UserInviteRepository` — and reach into raw SQL only for a row-level fact no interface answers, each with a one-line comment naming the fact.
- **Tracker tests** drive the verbs in `tracker/` through `TrackerMutation`; they never call the row helpers in `repositories/tasks/` directly. A rule the tracker composes — the lease and `attempts` moving with the state, a terminal move closing the task and unblocking its dependants, a session link written on commit — is asserted through the verb that composes it, and the lock and notification behaviour at the `TrackerMutation` seam (`tests/tracker_mutation.rs`). What stays at the repository is what has no verb: the cross-table scope rules of the edge, comment and hand-off inserts, and the reads. A precondition — a lease, an attempt count, a terminal state, a blocked flag, a current hand-off — is arranged through `tests/common/tracker.rs`, which reaches each of them with the verb that composes it, so `attempts` really is what three claims left behind and those claims really are in the event stream: a scenario asserting what its own call emitted reads from a cursor taken after its arrangement. The few row-level rules with no verb above them and no constraint below them — the state must belong to the project, the hand-off to the task, the event to a task of the project — are `#[cfg(test)]` unit tests beside the statement, where `pub(crate)` is reachable; they share the integration suites' Postgres through `repositories::testdb` and so need `DOCKER_HOST` like every other database test.
- **Engine tests** (`tests/engine.rs`) run only when `DOCKER_HOST` is set and only under their own `cargo test --test engine --test session_e2e` invocation ("Code quality"); CI runs them on both Podman and Docker. `tests/session_e2e.rs` is the session lifecycle on real containers under the same rule, over the stub image named by `MARS_STUB_IMAGE` (`README.md`, "Development"). The engine *contract* tests are the normalised semantics of `ARCHITECTURE.md`, "Engine adapter": one suite in `tests/common/engine_contract.rs` over an `Arc<dyn ContainerEngine>`, run against `MockEngine` on every run of the test suite (`tests/engine_mock.rs`, no engine needed) and against `BollardEngine` from `tests/engine.rs` with `DOCKER_HOST`. A new adapter, or a changed normalisation, changes the suite and both halves pass it.
- **Git tests** use real bare repositories in `tempfile` directories; git is never mocked.
- **Event translation tests** are fixture-based: recorded native output per pinned CLI version in `tests/fixtures/claude/<version>/` with the expected `AgentEvent` sequences beside them. A CLI version bump adds fixtures, never edits old ones.
- **MCP tests** drive the tool handlers through the `rmcp` server in-process with a session bearer token from `TestApp`.
- **Session owner tests** feed a transcript file line by line, kill and restart the owner mid-file, and assert `events` has no gaps and no duplicates.
- **Frontend unit tests** use Vitest; test files sit beside the module as `*.test.ts`.
- **Frontend E2E** (Playwright, `workers: 1`) runs against a real orchestrator started with `--features integration-tests`, creates fresh users per test through the test-only `/api/test/users` endpoint, and runs sessions on a stub image that replays a fixture transcript. A scenario arranges through the fixtures of `tests/utils/fixtures.ts` — `user`, `api`, `repo`, `project` and a `sessions` tracker that ends what it launched — and the helpers of `tests/utils/test-helpers.ts`; `frontend/tests/README.md` is the suite's document and carries the coverage table over `SPEC.md`, "User-facing features" and "Frontend", which `npm run test:e2e` checks before and after every run. A new scenario adds its row there; a new `data-testid` is a constant in `src/utils/testIds.ts`, re-exported from `tests/utils/test-ids.ts`. Nothing sleeps for a fixed period.

## Git workflow

- Feature branches from `main`; rebase before merging, no merge commits; `gh pr create`.
- Conventional Commits: `<type>(<scope>): <description>` with types `feat`, `fix`, `docs`, `chore`, `refactor`, `test`, `style`, `perf`, `ci`, `build` and scopes `orchestrator`, `frontend`, `images`, `infra`, `docs`.
- A PR that changes behaviour without touching the corresponding document is not mergeable (rule 1).
- Epics are implemented with the `implement-epic` skill (`.claude/skills/implement-epic/`): one `task-implementer` subagent (`.claude/agents/task-implementer.md`, Opus at medium effort) per task in its own worktree, tasks without unmet dependencies in parallel, each building in its own worktree and running fmt, clippy and the test binaries its task concerns; the coordinating session reviews and merges each branch, verifies each round with the full local quality chains, the only full-suite run there is, and pushes `main` once when the epic closes, so CI runs once per epic.
