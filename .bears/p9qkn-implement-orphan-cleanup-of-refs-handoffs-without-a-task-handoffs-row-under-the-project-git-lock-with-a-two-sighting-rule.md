---
id: p9qkn
title: Implement orphan cleanup of refs/handoffs/* without a task_handoffs row under the project git lock with a two-sighting rule
status: done
priority: P2
created: "2026-09-16T20:46:35.252601653Z"
updated: "2026-09-20T07:00:34.258341680Z"
tags:
  - orchestrator
  - cron
  - git
  - tracker
depends_on:
  - "876zs"
parent: cxmar
attempts: 1
---

## Summary
Add the third sweep of `CronService::orphan_cleanup`: for every project with a repository on disk, take the project git lock, list `refs/handoffs/*`, and delete refs whose id has no `task_handoffs` row. A ref is deleted only when it has been seen orphaned on two consecutive runs, so a publication that pinned its ref but has not yet committed its database row is never destroyed by the sweep. This closes the gap left by a crash between ref creation and the tracker transaction, and by interrupted task or project deletion.

## Documents
- `ARCHITECTURE.md` "Background jobs" (orphan cleanup row: "under each project's git lock, remove `refs/handoffs/*` with no matching hand-off row")
- `ARCHITECTURE.md` "Task tracker" -> "Code hand-offs" ("A crash before the database commit may leave an unreferenced hand-off ref ... The orphan cleanup job removes hand-off refs without matching database rows while holding the project git lock. Committed hand-off refs survive session deletion and remain until their task or project is deleted"), "Git model" -> "Ref ownership" (`refs/handoffs/<id>` is never a mutation target or push source), "Serialization" (git lock before any database lock; task deletion also acquires this lock before removing hand-off refs; a transaction holding the project row must never wait for the git lock)
- `docs/data-model.md` `task_handoffs` ("Task/project deletion removes their hand-off refs under the project git lock; cleanup retries remove orphan refs left by interrupted deletion or failed publication"), `tasks.current_handoff_id`
- ADR 0018

## Acceptance criteria
- [ ] `TaskHandoffRepository::existing_ids(project_id: Uuid, ids: &[Uuid]) -> Result<Vec<Uuid>>` runs `SELECT h.id FROM task_handoffs h JOIN tasks t ON t.id = h.task_id WHERE t.project_id = $1 AND h.id = ANY($2)` outside any transaction.
- [ ] `cleanup_handoff_refs(&self, now)` in `cron/orphan_cleanup.rs`: list every project id (all statuses) whose `DATA_DIR/projects/<id>/repo.git` exists; for each, acquire the project git lock (`ProjectGitLocks::lock(project_id).await`), call `git::refs::list_handoffs(mirror) -> Vec<(Uuid, String)>`, release nothing yet, query `existing_ids`, compute `orphans = listed - existing`; for each orphan look it up in the service's in-memory `HashMap<(Uuid, Uuid), DateTime<Utc>>` of first sightings: if absent, record `now` (`skipped += 1`); if present and `now - first_seen >= 1 h`, `git::refs::remove_handoff(mirror, id)` (`items += 1`, `info!(project_id = %pid, handoff_id = %hid, "removed orphan hand-off ref")`) and forget the entry; refs that are no longer orphaned (a row appeared) are forgotten. The git lock is released after the project's refs are processed.
- [ ] Sightings are kept on `CronService` behind a `tokio::sync::Mutex` (jobs borrow `&self`); entries for projects that no longer exist are dropped at the end of each run. The map is not persisted: after a restart an orphan simply needs two more sightings.
- [ ] Per-project errors (repository directory vanished after listing, git command failure) are logged `warn!(project_id = %pid, error = %e)` and counted as `failures += 1`; the loop continues. The sweep never opens a database transaction while holding the git lock; `existing_ids` is a single autocommit read.
- [ ] `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes; `.sqlx/` refreshed.

## Implementation notes
- Files: `orchestrator/src/cron/orphan_cleanup.rs` (extend), `orchestrator/src/cron/mod.rs` (`sightings` field on `CronService`), `orchestrator/src/repositories/task_handoff.rs` (query), `orchestrator/.sqlx/`.
- Reading the ref list under the git lock guarantees no publication is between "pin ref" and "release lock" at that instant; the two-sighting rule covers a publication whose tracker transaction commits after the lock is released. Both together make the sweep safe whether or not the hand-offs epic holds the git lock through the database commit.
- Never call `remove_handoff` for an id that `existing_ids` returned, even if the sighting map says otherwise: recompute against the fresh query on every run.
- Project deletion holds the same git lock while removing the mirror; if the sweep acquires the lock afterwards, `list_handoffs` fails on the missing directory -> `warn` and continue.

## Edge cases
- A task deleted between listing and `existing_ids`: its refs are first-sighted now and removed next run, which is the intended retry of the deletion's own cleanup.
- Hand-off row whose task moved project: impossible (tasks never change project).
- `refs/handoffs/<name>` where `<name>` is not a UUID: `list_handoffs` skips it with a `warn`; the sweep never deletes names it cannot parse.
- Thousands of refs: `existing_ids` takes the full id list in one `ANY($2)`; acceptable at v1 scale.
- Sightings map growth: bounded by the number of orphan refs, which the sweep itself shrinks.

## Testing
- Extend `orchestrator/tests/cron_orphan_cleanup.rs` via `TestApp` with a real bare repository per project (git epic test helper): project A with a task, a comment and a `task_handoffs` row pinned at `refs/handoffs/<id-a>`; also pin `refs/handoffs/<id-b>` for a row that is then deleted and `refs/handoffs/<id-c>` for an id that never existed; project B with one valid hand-off ref. First run at `now`: no ref removed, `skipped = 2`; second run at `now + 30 min`: still nothing removed; insert a hand-off row for `id-c` (new task) and run at `now + 61 min`: `id-b` removed, `id-c` and `id-a` and project B's ref remain, `items = 1`; run again: `JobReport::default()` for this sweep. Lock respect: hold project A's git lock in the test while calling the sweep with a 200 ms `tokio::time::timeout` and assert it has not completed, then release and assert completion. Missing repository directory for a project row: `failures = 1`, other projects processed.
- Command: `cd orchestrator && cargo sqlx prepare && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- `ARCHITECTURE.md` "Background jobs" orphan cleanup row and "Task tracker" -> "Code hand-offs": state that a hand-off ref is removed only after being found without a row on two runs at least one hour apart, so a publication in flight is never affected.

## Assumes from other epics
- "Git operations: mirror, clones, integration and REST API": `ProjectGitLocks`, `git::refs::list_handoffs`, `git::refs::remove_handoff` (idempotent), test helpers creating bare repositories.
- "Code hand-offs and review": `task_handoffs` rows and the repository module; ref pinning at publication.
- "Projects, agent profiles and shared directories": project listing and the data-directory layout helper giving the mirror path.