---
id: "26rj4"
title: Implement the background clone job moving a project from cloning to ready or error under the project git lock
status: done
priority: P1
created: "2026-09-16T20:30:42.442708759Z"
updated: "2026-09-18T10:30:07.099391524Z"
tags:
  - orchestrator
  - projects
  - git
depends_on:
  - maq3y
  - "8rwjd"
parent: pkaee
assignee: claude-opus-subagent
attempts: 1
---

## Summary
Implement `projects::clone_job::run(state, project_id)`, the background task spawned after `POST /projects` and `POST /projects/{id}/retry-clone`. Under the per-project git lock it creates the data-directory layout, initialises the bare project repository through the git module, discovers the default branch when the user did not supply one, runs the first fetch, seeds the integration heads, verifies the default branch resolves to an integration head, and marks the project `ready`; any failure marks it `error` with a credential-free `status_message`. The job is idempotent so `retry-clone` simply runs it again.

## Documents
- `ARCHITECTURE.md` "Git model" → "Project clone" (init --bare, `origin`, refspecs `+refs/heads/*:refs/remotes/origin/*` and `+refs/tags/*:refs/tags/*`, `gc.auto=0`, `gc.pruneExpire=never`, no `--mirror`, `git ls-remote --symref origin HEAD` unless `default_branch` supplied, seeding `refs/heads/<branch>` from each fetched upstream branch, bare `HEAD` set to the default integration branch), "Serialization" (initialization is under the per-project git lock; git lock before any database lock), "Credentials", "Storage".
- `SPEC.md` "Projects" (`status: cloning` → `ready`/`error`, `default_branch` "must resolve to an integration head before the project becomes `ready`", `status_message`).
- `docs/data-model.md` `projects` (`status`, `status_message`, `last_fetched_at`, CHECK on `default_branch`), `secret_uses` (`purpose = 'git'`).
- `CLAUDE.md` rule 3 (no credentials in logs or generated messages).

## Acceptance criteria
- [ ] `orchestrator/src/projects/clone_job.rs` exposes `pub fn spawn(state: AppState, project_id: Uuid, requested_by: Option<Uuid>) -> tokio::task::JoinHandle<()>` and `pub async fn run(state: &AppState, project_id: Uuid, requested_by: Option<Uuid>)`; `run` never panics and never returns an error to the spawner: every outcome is written to the project row and logged with `project_id = %id`.
- [ ] Sequence inside `run`: acquire the project git lock; re-read the project (stop silently if missing or not `cloning`); `ProjectLayout::ensure_created()`; if `repo.git` already exists (interrupted earlier attempt) remove it; call the git module's project-repository initialisation with `remote_url`; if `default_branch` is null, discover it with the git module's symbolic-HEAD discovery; fetch (`--prune`) with the credential provider resolving `GIT_CREDENTIAL` for this project; seed integration heads and set `HEAD`; verify `refs/heads/<default_branch>` exists; `ProjectRepository::mark_ready(id, default_branch)`; release the lock.
- [ ] On any error: `ProjectRepository::mark_error(id, message)` where `message` is a single line ≤ 1000 chars derived from the `GitError` display (which by the git epic's contract never contains the credential), additionally passed through a scrubber that replaces any `://<userinfo>@` in URLs with `://***@`; the `repo.git` directory is left in place (it is removed on the next run) and the lock is released.
- [ ] Specific messages: unreachable or unauthenticated remote → the git stderr summary; remote with no branches → "remote has no branches"; user-supplied branch absent after fetch → `default branch "<name>" not found on remote`; discovery failure with a non-empty remote → `could not discover the remote default branch; set default_branch explicitly`.
- [ ] `secret_uses`: the credential use is recorded with `purpose = git`, `user_id = requested_by`, `session_id = NULL` (the requesting user asked through REST; the mirror-fetch cron passes `None`).
- [ ] If the project row disappears while the job runs (`mark_ready`/`mark_error` affect 0 rows), the job removes the layout it created and exits without error.
- [ ] `.sqlx/` refreshed if new queries were added.

## Implementation notes
- Files: `orchestrator/src/projects/clone_job.rs`, `orchestrator/src/projects/mod.rs`.
- Use the git epic's `git::project` (or equivalently named) functions: `init_project_repository(path, remote_url)`, `discover_default_branch(path, credentials)`, `fetch_upstream(path, credentials)`, `seed_integration_heads(path, default_branch)`, `has_integration_head(path, name)`; and `GitCredentialProvider::for_project(project_id, actor)`. If any of these is missing, add it to `git/` in this task following the wrapper's argv-array conventions and note it in the git epic.
- Git lock handle: whatever the git epic exposes on `AppState` (e.g. `state.git_locks.lock(project_id).await`); hold the guard across the whole job, and open database transactions only while holding it (git lock before database lock; never the reverse).
- Spawn with `tokio::spawn` from the route after the creation transaction commits; keep a `tracing::info_span!("project_clone", project_id = %id)` on the task.
- Under the `integration-tests` feature, expose `pub async fn wait_for_clone(state: &AppState, project_id: Uuid, timeout: Duration) -> Project` that polls the row every 50 ms until `status != cloning`; integration tests in this and later tasks use it instead of sleeping. (Implement it as a plain poll so it needs no registry; a `JoinHandle` registry is not required.)

## Edge cases
- `remote_url` pointing at an empty repository: `ls-remote --symref HEAD` yields nothing → error "remote has no branches".
- Discovered `HEAD` pointing at a branch that is not among fetched heads (rare, e.g. `HEAD` → deleted branch) → the "default branch not found" error.
- Two concurrent `retry-clone` requests: the route transitions `error → cloning` atomically (only one wins) and the job re-checks status under the git lock, so only one job does work.
- Project deleted mid-clone: deletion also takes the git lock, so it waits for the job; the job's final `mark_*` then affects 0 rows and it cleans up.
- Credential rotated while cloning: irrelevant; the provider reads the secret once per command.

## Testing
- Integration tests through `TestApp` (git is never mocked; use `tempfile` bare repositories with at least one commit created through the `git` binary in the test):
  - project created with `file://<bare>` (the test-only URL exception) and no `default_branch` → `wait_for_clone` yields `ready`, `default_branch` equals the bare repo's `HEAD` branch, `last_fetched_at` set, `repo.git` exists with `refs/heads/<branch>` and `refs/remotes/origin/<branch>`, `claude/` and `shared/` exist, `gc.auto` is `0`.
  - user-supplied `default_branch` that exists → `ready` with that branch; that does not exist → `error` with the exact message.
  - unreachable `https://127.0.0.1:1/x.git` → `error`, `status_message` non-empty and containing no `@`.
  - empty bare repository → `error` "remote has no branches".
  - second `run` after fixing the remote (create the bare repo at the same path) → `ready`; `repo.git` was re-initialised cleanly.
  - project row deleted before the job's final update → no row change, layout removed.
- `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` must pass.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Git operations: mirror, clones, integration and REST API": project-repository init, symbolic-HEAD discovery, upstream fetch, integration-head seeding, `GitCredentialProvider` with `secret_uses` recording, `GitError` display without credentials, and the per-project asynchronous git lock.
- "Secrets manager": the `GIT_CREDENTIAL` project secret lookup used by the credential provider.
- "Database schema, models, repositories and test harness": `TestApp::spawn()` sets `DATA_DIR`/`DATA_DIR_HOST` to a per-test temporary directory; if it does not, add that to `tests/common/mod.rs` in this task.