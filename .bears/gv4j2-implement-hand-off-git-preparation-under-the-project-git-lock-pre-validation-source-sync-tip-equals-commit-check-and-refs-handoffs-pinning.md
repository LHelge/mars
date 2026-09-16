---
id: gv4j2
title: "Implement hand-off git preparation under the project git lock: pre-validation, source sync, tip-equals-commit check and refs/handoffs pinning"
status: open
priority: P1
created: "2026-09-16T20:41:10.624148345Z"
updated: "2026-09-16T20:41:10.624148345Z"
tags:
  - orchestrator
  - tracker
  - git
depends_on:
  - tjhbr
parent: xjaah
---

## Summary
Implement the first half of the publication protocol: with the project git lock held, read the task and validate state, holder and current hand-off, then for a revision silently sync the source session branch, require the fetched tip to equal the requested commit and pin it at `refs/handoffs/<new id>`; for a forward, verify the current hand-off id and pin a new ref at the same commit without syncing. The result is a `PreparedHandoff` the tracker transaction (next task) consumes; nothing in the database changes here.

## Documents
- `ARCHITECTURE.md` "Task tracker" -> "Code hand-offs" (sync the session branch, require the fetched tip to match, pin at `refs/handoffs/<new id>` before the state change; never create a commit or include uncommitted files; forwarding reuses source session, branch and commit and creates a new ref at the same commit without syncing or requiring the original session to still exist; "Git and Postgres cannot share a transaction ... Under the project git lock, publication first validates the task's state, holder and current hand-off, then syncs and creates the immutable ref"; a crash before the database commit may leave an unreferenced ref).
- `ARCHITECTURE.md` "Git model" -> "Ref ownership" (`refs/handoffs/<id>` never a mutation target or push source), "Fetch-back", "Serialization" (composite operations acquire the lock once; git lock before the database project lock; a transaction holding the project row must never wait for the git lock).
- `SPEC.md` "Code hand-offs and review" (sync must produce that exact tip or return 409 / MCP `conflict`; forwarding requires `handoff_id` to equal the current hand-off, 409 / `conflict` otherwise; sync failure leaves the task and lease unchanged).
- `docs/data-model.md` `task_handoffs` (`source_branch` is a snapshot of `session/<id>`; `commit` validated as a commit in the project repository).
- ADR 0018.

## Acceptance criteria
- [ ] `tracker::handoffs::prepare(state: &AppState, guard: &ProjectGitGuard, project_id, task: &Task, current_state: &TaskState, target_state: &TaskState, validated: &ValidatedHandoff, caller: &HandoffCaller) -> Result<PreparedHandoff>` where `PreparedHandoff { id: Uuid, task_id, source_session_id: Option<Uuid>, source_branch: String, commit: String, comment: String, previous_handoff_id: Option<Uuid>, review: ReviewCarry, ref_name: String }` and `ReviewCarry::{ Fresh /* unreviewed */, Decision(ReviewDecision), CarriedFrom(TaskHandoff) }`.
- [ ] Pre-validation (plain reads, no row locks): `HandoffCaller::Session` must hold the lease (`task.lease_holder_session_id == Some(caller)`), otherwise `Error::Conflict("task is not held by the calling session")`; `HandoffCaller::User` is not lease-bound. `target_state` must differ from `current_state` (`TaskError::HandoffRequiresStateChange`, already checked by the route but re-checked here).
- [ ] Revision: the source session is loaded with `SessionRepository::get(project_id, source_session_id)`; a session of another project or unknown -> `Error::BadRequest("source_session_id must name a session of this project")`. Then `GitService::sync_session_silent(guard, project_id, source_session_id)` (skipping the sync when the work directory is gone but `refs/sessions/<sid>` exists, per the GitService rule), then `git::refs::resolve(mirror, GitRef::Session(sid))`; if the resolved commit differs from `validated.commit` -> `Error::Conflict(format!("session branch tip {tip} does not match commit {commit}"))` and no ref is created. `source_branch` is `session.branch` (`session/<sid>`). A session that has no work tree and no session ref (state `creating`) -> `Error::BadRequest("session has no synced branch yet")`.
- [ ] Forward: `task.current_handoff_id` must equal `validated.handoff_id`, otherwise `Error::Conflict("handoff_id is not the task's current hand-off")` (including when the task has no current hand-off). The current row is loaded with `TaskRepository::find_handoff(project_id, id)`; `source_session_id`, `source_branch`, `commit` are copied; `review` is `Decision(d)` when supplied else `CarriedFrom(current)`. No sync and no session lookup.
- [ ] Pinning: `git::refs::retain_handoff(mirror, new_id, commit)` for both kinds; `new_id = Uuid::new_v4()` generated here so the ref name and the row id match. `retain_handoff` failing (commit not a commit object) -> `Error::Conflict("commit is not present in the project repository")`.
- [ ] `tracker::handoffs::discard_prepared(mirror, &PreparedHandoff)` removes the ref (`remove_handoff`, idempotent) for the composition task to call when the database step fails; failures are logged at `warn!` with `project_id` and `handoff_id` fields and otherwise ignored (orphan cleanup is the backstop).
- [ ] The function never opens a database transaction and never acquires the git lock itself: the caller passes the guard.

## Implementation notes
- Files: `orchestrator/src/tracker/handoffs.rs` (new; if the tracker epic placed its service module elsewhere, e.g. `src/tasks/`, colocate with it), `orchestrator/src/tracker/mod.rs`.
- Order inside the caller (documented in the module doc): git lock -> `prepare` -> `begin_mutation` (project row) -> task row `FOR UPDATE` -> recheck -> write -> commit -> release git lock. The git lock is held through the transaction commit so a task merge's approval check under the same lock cannot interleave with a publication.
- Use `git::refs::resolve` on the mirror path from the project layout helper (`DataPaths::mirror(project_id)`), never on the session work clone.
- Never log the hand-off comment; log `task_id`, `handoff_id`, `commit` as structured fields at `debug`.

## Edge cases
- Source session `done`/`failed` with its directory removed but `refs/sessions/<sid>` present: the sync is skipped and the existing ref tip is compared with the commit (allows publishing work from an ended session).
- Source session deleted from the database: revision -> 400 (cannot verify project membership); forward -> works, `source_session_id` copied as `None` from the current row.
- The commit is reachable in the mirror but the session tip has moved past it (agent committed more after telling the orchestrator): 409 with the tip in the message; the agent re-reads its tip and republishes.
- Two publications for the same task race: the git lock serialises them; the second sees the first's committed state in its own pre-validation (or in the transaction recheck).
- Forward whose current hand-off commit was retained but the ref was removed by an interrupted deletion: `retain_handoff` still succeeds because the object exists in the mirror (gc is disabled); if the object is gone the operation fails with the 409 above.

## Testing
- Tests with real bare repositories in `tempfile` directories (reuse the git epic's test helper: upstream repo, `init_project_repo`, session rows via `SessionRepository`, work clone via `create_work_clone`) driven through `TestApp`: revision with a matching commit creates `refs/handoffs/<id>` pointing at it; tip mismatch returns `Conflict` and `git::refs::list_handoffs` is empty; a caller session that does not hold the lease -> `Conflict`; a user caller with a held task passes pre-validation; source session of another project -> `BadRequest`; forward with the current id pins a second ref at the same commit without touching the work clone (delete the work dir first); forward with a stale id -> `Conflict`; `discard_prepared` removes the ref and is idempotent.
- `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Git operations: mirror, clones, integration and REST API": `ProjectGitLocks` / `ProjectGitGuard`, `git::refs::{resolve, retain_handoff, remove_handoff, list_handoffs}`, `GitService::sync_session_silent` and its missing-work-dir rule, the real-repository test helper.
- "Database schema, models, repositories and test harness": `SessionRepository::get`, `TaskRepository::find_handoff`, `Task`/`TaskState`/`TaskHandoff` row types.
- "Task tracker: states, tasks, leases, dependencies and events": the `Task` read path used for pre-validation (`find_task`, `find_state_by_name`).