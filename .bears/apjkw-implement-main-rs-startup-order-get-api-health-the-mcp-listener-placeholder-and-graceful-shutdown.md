---
id: apjkw
title: Implement main.rs startup order, GET /api/health, the MCP listener placeholder and graceful shutdown
status: open
priority: P0
created: "2026-09-16T20:29:02.388775972Z"
updated: "2026-09-16T20:29:02.388775972Z"
tags:
  - orchestrator
  - core
depends_on:
  - au4vs
parent: sywed
---

## Summary
Turn the crate into a running orchestrator: `main.rs` follows the documented startup order (config, tracing, pool, migrations, listeners), `routes/health.rs` serves `GET /api/health` with the documented body and status, a second listener on `MCP_PORT` is bound with an empty router as the placeholder the MCP epic replaces, and SIGINT/SIGTERM shut both listeners down gracefully so `cargo run` exits cleanly. `lib.rs` exposes `build_api_router(AppState)` and `run(Config)` so tests and `main` share one code path.

## Documents
- `ARCHITECTURE.md` "Orchestrator internals": `main.rs` "config, pool, migrations, listeners, recovery, spawn services"; `routes/` "axum routers, one module per resource, nested under /api"; `tower-http` (`trace`, `cors`).
- `ARCHITECTURE.md` "Durability and recovery", "Restart procedure": step 1 "Runs migrations"; steps 2 to 4 (adoption, marking `creating` sessions failed, cron start) belong to the Session lifecycle and Background jobs epics; leave clearly named hook points.
- `ARCHITECTURE.md` "MCP design": own listener on `MCP_PORT` (default 7001), path `/mcp`, binds on all interfaces.
- `SPEC.md` "Health": `GET /api/health` → `{ orchestrator: true, database: bool, engine: bool }` with 200 or 503. Unauthenticated, for compose health checks.
- `SPEC.md` "REST API": all routes under `/api`; bare JSON.
- `README.md` "Development", "Orchestrator": `cargo run  # runs migrations, listens on API_PORT and MCP_PORT`; "Configuration": `API_PORT` default 7000, `MCP_PORT` default 7001.
- `CLAUDE.md` "Backend conventions": routes export `routes() -> Router<AppState>`, DTOs private to the module; `unwrap`/`expect` only at startup.

## Acceptance criteria
- [ ] `src/main.rs`: `Config::from_env()` (on error print `error: <ConfigError Display>` to stderr and exit 1 — tracing is not yet initialised), `init_tracing(&config.rust_log)`, `PgPoolOptions::new().max_connections(20).connect(&config.database_url)` with a 10 s acquire timeout, `sqlx::migrate!("./migrations").run(&pool)` (empty directory is fine now; the Database epic fills it), build `AppState`, then `mars_orchestrator::run(state, shutdown_signal())`.
- [ ] `src/lib.rs`: `pub fn build_api_router(state: AppState) -> Router` nesting `Router::new().nest("/api", routes::routes())` with `TraceLayer::new_for_http()` (from `tower-http`) whose span includes method and path but **not** the query string (tokens travel in `?token=` on stream endpoints), and `pub async fn run(state, shutdown: impl Future<Output = ()>) -> Result<()>` binding `0.0.0.0:{api_port}` and `0.0.0.0:{mcp_port}` with `tokio::net::TcpListener`, serving both with `axum::serve(...).with_graceful_shutdown(...)`, and returning when both have stopped.
- [ ] `src/routes/mod.rs`: `pub fn routes() -> Router<AppState>` merging each resource module's `routes()`; this task adds only `pub mod health;`. `src/routes/health.rs`: `pub fn routes() -> Router<AppState>` with `.route("/health", get(health))`; handler runs `sqlx::query("SELECT 1").execute(&pool)` with a 2 s timeout to set `database`, sets `engine` from `state.engine_ready().await` (see notes), and answers 200 when both are true, else 503, always with the JSON body `{ "orchestrator": true, "database": <bool>, "engine": <bool> }`. The health query uses the unchecked `sqlx::query` so no `.sqlx/` offline data is needed in this epic.
- [ ] MCP placeholder: `src/mcp/mod.rs` gets `pub fn placeholder_router() -> Router` returning 404 for everything, served on `MCP_PORT`; a doc comment says the MCP epic replaces it with the `rmcp` Streamable HTTP service at `/mcp`.
- [ ] `shutdown_signal()` in `main.rs` resolves on `SIGINT` or `SIGTERM` (`tokio::signal::unix`), logs `info!("shutdown signal received")`, and the process exits 0 within `STOP_GRACE_SECS` (open connections are given until then; a hard exit after that is acceptable and logged at `warn`).
- [ ] `tower-http` `CorsLayer` is **not** added (same-origin behind nginx; `README.md` says nothing about CORS) — leave a note in `lib.rs`.
- [ ] Every log line at startup uses structured fields (`api_port = config.api_port`, `mcp_port = ...`); `database_url`, `jwt_secret` and master keys are never logged (the redacting `Debug` from the config task makes `?config` safe, but prefer explicit fields).
- [ ] `cd orchestrator && cargo fmt && cargo clippy --all-targets --features integration-tests -- -D warnings && cargo test --features integration-tests` pass, and a manual `cargo run` with the README Postgres container answers `curl -s localhost:7000/api/health` and exits 0 on Ctrl-C.

## Implementation notes
- Files: `orchestrator/src/main.rs`, `orchestrator/src/lib.rs`, `orchestrator/src/routes/mod.rs`, `orchestrator/src/routes/health.rs`, `orchestrator/src/mcp/mod.rs`, `orchestrator/tests/health.rs`.
- `engine` in the health body: the `ContainerEngine` trait does not exist yet (Container engine epic). Give `AppState` a method `pub async fn engine_ready(&self) -> bool` in `prelude/state.rs` that returns `true` in this epic with a doc comment `// Container engine epic: replace with self.engine.ping().await.is_ok()`. This keeps the health contract's shape and status rule intact while making the skeleton report healthy; note the judgment call in the PR.
- Health DTO is a private `#[derive(Serialize)] struct HealthResponse { orchestrator: bool, database: bool, engine: bool }` inside `routes/health.rs`.
- Startup hooks for later epics: after migrations and before `run`, leave three commented, named call sites in order — `// Container engine epic: ensure networks, startup probe` — `// Session lifecycle epic: adopt running containers, fail sessions in creating` — `// Background jobs epic: CronService::start`. Comments, not TODO lists.
- `sqlx::migrate!` with a path resolves relative to `CARGO_MANIFEST_DIR`; keep the `migrations/.gitkeep` from the skeleton task so the macro finds the directory.
- The API listener binds `0.0.0.0` because nginx reaches it over the compose network; the MCP listener likewise per `ARCHITECTURE.md` "Networks".

## Edge cases
- Database unreachable at startup: `connect` fails; log `error!(error = %e, "database connection failed")` and exit 1 (fail fast, no retry loop in v1).
- Port already in use: bind error is fatal with the port in the log line.
- Health when the pool is exhausted or the query times out: `database: false`, 503, never a 500 body.
- `SIGTERM` during migrations: let the migration finish (sqlx migrations are transactional per file), then exit.

## Testing
- `tests/health.rs` (integration, uses `testcontainers-modules::postgres` directly since `TestApp` does not exist yet; keep it self-contained and small so the Database epic can delete it when `TestApp` lands): start Postgres 18, build `AppState` with a real pool and a `Config` built through `Config::from_vars` with fake values, `axum_test::TestServer::new(build_api_router(state))`, `GET /api/health` → 200 and body `{"orchestrator":true,"database":true,"engine":true}`.
- Unit test in `routes/health.rs` with `PgPool::connect_lazy("postgres://invalid@127.0.0.1:1/x")`: `GET /api/health` → 503 with `database: false` (the lazy pool never connects), `orchestrator: true`.
- Shutdown test in `tests/shutdown.rs`: call `run(state, oneshot_rx)` on ports 0 (make `run` accept the bound listeners or a `0` port and expose the chosen addresses) and assert it returns `Ok(())` within 2 s after the oneshot fires.
- Command: `cd orchestrator && cargo fmt && cargo clippy --all-targets --features integration-tests -- -D warnings && cargo test --features integration-tests`.

## Documentation
- none: implements `SPEC.md` "Health" and the `main.rs` order as written. If `run` needs a port-0 test hook, it is internal and needs no document change.

## Assumes from other epics
- Container engine epic replaces `AppState::engine_ready` with the real `ContainerEngine` check and adds network creation and the startup probe at the named hook.
- Database epic adds the migrations, `.sqlx/` and `TestApp`, and may retire `tests/health.rs` into the shared harness.
- MCP epic replaces `mcp::placeholder_router` with the `rmcp` service.
- Session lifecycle and Background jobs epics fill the remaining startup hooks.