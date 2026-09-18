---
id: cgj5v
title: "Add the projects route module: GET/POST /projects, GET/PUT /projects/{id} and POST /projects/{id}/retry-clone"
status: in_progress
priority: P1
created: "2026-09-16T20:31:16.970398856Z"
updated: "2026-09-18T10:35:41.458956297Z"
tags:
  - orchestrator
  - projects
depends_on:
  - "5ywhm"
  - "26rj4"
parent: pkaee
assignee: claude-opus-subagent
attempts: 1
---

## Summary
Create `routes/projects.rs` exporting `routes() -> Router<AppState>` with the list, create, read, update and retry-clone endpoints of the Projects table. Create runs the project-creation transaction and then spawns the clone job; retry-clone flips `error → cloning` atomically and spawns the same job; update validates a changed `default_branch` against the mirror when the project is `ready`. Fetch, branches and delete are separate tasks in this epic and mount into this module.

## Documents
- `SPEC.md` "REST API" (status table, error shape `{status, error}`, 201 for creates), "Projects (`/api/projects`)" rows for `GET /projects`, `POST /projects`, `GET /projects/{id}`, `PUT /projects/{id}`, `POST /projects/{id}/retry-clone`, and the `Project` DTO paragraph.
- `SPEC.md` "Authentication" (JWT required; `must_change_password` gate → 403).
- `ARCHITECTURE.md` "Orchestrator internals" (routes: one module per resource, DTOs private to the module), "Git model" → "Project clone" (bare `HEAD` points at the default integration branch).
- `docs/data-model.md` `projects`.
- `CLAUDE.md` "API conventions", "Testing expectations".

## Acceptance criteria
- [ ] `GET /api/projects` → 200 `Project[]` ordered by `created_at` ascending; `GET /api/projects/{id}` → 200 `Project` or 404.
- [ ] `POST /api/projects` with `{name, remote_url, default_branch?, credential?}` → 201 `Project` with `status: "cloning"` and `has_credential`; 400 for every validation failure with the model's message; 409 "project name already exists"; the clone job is spawned only after the transaction committed, with `requested_by = current user`.
- [ ] `PUT /api/projects/{id}` with `{name?, default_branch?, max_attempts?}` → 200 `Project`; 400 for invalid values; 404 unknown; 409 duplicate name; an empty body `{}` is a 200 no-op returning the current row.
- [ ] `PUT` changing `default_branch` on a `ready` project verifies under the project git lock that `refs/heads/<name>` exists in `repo.git` (400 `default_branch "<name>" is not an integration head of this project` otherwise) and updates the bare repository's symbolic `HEAD` to it in the same locked section before writing the row. On a `cloning` or `error` project the value is stored without a git check (the clone job validates it).
- [ ] `POST /api/projects/{id}/retry-clone` → 200 `Project` with `status: "cloning"` and `status_message: null` when the project was `error`; 409 "project is not in error state" otherwise; 404 unknown; the job is spawned after the update.
- [ ] Every endpoint returns 401 without a valid JWT and 403 while `must_change_password` is set (the auth epic's extractor/middleware provides both).
- [ ] The `Project` response DTO is exactly `{ id, name, remote_url, default_branch, status, status_message, last_fetched_at, max_attempts, created_at, has_credential }` (no `updated_at`, `created_by`, `next_task_number`); `credential` never appears in any response or log.
- [ ] The router is mounted under `/api` in the app builder.

## Implementation notes
- Files: `orchestrator/src/routes/projects.rs` (new), `orchestrator/src/routes/mod.rs` (mount), reuse `projects::create_project`, `projects::clone_job::spawn`, `ProjectRepository`.
- Private DTOs: `CreateProjectBody`, `UpdateProjectBody`, `ProjectDto` with `From<Project>`.
- Handler order for `PUT` with `default_branch`: validate body → load project → if `ready` and `default_branch` differs: `git_lock(project_id)` → `has_integration_head` → `set_head` → drop lock → `ProjectRepository::update`. Git lock before the database write; never hold a database transaction while waiting for the git lock.
- `retry-clone`: `ProjectRepository::mark_cloning_from_error(id)`; `None` → distinguish 404 (row missing) from 409 (row exists, wrong status) with a follow-up `get`.
- Logging: `tracing::info!(project_id = %id, "project created")`; never log the body.

## Edge cases
- `POST` with `credential` for a public repository is allowed; it is simply stored.
- `PUT` on a `cloning` project changing `name` is fine; changing `max_attempts` is fine in any status.
- `PUT default_branch` equal to the current value is a no-op (no git check).
- Concurrent `retry-clone` calls: only one gets 200; the other gets 409 because the guarded `UPDATE` affected 0 rows and the row is now `cloning`.
- `GET /projects/{id}` with a non-UUID path segment → 400 (axum `Path<Uuid>` rejection mapped to the error shape by the shared rejection handler).

## Testing
- Integration tests in `orchestrator/tests/projects.rs` via `TestApp`, one `#[tokio::test]` per scenario, asserting with `response.assert_status()` and `response.json::<T>()`:
  - 401 for each of the five endpoints without a token; 403 for a user with `must_change_password`.
  - create → 201 `cloning`, `has_credential` false/true (with credential); the `GET /api/secrets?scope=project&scope_id=<id>` list shows `GIT_CREDENTIAL` with `orchestrator_only: true` and no value.
  - create validation: bad name (empty, 101 chars), `http://` URL, URL with userinfo, bad `default_branch`, empty credential → 400 each; duplicate name → 409.
  - list contains the created project; get unknown → 404.
  - update name/max_attempts → 200; `max_attempts` 0 and 21 → 400; duplicate name → 409; empty body → 200 unchanged.
  - update `default_branch` on a `ready` project (use a `file://` bare repo with two branches and `wait_for_clone`): existing branch → 200 and `git symbolic-ref HEAD` in `repo.git` now names it; missing branch → 400 with the exact message.
  - retry-clone: from `error` (unreachable remote) → 200 `cloning`; from `ready` → 409; unknown → 404.
- `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` must pass.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Authentication, users, invites and email": the `Claims`/current-user extractor, the `must_change_password` gate and the test helper that creates an authenticated user.
- "Git operations": `has_integration_head`, `set_head` (symbolic-ref) and the per-project git lock.
- "Secrets manager": `GET /api/secrets` for the `has_credential` assertion.