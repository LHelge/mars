---
id: "2xrbu"
title: Add project, shared-directory and agent-profile models and ProjectRepository with the project row lock
status: in_progress
priority: P1
created: "2026-09-16T20:30:40.445907825Z"
updated: "2026-09-17T07:17:20.157266248Z"
tags:
  - orchestrator
  - core
  - projects
depends_on:
  - k42gy
parent: p5tsd
assignee: claude-opus-subagent
attempts: 1
---

## Summary
Add the `Project`, `SharedDir` and `AgentProfile` domain types with their validation rules and error enums, and `ProjectRepository<'a>` covering basic CRUD for the three tables plus two primitives every tracker mutation depends on: `lock_project` (`SELECT id FROM projects WHERE id = $1 FOR UPDATE`) and `allocate_task_number` (`UPDATE projects SET next_task_number = next_task_number + 1 ... RETURNING`). The clone job, default-profile creation and the routes belong to the projects epic.

## Documents
- `docs/data-model.md` "Enums" (`project_status`, `profile_kind`, `agent_backend`), "Projects and profiles" (`projects`, `project_shared_dirs`, `agent_profiles`), "Tracker mutation transactions" (project row lock, `next_task_number` taken with `UPDATE ... RETURNING`).
- `SPEC.md` "Projects" (`Project` DTO; `max_attempts` 1–20), "Shared directories" (`name` and `container_path` rules), "Agent profiles" (`permission_mode` must be `bypass`; `partial_messages` default by kind).
- `ARCHITECTURE.md` "Storage" (shared-directory path rules), "Task tracker" (project row lock).

## Acceptance criteria
- [ ] Enums `ProjectStatus { Cloning, Ready, Error }`, `ProfileKind { Conversational, Ephemeral }`, `AgentBackend { Claude }` derive `sqlx::Type` with the Postgres type names and `serde(rename_all = "snake_case")`.
- [ ] `ProjectName::parse`: trimmed, 1–100 chars (`ProjectError::InvalidName`). `RemoteUrl::parse`: must start with `https://`, must not contain userinfo (`@` before the first `/` after the scheme), no whitespace (`ProjectError::InvalidRemoteUrl`). `MaxAttempts::parse(i16)`: 1–20 (`ProjectError::InvalidMaxAttempts`). `default_branch`, when given, is non-empty and contains no whitespace.
- [ ] `SharedDirName::parse`: 1–64 chars matching `^[a-z0-9][a-z0-9_-]*$` (`SharedDirError::InvalidName`). `ContainerPath::parse`: absolute; no `.` or `..` segments; no repeated or trailing slashes; not `/data` nor below it; neither equal to nor an ancestor of `/session/work`, `/session/home`, `/session/log`, `/session/mcp.json`; may lie inside `/session/work` (`SharedDirError::InvalidPath` with a message naming the rule).
- [ ] `NewAgentProfile::validate`: `name` trimmed non-empty; `permission_mode == "bypass"` (`ProfileError::UnsupportedPermissionMode`); `image` non-empty; `idle_timeout_secs >= 1`; each `secrets` entry matches `^[A-Z][A-Z0-9_]{0,127}$`; `partial_messages: Option<bool>` resolves to `true` for `Conversational` and `false` for `Ephemeral` when `None`.
- [ ] `ProjectError`, `SharedDirError`, `ProfileError` are `#[from]` variants of `prelude::Error` mapping to 400.
- [ ] `ProjectRepository<'a>`: `insert(tx, &NewProject)` (unique violation on `projects_name_key` → `Conflict("project name already taken")`), `find(id) -> Option<Project>`, `list()` ordered by `name`, `update(tx, id, ProjectUpdate { name?, default_branch?, max_attempts? })`, `set_status(tx, id, status, status_message)`, `set_last_fetched_at(tx, id)`, `delete(tx, id) -> bool`; every write sets `updated_at = NOW()`.
- [ ] `lock_project(tx, project_id) -> Result<()>` executes `SELECT id FROM projects WHERE id = $1 FOR UPDATE` and returns `Error::NotFound` when no row matches.
- [ ] `allocate_task_number(tx, project_id) -> Result<i32>` executes `UPDATE projects SET next_task_number = next_task_number + 1 WHERE id = $1 RETURNING next_task_number - 1` and is documented as "call only while holding `lock_project` in the same transaction".
- [ ] Shared directories: `insert_shared_dir(tx, project_id, &NewSharedDir)` (PK violation → `Conflict("shared directory name already used")`, unique `(project_id, container_path)` → `Conflict("container path already used")`), `list_shared_dirs(project_id)` ordered by `name`, `delete_shared_dir(tx, project_id, name) -> bool`.
- [ ] Profiles: `insert_profile(tx, &NewAgentProfile)` (unique `(project_id, name)` → `Conflict("profile name already taken")`; `agent_profiles_one_default_idx` → `Conflict("project already has a default profile")`), `find_profile(project_id, id)`, `find_default_profile(project_id)`, `list_profiles(project_id)`, `update_profile(tx, project_id, id, ProfileUpdate)`, `delete_profile(tx, project_id, id) -> Result<bool>` mapping the `sessions_profile_id_fkey` restrict violation to `Conflict("profile has sessions")`.
- [ ] Scope in every `WHERE` (`... WHERE id = $1 AND project_id = $2`), never a check after the mutation.
- [ ] `cargo sqlx prepare` run and `.sqlx/` committed.

## Implementation notes
- Files: `orchestrator/src/models/project.rs`, `models/shared_dir.rs`, `models/agent_profile.rs`, `src/repositories/projects.rs`, `src/prelude/error.rs`.
- `TEXT[]` columns map to `Vec<String>`; `sqlx::query_as!` needs the enum types annotated as `status as "status: ProjectStatus"` etc.
- The `Project` DTO's `has_credential` is derived by the projects epic from the `GIT_CREDENTIAL` secret; the row struct does not carry it.
- Path normalisation: split on `/`, reject empty segments (except the leading one), `.` and `..`; compare against the reserved list by segment prefix, not string prefix (`/session/workspace` is allowed, `/session/work` is not).

## Edge cases
- `set_status(Ready)` with a NULL `default_branch` violates the table `CHECK`; map the check violation (`sqlx::Error::Database` with constraint name) to `Error::Conflict("default branch unknown")` rather than 500.
- `remote_url` with a trailing `.git` or without is accepted as-is; no normalisation.
- `delete` of a project cascades sessions, tasks, profiles, shared dirs at the database level; the filesystem and git cleanup are the projects epic's.

## Testing
- Unit tests for every validation rule above, including the reserved-path table (`/data`, `/data/x`, `/session/work`, `/session`, `/`, `/session/work/target` allowed, `/a//b`, `/a/../b`, `/a/`, `relative`).
- Integration tests in `tests/repositories_projects.rs` on `common::db::test_pool()`: CRUD round trips for projects, shared dirs and profiles; each documented `Conflict`; `lock_project` on an unknown id → `NotFound`; two concurrent transactions calling `lock_project` + `allocate_task_number` produce 1 and 2 with no duplicates; deleting a profile referenced by a session → `Conflict`.
- `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` must pass.

## Documentation
- None: implements the documented contract as written.

## Assumes from other epics
- "Repository scaffolding, tooling and CI": `prelude::Error` and `repositories/mod.rs` skeleton; the `unique_violation` helper from the UserRepository task of this epic (if that task is not yet done, add the helper here, it is a few lines).