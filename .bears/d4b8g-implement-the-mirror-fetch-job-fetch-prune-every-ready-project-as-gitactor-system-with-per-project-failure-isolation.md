---
id: d4b8g
title: "Implement the mirror fetch job: fetch --prune every ready project as GitActor::System with per-project failure isolation"
status: open
priority: P1
created: "2026-09-16T20:43:25.346399139Z"
updated: "2026-09-16T20:43:25.346399139Z"
tags:
  - orchestrator
  - cron
  - git
  - projects
depends_on:
  - yb2ny
parent: cxmar
---

## Summary
Fill in `CronService::mirror_fetch`: every `MIRROR_FETCH_INTERVAL_SECS` iterate the projects in status `ready` and run the git epic's `fetch_project` for each as `GitActor::System`, updating `last_fetched_at`, so upstream-tracking refs stay fresh without touching integration heads or session refs. One unreachable upstream must not stop the other projects from being fetched.

## Documents
- `ARCHITECTURE.md` "Background jobs" (mirror fetch row: `10 min`, "`git fetch --prune` on every `ready` mirror")
- `ARCHITECTURE.md` "Git model" -> "Project clone" ("A cron job runs `git fetch --prune origin` on every ready mirror every `MIRROR_FETCH_INTERVAL_SECS` (default 600) and updates `last_fetched_at`"; subsequent fetches update or prune only upstream-tracking refs and tags), "Ref ownership", "Serialization" (the per-project git lock covers upstream fetch; cron, REST and MCP operations are serialised by it)
- `README.md` "Configuration" (`MIRROR_FETCH_INTERVAL_SECS`), "Operating notes" (a background fetch cannot discard a merge waiting to be pushed)
- `docs/data-model.md` `projects.last_fetched_at` ("Updated by the periodic mirror fetch"), `project_status`, `secret_uses` ("the mirror-fetch job sets neither" `session_id` nor `user_id`)
- ADR 0017

## Acceptance criteria
- [ ] `cron/mirror_fetch.rs`: `impl CronService { pub async fn mirror_fetch(&self, now: DateTime<Utc>) -> Result<JobReport> }` lists project ids with `status = 'ready'` through `ProjectRepository::list_ids_by_status(ProjectStatus::Ready)` (add the query if the projects epic did not: `SELECT id FROM projects WHERE status = $1 ORDER BY id`), and for each calls `git::mirror::fetch_project(&self.state, project_id, &GitActor::System, None).await` sequentially.
- [ ] A successful fetch counts as `items += 1`; `Error::Conflict` (project no longer `ready`) or `Error::NotFound` (project deleted between listing and fetching) counts as `skipped += 1` at `debug`; any other error counts as `failures += 1`, is logged `warn!(project_id = %id, error = %e, "mirror fetch failed")`, and the loop continues with the next project.
- [ ] The job never holds a database transaction while git runs and never touches `refs/heads/*`, `refs/sessions/*` or `refs/handoffs/*` (it only calls the routine that already guarantees this).
- [ ] `now` is accepted for signature uniformity and passed through as the log timestamp only; freshness is decided by `fetch_project` (`max_age = None`: the cron job always fetches).
- [ ] The mock `GitCredentialProvider` records `GitActor::System` for every cron fetch, and no `secret_uses` row from the job carries a `session_id` or `user_id`.
- [ ] `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes (with `cargo sqlx prepare` rerun if a query was added).

## Implementation notes
- Files: `orchestrator/src/cron/mirror_fetch.rs`, `orchestrator/src/cron/mod.rs` (`mod mirror_fetch;`), `orchestrator/src/repositories/project.rs` (query, if missing), `orchestrator/.sqlx/`.
- Sequential, not `join_all`: each fetch acquires that project's git lock and one credential lookup; parallel fetches of many projects would hammer upstream and the credential provider for no gain. Log `elapsed_ms` per project at `debug`.
- The scheduler's period already comes from `Config.mirror_fetch_interval_secs` (scheduler task); nothing to configure here.

## Edge cases
- Project deleted while its fetch waits on the git lock: the mirror directory is gone when the lock is acquired; `fetch_project` fails with a git error -> `failures += 1`; the next tick no longer lists the project.
- Project moved to `error` by a retry-clone between listing and fetching: `Conflict` -> skipped.
- Upstream credential missing (`GIT_CREDENTIAL` secret deleted): the credential provider's error is a failure for that project only; logged without any credential value (rule 3).
- Zero ready projects: `Ok(JobReport::default())`, logged at `debug` by the scheduler.
- A single very slow upstream delays the others within the tick; there is no per-project timeout in v1 (the git wrapper owns process timeouts, if any).

## Testing
- Integration test `orchestrator/tests/cron_mirror_fetch.rs` via `TestApp` with real bare repositories in `tempfile` directories: two `ready` projects initialised through the git epic's `init_project_repo`, one `cloning` and one `error` project; advance upstream `main` on both ready repos; run `app.cron().mirror_fetch(Utc::now())`; assert `items == 2`, `refs/remotes/origin/main` updated in both mirrors, `refs/heads/main` unchanged, `last_fetched_at` set on the two ready rows and null on the others, the mock credential provider recorded exactly two `GitActor::System` calls. Then delete one upstream directory and rerun: `items == 1`, `failures == 1`, the healthy project still fetched and its `last_fetched_at` advanced. With no projects: `JobReport::default()`.
- Command: `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Git operations: mirror, clones, integration and REST API": `git::mirror::fetch_project(state, project_id, actor, max_age)`, `GitActor::System`, `init_project_repo` for tests, mock `GitCredentialProvider` recording actors.
- "Projects, agent profiles and shared directories": `ProjectRepository` with a status-filtered listing and `touch_last_fetched`.