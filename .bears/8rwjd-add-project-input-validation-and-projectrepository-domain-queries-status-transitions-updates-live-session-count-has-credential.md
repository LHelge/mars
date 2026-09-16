---
id: "8rwjd"
title: Add project input validation and ProjectRepository domain queries (status transitions, updates, live-session count, has_credential)
status: open
priority: P1
created: "2026-09-16T20:28:30.815282444Z"
updated: "2026-09-16T20:51:51.836811748Z"
tags:
  - orchestrator
  - projects
depends_on:
  - z4u4e
parent: pkaee
---

## Summary
Complete the `Project` model and `ProjectRepository` so the routes, clone job and deletion in this epic have every query they need: validated create and update inputs, status transitions guarded by the current status in the `WHERE` clause, `last_fetched_at` updates, `has_credential` computed from the `secrets` table, and a count of live sessions used by every running-session refusal. The schema epic delivers the basic model and CRUD; this task adds the domain shape.

## Documents
- `SPEC.md` "Projects (`/api/projects`)": `Project` DTO fields, `max_attempts` 1–20 default 3, `default_branch` null while discovery pending, `credential` stored as `GIT_CREDENTIAL`.
- `docs/data-model.md` `projects` (columns, `CHECK (status <> 'ready' OR default_branch IS NOT NULL)`, `max_attempts` CHECK 1–20, `name` UNIQUE 1–100 chars, `remote_url` `https://` only), `project_status` enum, `sessions` (`state`), `secrets` (`scope = 'project'`, `scope_id`, `name`).
- `ARCHITECTURE.md` "Orchestrator internals" (models hold validation and a per-model error enum, repositories hold all SQL with the scope in the `WHERE` clause).

## Acceptance criteria
- [ ] `models/project.rs` defines `Project` (all columns), `ProjectStatus` (`Cloning`, `Ready`, `Error`, serialised lower-case), `NewProject { name, remote_url, default_branch: Option<String>, created_by }`, `ProjectUpdate { name: Option<String>, default_branch: Option<String>, max_attempts: Option<i16> }` and `ProjectError` (`#[from]` into `Error` as 400).
- [ ] Validation: `name` trimmed, 1–100 chars; `remote_url` must parse as a URL with scheme `https` and no userinfo (`user:pass@`), no fragment; `default_branch` when given is 1–255 chars and passes a conservative git branch-name check (no whitespace or control chars, no `..`, `~`, `^`, `:`, `?`, `*`, `[`, `\`, `@{`, does not start with `-`, `/` or `refs/`, does not end with `/`, `.` or `.lock`); `max_attempts` 1–20.
- [ ] Under `#[cfg(feature = "integration-tests")]` only, `remote_url` additionally accepts absolute `file://` URLs so integration tests can clone real local bare repositories; the release build keeps `https://` only. The exception is documented in a doc comment on the validator.
- [ ] `ProjectRepository<'a>` (borrowing `&PgPool`) provides: `insert(tx, &NewProject) -> Project` (status `cloning`), `list() -> Vec<Project>`, `get(id) -> Option<Project>`, `update(id, &ProjectUpdate) -> Option<Project>`, `mark_ready(id, default_branch)` (`UPDATE ... SET status='ready', default_branch=$2, status_message=NULL, last_fetched_at=NOW(), updated_at=NOW() WHERE id=$1 AND status='cloning'` returning the rows-affected count), `mark_error(id, message)` (same guard `status='cloning'`), `mark_cloning_from_error(id) -> Option<Project>` (`WHERE id=$1 AND status='error'`), `touch_fetched(id)` (`last_fetched_at=NOW()`), `delete(tx, id) -> bool`, `lock_for_update(tx, id) -> bool` (`SELECT id FROM projects WHERE id=$1 FOR UPDATE`).
- [ ] Every read returns `has_credential` computed as `EXISTS (SELECT 1 FROM secrets s WHERE s.scope='project' AND s.scope_id=p.id AND s.name='GIT_CREDENTIAL')` in the same query; the `Project` model carries `has_credential: bool`.
- [ ] `SessionRepository::count_live_for_project(tx_or_pool, project_id) -> i64` runs `SELECT COUNT(*) FROM sessions WHERE project_id=$1 AND state IN ('running','creating')`.
- [ ] A unique violation on `projects_name_key` is mapped to `Error::Conflict("project name already exists")`.
- [ ] `.sqlx/` is refreshed with `cargo sqlx prepare` and committed.

## Implementation notes
- Files: `orchestrator/src/models/project.rs`, `orchestrator/src/repositories/projects.rs`, `orchestrator/src/repositories/sessions.rs` (one added method), `orchestrator/src/prelude/error.rs` (add `#[from] ProjectError` if the schema epic did not).
- Use `sqlx::query_as!` with an explicit `ProjectRow` struct and a `From<ProjectRow> for Project`; `status` maps through `sqlx::Type` on `ProjectStatus` with `#[sqlx(type_name = "project_status", rename_all = "lowercase")]`.
- `update` builds `SET name = COALESCE($2, name), default_branch = COALESCE($3, default_branch), max_attempts = COALESCE($4, max_attempts), updated_at = NOW()`; the caller decides whether `default_branch` may change (route task validates against the mirror when the project is `ready`).
- `mark_ready` must satisfy the CHECK constraint by always supplying a non-null `default_branch`.
- Repository helpers that write take `&mut PgConnection` / the caller's transaction (`impl Executor` or `&mut Transaction<'_, Postgres>`), following the pattern the schema epic established.
- The `Project` DTO for routes is the model serialised with `serde`: `{ id, name, remote_url, default_branch, status, status_message, last_fetched_at, max_attempts, created_at, has_credential }`; `updated_at`, `created_by` and `next_task_number` are `#[serde(skip)]` or excluded via a route-private DTO in the routes task.

## Edge cases
- `remote_url` with userinfo (`https://token@github.com/...`) is rejected with 400 "remote_url must not contain credentials"; the credential goes in `credential`.
- `remote_url` is stored exactly as given after trimming; no normalisation of `.git` suffix.
- `mark_ready`/`mark_error` returning 0 rows means the project was deleted or already transitioned; callers treat it as "stop, nothing to do", never as an error.
- `name` uniqueness is global, not per user.

## Testing
- Unit tests for every validation rule above (valid https URL, http rejected, userinfo rejected, `file://` accepted only with the feature, branch-name rejects `-x`, `a..b`, `a/`, `a.lock`, `refs/heads/x`, accepts `main`, `release/1.2`, `feat_x-1`).
- Integration tests through `TestApp` (repository level, no routes yet): insert → `get` shows `cloning` and `has_credential=false`; after inserting a project-scoped `GIT_CREDENTIAL` secret through the secrets repository `has_credential=true`; `mark_ready` twice affects 1 then 0 rows; `mark_cloning_from_error` on a `cloning` project returns `None`; `count_live_for_project` counts only `running`/`creating`.
- `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` must pass.

## Documentation
- none: implements the documented contract as written (the `file://` test-only exception lives in a doc comment, not in `SPEC.md`, because release behaviour is unchanged).

## Assumes from other epics
- "Database schema, models, repositories and test harness": `projects` migration, basic `Project` model and `ProjectRepository` skeleton, `SessionRepository`, `TestApp::spawn()`.
- "Secrets manager": the `secrets` table is populated through `SecretsRepository`; this task only reads its existence.