---
id: "5ywhm"
title: "Implement the project-creation transaction: insert project, seed default task states, default profile serving ready, and the GIT_CREDENTIAL secret"
status: in_progress
priority: P1
created: "2026-09-16T20:30:05.449645875Z"
updated: "2026-09-18T09:49:52.853116818Z"
tags:
  - orchestrator
  - projects
  - tracker
  - secrets
depends_on:
  - "8rwjd"
  - z4g29
parent: pkaee
assignee: claude-opus-subagent
attempts: 1
---

## Summary
Provide one function, `projects::create_project`, that performs everything a new project needs in a single database transaction: the `projects` row in `cloning`, the seven default `task_states`, the `default` conversational profile using `SESSION_IMAGE_DEFAULT`, its `profile_states` row for `ready`, and, when a credential was supplied, the project-scoped orchestrator-only `GIT_CREDENTIAL` secret. The route task calls this and then spawns the clone job; nothing here touches git or the filesystem.

## Documents
- `SPEC.md` "Projects" (`POST /projects` body `{name, remote_url, default_branch?, credential?}`, `credential` stored as the project-scoped orchestrator-only secret `GIT_CREDENTIAL`), "User-facing features" → "Agent profiles" (every project starts with a `default` conversational profile using the built-in Claude image serving `ready`), "Task states" (every project starts with the default set).
- `docs/data-model.md` `task_states` "Default set, created with the project" (`backlog` queue 0, `ready` queue 1, `review` queue 2, `merge` queue 3, `needs_human` human 4, `done` terminal 5, `cancelled` terminal 6), `profile_states` ("The default profile of a new project serves `ready`"), `agent_profiles` (defaults, `is_default`), `projects` ("The remote credential is not a column"), `secrets` (`scope='project'`, `scope_id = projects.id`, `orchestrator_only`).
- `README.md` "Configuration" `SESSION_IMAGE_DEFAULT`.

## Acceptance criteria
- [ ] `orchestrator/src/projects/create.rs` exposes `pub async fn create_project(state: &AppState, input: NewProjectRequest, created_by: Uuid) -> Result<Project>` where `NewProjectRequest { name, remote_url, default_branch: Option<String>, credential: Option<String> }`.
- [ ] Validation runs before the transaction (`NewProject` from the project model; `credential`, when present, must be non-empty after trimming and ≤ 4096 chars, 400 otherwise).
- [ ] One `BEGIN … COMMIT` covers, in order: `ProjectRepository::insert` (status `cloning`, `default_branch` as supplied or null); `TaskStateRepository::seed_defaults(tx, project_id)` inserting the seven rows with the exact names, kinds and positions above; `AgentProfileRepository::insert` of `{ name: "default", kind: conversational, backend: claude, permission_mode: "bypass", image: config.session_image_default, mcp_tools: [], secrets: [], partial_messages: true, idle_timeout_secs: 1800, is_default: true }`; `AgentProfileRepository::set_served_states(tx, project_id, profile_id, &["ready"])`; when `credential` is `Some`, the secrets epic's create-in-transaction function with `scope = project`, `scope_id = project_id`, `name = "GIT_CREDENTIAL"`, `orchestrator_only = true`, `created_by`.
- [ ] The returned `Project` has `status = cloning` and `has_credential` reflecting whether a credential was stored.
- [ ] The plaintext credential is held in a `zeroize`d buffer and never logged, never included in any error message, and dropped before the function returns.
- [ ] Seeding is not written as a trigger or migration default: it is application code so the tracker epic's state editing semantics stay in one place.
- [ ] `.sqlx/` refreshed and committed.

## Implementation notes
- Files: `orchestrator/src/projects/create.rs` (new), `orchestrator/src/repositories/task_states.rs` (add `seed_defaults`), `orchestrator/src/projects/mod.rs` (export).
- Constant `pub const GIT_CREDENTIAL_SECRET_NAME: &str = "GIT_CREDENTIAL";` must be the one the secrets/git epics define; import it rather than redefining. If it does not exist yet, define it in `secrets/` and note it for the git epic.
- No project row lock is needed: the row is being inserted and is invisible to other transactions until commit; no `task_events` row is written for the seeded states (there is no subscriber yet, and `states_changed` is for edits; state this in a comment so the tracker epic does not "fix" it).
- The project id is generated with `Uuid::new_v4()` before the insert so the directory name is known to the clone job.
- No filesystem or git work here; the route spawns the clone job after commit (next task) so a failed commit never leaves a directory behind.

## Edge cases
- Duplicate `name` → the repository's `Error::Conflict("project name already exists")`, transaction rolled back, no secret written.
- Secret creation failing (keyring error) rolls back the project too; the user sees a 500 with a generic message and the error is logged with `project_id`.
- `credential: Some("")` or whitespace-only → 400 "credential must not be empty"; omit the field for a public repository.
- `default_branch` supplied → stored as-is and the clone job validates it against the fetched heads; not supplied → null until discovery.

## Testing
- Integration tests through `TestApp` calling `create_project` directly: project row is `cloning`; `task_states` for the project are exactly the seven rows in position order with the right kinds; exactly one `agent_profiles` row, `is_default = true`, name `default`, `partial_messages = true`, `idle_timeout_secs = 1800`, image equal to the test config's `session_image_default`; `profile_states` links it to `ready`; with `credential` a `secrets` row `(project, <id>, GIT_CREDENTIAL, orchestrator_only = true)` exists and decrypts to the given value through the secrets repository; without it none exists and `has_credential = false`; duplicate name rolls everything back (no orphan states, profile or secret).
- `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` must pass.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Database schema, models, repositories and test harness": `TaskStateRepository` skeleton, migrations, `TestApp`, `Config.session_image_default`.
- "Secrets manager": a function to create a secret inside a caller-supplied transaction (encrypting with the keyring in `AppState`), the `GIT_CREDENTIAL` constant, and a way to read a secret's plaintext in tests.
- "Task tracker": owns state editing and events; this task only seeds rows.