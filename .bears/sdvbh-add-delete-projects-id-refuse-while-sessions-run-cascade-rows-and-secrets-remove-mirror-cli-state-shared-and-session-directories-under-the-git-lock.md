---
id: sdvbh
title: "Add DELETE /projects/{id}: refuse while sessions run, cascade rows and secrets, remove mirror, CLI state, shared and session directories under the git lock"
status: in_progress
priority: P1
created: "2026-09-16T20:32:20.226061547Z"
updated: "2026-09-18T11:05:35.412404470Z"
tags:
  - orchestrator
  - projects
  - git
depends_on:
  - cgj5v
  - maq3y
parent: pkaee
assignee: claude-opus-subagent
attempts: 1
---

## Summary
Implement project deletion as `projects::delete_project` plus the `DELETE /projects/{id}` handler. It takes the project git lock, then in one database transaction locks the project row, refuses with 409 if any session is `running` or `creating`, deletes the project's secrets (no FK) and the project row (cascading profiles, shared dirs, sessions, events, tasks, task events, hand-offs), commits, and only then removes every on-disk artefact: the project directory (`repo.git`, `claude/`, `shared/`) and each former session's directory.

## Documents
- `SPEC.md` "Projects" row `DELETE /projects/{id}` (204; 409 while any session is `running` or `creating`), "User-facing features" → "Projects" ("Deleting a project deletes its sessions, tasks, secrets, shared directories, CLI state directory and mirror").
- `ARCHITECTURE.md` "Storage" (last paragraph: "Deleting a project removes the mirror, the CLI state directory and all shared directories after all of its sessions are gone"), "Git model" → "Serialization" ("project deletion uses the same lock"; git lock before database project lock), "Task tracker" → "One mutation at a time per project" (project deletion coordinates tracker changes through the project lock), "Background jobs" (orphan cleanup as the backstop).
- `README.md` "Operating notes" ("Removing a project removes its mirror, its CLI state directory, its shared directories and every session directory under it").
- `docs/data-model.md`: every FK to `projects(id)` is `ON DELETE CASCADE`; `secrets.scope_id` has no FK; `task_handoffs` refs live in `repo.git`.

## Acceptance criteria
- [ ] `DELETE /api/projects/{id}` → 204 on success; 404 unknown; 409 "project has running sessions" while `count_live_for_project > 0`; 401 unauthenticated.
- [ ] Order inside `projects::delete_project(state, id)`: acquire the project git lock → `BEGIN` → `ProjectRepository::lock_for_update` (false → 404) → `SessionRepository::count_live_for_project` (> 0 → rollback, 409) → collect the project's session ids and `cli_session_id`s → `DELETE FROM secrets WHERE scope = 'project' AND scope_id = $1` → `ProjectRepository::delete` → `COMMIT` → `ProjectLayout::remove_all()` → for each session id `remove_dir_all(session_dir(data_dir, sid))` (missing is fine) → release the git lock.
- [ ] Filesystem failures after commit are logged with `project_id` and the path in structured fields and still answer 204 (the rows are gone; the orphan cleanup job and operators handle leftovers); they never roll the deletion back.
- [ ] No container work: with no session `running`/`creating`, every session container is already removed by the lifecycle rules; stray containers are the orphan-cleanup job's responsibility. If the session registry exposes a way to drop entries for a project, call it after commit.
- [ ] Deleting a project with `parked`, `done` and `failed` sessions, tasks with dependencies, comments, hand-offs, task events, profiles, shared dirs and secrets leaves zero rows in every table for that id (including `task_events`, which cascade on `project_id`).
- [ ] `.sqlx/` refreshed for the new secrets and session-id queries.

## Implementation notes
- Files: `orchestrator/src/projects/delete.rs` (new), `orchestrator/src/routes/projects.rs` (handler), `orchestrator/src/repositories/secrets.rs` (add `delete_all_for_project(tx, project_id)` if the secrets epic did not), `orchestrator/src/repositories/sessions.rs` (add `ids_for_project(tx, project_id) -> Vec<Uuid>`).
- Hold the git lock across the transaction and the filesystem removal so the cron mirror fetch, a launch or a git endpoint cannot open `repo.git` while it is being removed.
- Because `claude/` holds transcripts by `cli_session_id`, removing the whole project directory removes them; no per-session transcript removal is needed here.
- Keep the transaction short: no filesystem work between `BEGIN` and `COMMIT`.

## Edge cases
- Race with a launch: the launcher inserts the session row as `creating` in its own transaction; if it commits first the count sees it (409); if deletion commits first the launcher's later steps find no project and fail the launch. Both outcomes are acceptable and documented by the 409 contract; the git lock additionally serializes the launcher's clone against the removal.
- Race with the clone job: the job holds the git lock; deletion waits, then deletes; the job's final `mark_*` affects 0 rows and it removes what it created (see the clone-job task).
- Deleting a project whose directory was never created (creation transaction committed but the job had not started) is a 204 with nothing to remove.
- A second `DELETE` for the same id after the first → 404.

## Testing
- Integration tests in `orchestrator/tests/projects.rs`:
  - happy path with a `ready` project (real `file://` bare repo): create a shared dir row and its directory, a profile, tasks with a dependency and a comment, a `parked` session row with a session directory under `DATA_DIR/sessions/<sid>/`, a project secret and a `GIT_CREDENTIAL`; `DELETE` → 204; every table has zero rows for the project (`projects`, `agent_profiles`, `profile_states`, `project_shared_dirs`, `sessions`, `events`, `task_states`, `tasks`, `task_dependencies`, `task_comments`, `task_events`, `secrets`); `DATA_DIR/projects/<id>` and `DATA_DIR/sessions/<sid>` no longer exist.
  - a `running` session row → 409 and nothing removed; a `creating` one → 409; after updating it to `done` → 204.
  - unknown id → 404; no token → 401; a `cloning` project with no directory → 204.
- `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` must pass.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Git operations": the per-project git lock.
- "Secrets manager": `SecretsRepository` (a `delete_all_for_project` helper is added here if absent).
- "Session lifecycle": the session directory path convention `DATA_DIR/sessions/<sid>/` (mirrored by `projects::layout::session_dir`) and, optionally, a registry method to forget a project's sessions.