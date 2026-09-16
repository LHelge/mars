---
id: cad3v
title: Implement the GIT_CREDENTIAL project credential helpers with purpose=git use records
status: open
priority: P1
created: "2026-09-16T20:31:26.655886658Z"
updated: "2026-09-16T20:31:26.655886658Z"
tags:
  - orchestrator
  - secrets
  - git
  - projects
depends_on:
  - qafug
parent: t36d2
---

## Summary
Provide the fixed-name `GIT_CREDENTIAL` convention as a small module that the git credential provider and the project routes call: fetch and decrypt the project-scoped orchestrator-only secret named `GIT_CREDENTIAL` while recording a `secret_uses` row with `purpose = git`, store or replace it on project creation, and answer `has_credential`. This keeps crypto and the fixed name out of `git/` and `routes/projects.rs`.

## Documents
- `docs/data-model.md` `projects` ("The remote credential is not a column. It is a project-scoped, orchestrator-only secret named `GIT_CREDENTIAL` (see `secrets`). The git credential provider looks it up by that fixed name."), `secret_uses` ("Uses by the git credential provider are recorded with `purpose = 'git'` and whichever of `session_id` (MCP tool) or `user_id` (REST) asked; the mirror-fetch job sets neither")
- `SPEC.md` "Projects (`/api/projects`)" (`credential` on create "is stored as the project-scoped orchestrator-only secret `GIT_CREDENTIAL` and is never returned"; `has_credential` in `Project`)
- `ARCHITECTURE.md` "Git model" → "Credentials"; ADR 0002; ADR 0006 ("Decryption happens only at session launch (for injection) and inside the git credential provider")

## Acceptance criteria
- [ ] `pub const GIT_CREDENTIAL_NAME: &str = "GIT_CREDENTIAL";` in `orchestrator/src/secrets/git_credential.rs`.
- [ ] `pub enum GitUseContext { Session(Uuid), User(Uuid), System }` mapping to `secret_uses` `(session_id, user_id)` = `(Some, None)`, `(None, Some)`, `(None, None)`.
- [ ] `project_git_credential(pool, keyring, project_id, ctx: GitUseContext) -> Result<Option<Zeroizing<String>>>`: `find_by_scope_name(Project, Some(project_id), GIT_CREDENTIAL_NAME)`; `None` when absent (no use row); otherwise opens under the row AAD and inserts one `secret_uses` row with `purpose = "git"` in the same transaction as the read; decrypt failure → `Internal` with `error!(project_id = %project_id, key_version)`.
- [ ] `set_project_git_credential(pool, keyring, project_id, value: Zeroizing<String>, created_by: Option<Uuid>) -> Result<()>` creates the row with `orchestrator_only = true`, or replaces the value if the row exists (same AAD, fresh data key), in one transaction; empty or oversize value → `BadRequest`.
- [ ] `has_project_git_credential(pool, project_id) -> Result<bool>` for the `Project` DTO (`exists_by_scope_name`).
- [ ] The row appears in `GET /api/secrets?scope=project&scope_id=<pid>` as an ordinary orchestrator-only row (users may rotate or delete it through the secrets API) and is never injected into containers because of the flag.

## Implementation notes
- File: `orchestrator/src/secrets/git_credential.rs`; re-export from `orchestrator/src/secrets/mod.rs`.
- Reuse task 3's `find_by_scope_name`, `exists_by_scope_name`, `insert`, `find_row_for_update`, `update_value`, `insert_use` and task 2's `seal` / `open`.
- The provider in `git/` (Git operations epic) turns the returned PAT into `Authorization: Basic base64("x-access-token:" + PAT)`; this module returns the raw value only.
- Never log the value; span fields `project_id = %project_id`, `purpose = "git"`.

## Edge cases
- Project row deleted between lookup and use: the secret row is orphaned until the cleanup job removes it; returning it is harmless (project deletion cascades secrets in the Projects epic).
- `set_project_git_credential` racing a user `POST /api/secrets` of the same name: on `Conflict` retry once as a replace.
- A `GIT_CREDENTIAL` row that a user toggled to `orchestrator_only = false`: still read by the provider (the name, not the flag, is the contract); the resolver may then inject it if a profile lists it, which is the user's explicit choice.

## Testing
- Integration tests in `orchestrator/tests/secrets_git_credential.rs` via `TestApp` with a seeded project: absent → `None` and no use row; set then get returns the value and writes one `git` use row with the right `(session_id, user_id)` for each `GitUseContext` variant; set twice replaces (still one row, `updated_at` advanced, `data_key_wrapped` changed); `has_project_git_credential` before and after; the row is listed by the repository with `orchestrator_only = true` and `name = "GIT_CREDENTIAL"`; empty value → `BadRequest`.
- `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Git operations: mirror, clones, integration and REST API": `GitCredentialProvider::credential_for` calls `project_git_credential` with the right `GitUseContext` and builds the header.
- "Projects, agent profiles and shared directories": `POST /api/projects` calls `set_project_git_credential` and the DTO uses `has_project_git_credential`.