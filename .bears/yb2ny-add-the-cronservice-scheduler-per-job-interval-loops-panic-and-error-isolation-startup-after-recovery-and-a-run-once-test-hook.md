---
id: yb2ny
title: "Add the CronService scheduler: per-job interval loops, panic and error isolation, startup after recovery and a run-once test hook"
status: done
priority: P0
created: "2026-09-16T20:42:23.411464510Z"
updated: "2026-09-20T05:02:28.946821522Z"
tags:
  - orchestrator
  - cron
  - core
depends_on:
  - xjaah
parent: cxmar
attempts: 1
---

## Summary
Deliver `orchestrator/src/cron/`: one `CronService` holding `AppState`, a named async method per job, and a generic scheduler loop that runs each job on its own interval, never lets a panic or error in one job stop the process or the other jobs, logs every outcome, and starts from `main.rs` after recovery. Later tasks in this epic fill in the job bodies; this task fixes their signatures, the `JobReport` shape, the intervals and the way integration tests invoke a job directly with a controlled `now`.

## Documents
- `ARCHITECTURE.md` "Background jobs" (one cron service with independent intervals; job table with mirror fetch `10 min`, idle reaper `1 min`, stuck-task reaper `1 min`, token cleanup `1 h`, secret rotation `1 h`, orphan cleanup `1 h`; "Every job logs its outcome and never panics the process; a failing job is retried at its next interval")
- `ARCHITECTURE.md` "Orchestrator internals" (module tree: `cron/  periodic jobs`; "**Cron jobs** are each a method on `CronService`"; `main.rs` "config, pool, migrations, listeners, recovery, spawn services"; crate table is binding, `tokio` `full`)
- `ARCHITECTURE.md` "Restart procedure" step 4 ("Starts the cron jobs")
- `README.md` "Configuration" (`MIRROR_FETCH_INTERVAL_SECS`, default 600)
- `CLAUDE.md` "Backend conventions" (functions return `Result`; `tracing` with structured fields; `unwrap`/`expect` only at startup)

## Acceptance criteria
- [ ] `orchestrator/src/cron/mod.rs` defines `pub struct CronService { state: AppState }`, `CronService::new(state: AppState) -> Self`, and `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum JobName { MirrorFetch, IdleReaper, StuckTaskReaper, TokenCleanup, SecretRotation, OrphanCleanup }` with `Display` giving `mirror_fetch`, `idle_reaper`, `stuck_task_reaper`, `token_cleanup`, `secret_rotation`, `orphan_cleanup`, and `JobName::period(&self, config: &Config) -> Duration` returning `config.mirror_fetch_interval_secs` seconds for `MirrorFetch`, 60 s for `IdleReaper` and `StuckTaskReaper`, 3600 s for the other three.
- [ ] `#[derive(Debug, Default, Clone, PartialEq, Eq)] pub struct JobReport { pub items: u64, pub skipped: u64, pub failures: u64 }` is the common outcome type; every job method has the signature `pub async fn <job>(&self, now: DateTime<Utc>) -> Result<JobReport>` and `pub async fn run_once(&self, job: JobName, now: DateTime<Utc>) -> Result<JobReport>` dispatches by name. In this task each job method exists and returns `Ok(JobReport::default())` with a doc comment naming the task in this epic that implements it; no job body is written here.
- [ ] `cron/scheduler.rs` provides the generic loop `pub fn spawn_job<F, Fut>(name: &'static str, period: Duration, mut shutdown: watch::Receiver<bool>, job: F) -> JoinHandle<()> where F: Fn() -> Fut + Send + Sync + 'static, Fut: Future<Output = Result<JobReport>> + Send + 'static`: it uses `tokio::time::interval(period)` with `MissedTickBehavior::Delay`, so the first tick fires immediately after start and a tick never overlaps the same job's previous run; each tick runs `tokio::spawn(job())` and awaits the `JoinHandle`, so a panic surfaces as `JoinError::is_panic()` and is logged with `tracing::error!(job = name, "job panicked")` instead of unwinding the loop; an `Err(e)` is logged `tracing::error!(job = name, error = %e, "job failed")`; success is logged with `elapsed_ms`, `items`, `skipped`, `failures` at `info` when `items + failures > 0` and at `debug` otherwise. The loop exits when `shutdown` becomes `true`, letting an in-flight tick finish first.
- [ ] `CronService::start(self: Arc<Self>, shutdown: watch::Receiver<bool>) -> Vec<JoinHandle<()>>` spawns one `spawn_job` per `JobName` (period from `state.config`), each closure calling `self.run_once(name, Utc::now())`.
- [ ] `main.rs` calls `CronService::new(state.clone()).start(shutdown_rx)` at the `// Background jobs epic: CronService::start` hook, after `recover` and before the listeners accept traffic; `run` waits for the job handles after the listeners stop, bounded by `STOP_GRACE_SECS`, and logs `warn!` if a job did not finish in time.
- [ ] `TestApp` exposes `pub fn cron(&self) -> CronService` (a `CronService` over the test `AppState`) so integration tests in later tasks call `app.cron().idle_reaper(now).await` directly; `CronService::start` is never called by `TestApp::spawn()`, so tests are not raced by background ticks.
- [ ] `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Implementation notes
- Files: `orchestrator/src/cron/mod.rs`, `orchestrator/src/cron/scheduler.rs`, `orchestrator/src/lib.rs` (`pub mod cron;`), `orchestrator/src/main.rs` (start after recovery), `orchestrator/tests/common/mod.rs` (`cron()` accessor).
- `use crate::prelude::*` in every cron module. `Result` is the prelude alias; the scheduler never converts errors, it only logs them.
- Shutdown uses `tokio::sync::watch<bool>` shared with the listener shutdown in `run` (the scaffolding epic's `run(state, shutdown)` already has a shutdown future; derive a `watch` sender from it or pass the same receiver). `tokio_util::CancellationToken` is not added: it is not in the crate table.
- Log fields are structured: `job = %name`, `elapsed_ms = elapsed.as_millis() as u64`, `items = report.items`, never a formatted string of ids. Job bodies added later log per-item lines with `session_id = %id`, `project_id = %id` and so on.
- Keep `CronService` clonable-by-`Arc` only; jobs borrow `&self` so each job method can be unit-called without the scheduler.
- Job methods added by later tasks live in their own files (`cron/mirror_fetch.rs`, `cron/idle_reaper.rs` delegating to `session/idle_reaper.rs`, `cron/stuck_tasks.rs`, `cron/token_cleanup.rs`, `cron/secret_rotation.rs`, `cron/orphan_cleanup.rs`) as `impl CronService` blocks; reserve those module names in `mod.rs` comments, not as empty files.

## Edge cases
- A job that runs longer than its period: `MissedTickBehavior::Delay` schedules the next tick one full period after the late tick fires, so runs never pile up.
- Shutdown arriving mid-tick: the in-flight job completes (jobs are short and transactional) and the loop then returns; `run` bounds the wait with `STOP_GRACE_SECS`.
- `watch` sender dropped without sending `true`: treat `changed()` returning `Err` as shutdown.
- A panic inside a job must not poison shared state: jobs hold no locks across `.await` points that a panic could leave held, and every database work in later tasks is transactional.

## Testing
- Unit tests in `cron/scheduler.rs` with `tokio::time::pause()` and a 10 ms period: a job whose first call panics, second returns `Err`, third returns `Ok(JobReport { items: 1, .. })` is invoked at least three times (assert through an `Arc<AtomicUsize>`), while a second job spawned alongside keeps incrementing its own counter throughout; sending `true` on the `watch` stops both loops and the `JoinHandle`s resolve within one period; the first tick fires without waiting a full period.
- Integration test in `orchestrator/tests/cron.rs` via `TestApp`: `app.cron().run_once(JobName::TokenCleanup, Utc::now())` returns `Ok`; every `JobName` maps to the documented `Display` string and period (`MirrorFetch` follows `Config.mirror_fetch_interval_secs` when `TestApp` sets it to a non-default value).
- Shutdown test extension in `orchestrator/tests/shutdown.rs` (scaffolding epic): with cron started, `run` still returns within 2 s after the signal.
- Command: `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- `ARCHITECTURE.md` "Orchestrator internals": the `cron/` line in the module tree lists only four jobs; change it to "periodic jobs: mirror fetch, idle reaper, stuck-task reaper, token cleanup, secret rotation, orphan cleanup".
- `ARCHITECTURE.md` "Restart procedure" step 4: list all six jobs the same way.
- `ARCHITECTURE.md` "Background jobs": add one sentence after the table: "Every job runs once when the service starts, after recovery, then at its interval; a tick never overlaps the same job's previous run. Outcomes with no work are logged at `debug`, everything else at `info`."

## Assumes from other epics
- "Repository scaffolding, tooling and CI": `main.rs` startup order with the named `// Background jobs epic: CronService::start` hook, `run(state, shutdown)`, `Config.mirror_fetch_interval_secs`, `Config.stop_grace_secs`.
- "Database schema, models, repositories and test harness": `TestApp::spawn()` and its `AppState`.
- "Session lifecycle: launcher, owner, recovery and sessions API": `recover(&state)` is called before `CronService::start`.