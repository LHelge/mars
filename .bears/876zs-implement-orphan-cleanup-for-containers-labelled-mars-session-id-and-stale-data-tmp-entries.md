---
id: "876zs"
title: Implement orphan cleanup for containers labelled mars.session_id and stale /data/tmp entries
status: open
priority: P2
created: "2026-09-16T20:45:23.490527713Z"
updated: "2026-09-16T20:45:23.490527713Z"
tags:
  - orchestrator
  - cron
  - engine
depends_on:
  - yb2ny
parent: cxmar
---

## Summary
Fill in the first two sweeps of `CronService::orphan_cleanup`: remove containers carrying the `mars.session_id` label whose session is `parked`, `done`, `failed` or missing, and delete leftovers under `DATA_DIR/tmp` (temporary merge/rebase clones and startup-probe directories) older than one hour. The hand-off ref sweep is the next task; this task establishes the job method and the sweep-isolation structure it plugs into.

## Documents
- `ARCHITECTURE.md` "Background jobs" (orphan cleanup row: `1 h`, "Remove containers labelled `mars.session_id` whose session is `parked`/`done`/`failed`/missing; delete `/data/tmp` leftovers; ...")
- `ARCHITECTURE.md` "Storage" (`/data/tmp` "temporary clones for merge/rebase and the startup probe; emptied by orphan cleanup"), "Engine adapter" (list with label filter; "Container labels: `mars.session_id`, `mars.project_id`, `mars.profile_id`. Container name: `mars-session-<sid>`"; startup probe writes into `/data/tmp/probe-<random>/`), "Session lifecycle" (`parked`/`done`/`failed` have no container; `creating` and `running` do), "Restart procedure" (recovery does not remove stray containers; that is this job's work), "Git model" -> "Merge, rebase, push" (temp clones in `/data/tmp/`)
- `docs/data-model.md` `sessions` (`container_id` NULL once removed)
- `README.md` "Configuration" (`DATA_DIR`)

## Acceptance criteria
- [ ] `cron/orphan_cleanup.rs`: `impl CronService { pub async fn orphan_cleanup(&self, now: DateTime<Utc>) -> Result<JobReport> }` runs `cleanup_containers(now)`, `cleanup_tmp(now)` and (next task) `cleanup_handoff_refs(now)` in sequence; each sweep returns its own `JobReport`, a sweep error is logged `error!(sweep = "containers" | "tmp" | "handoff_refs", error = %e)` and counted as `failures += 1` without skipping the remaining sweeps; the reports are summed.
- [ ] `cleanup_containers`: `engine.list_by_label("mars.session_id")` once; for each container parse the label as `Uuid` (unparseable -> `warn!` and skip); look the session up with `SessionRepository::get`; remove the container (`engine.remove(id, force = true)`, `NotFound` ignored) when the session is missing or its state is `parked`, `done` or `failed`, **unless** the registry has an entry for that session (`registry.has(sid)`: a relaunch of a parked session between container creation and `running`) or the container was created less than 5 minutes before `now` (engine `created` timestamp); sessions in `creating` or `running` are never touched. After removing a container that equals `sessions.container_id`, set `container_id = NULL`. Each removal is logged `info!(session_id = %sid, container_id = %cid, "removed orphan container")`.
- [ ] `cleanup_tmp`: list `DATA_DIR/tmp` (missing directory -> nothing to do, `Ok`); for each entry whose modification time is older than `now - 1 h`, `remove_dir_all` (or `remove_file` for a plain file); permission or IO errors are logged per entry `warn!(path = %p, error = %e)` and counted as `failures`, the loop continues. Entries younger than one hour are left alone because a merge or rebase may be using them under a project git lock.
- [ ] Every engine or filesystem error is isolated to its item; the sweep's own `Err` is reserved for the listing call failing.
- [ ] `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Implementation notes
- Files: `orchestrator/src/cron/orphan_cleanup.rs`, `orchestrator/src/cron/mod.rs`, `orchestrator/src/session/registry.rs` (`has(sid) -> bool` if absent).
- The engine trait's list result must expose the container id, labels and created time; if `ContainerSummary` lacks `created`, add it to the trait and the mock in this task (both engines report it in the list endpoint; note it in the engine table only if a new API field is used).
- Use `std::fs::metadata(..).modified()` for tmp ages; `now` converts to `SystemTime` for the comparison so tests can pass a future `now` instead of rewriting mtimes.
- The container age guard and the registry guard together cover the launch window: the launcher registers the owner before creating the container and the session leaves `parked` at `init`; a container older than 5 minutes for a `parked` session with no owner is a leftover.

## Edge cases
- Two containers for one session (crash between create and remove on a resume): both match the rule and both are removed when the session is parked; a running session's containers are never touched, recovery handles that case.
- `container_id` pointing at a container the engine no longer lists: not this sweep's concern; the launcher overwrites it on the next launch.
- Startup probe directories `probe-<random>` are removed by the probe itself on success; this sweep catches the failure case after one hour.
- `DATA_DIR/tmp` containing a symlink: `remove_file` on the link, never follow it.
- Engine unreachable: `cleanup_containers` returns `Err`, `cleanup_tmp` still runs.

## Testing
- Integration test `orchestrator/tests/cron_orphan_cleanup.rs` via `TestApp` with the mock engine's seeded container list and a `tempfile` `DATA_DIR`: containers labelled for (a) a missing session, (b) a `parked` session, (c) a `done` session, (d) a `failed` session, (e) a `running` session, (f) a `creating` session, (g) a `parked` session with a registry entry, (h) a `parked` session whose container was created 2 minutes ago, (i) an unparseable label; all others created 10 minutes ago. Run `app.cron().orphan_cleanup(now)`: exactly (a), (b), (c), (d) removed, `container_id` cleared on (b)–(d), report `items = 4`; tmp directory with `old-clone/` (mtime 2 h ago via a future `now`), `fresh-clone/` (just created), `stale.file`: only `old-clone/` and `stale.file` removed. Engine list failure (mock returns `Err`): `failures = 1` and the tmp sweep still ran. Missing tmp directory: `Ok`.
- Command: `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- `ARCHITECTURE.md` "Background jobs" orphan cleanup row: add "(containers younger than 5 minutes and sessions with a live owner are skipped; `/data/tmp` entries older than one hour)" so the guards are part of the contract.

## Assumes from other epics
- "Container engine adapter": `ContainerEngine::list_by_label`, `remove`, container summaries with labels and created time, mock engine seeding and `Err` injection.
- "Session lifecycle: launcher, owner, recovery and sessions API": `SessionRegistry` entries during launch, `SessionRepository::get`/`set_container_id`.