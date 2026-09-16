---
id: "7ac22"
title: Add shared-directory name and container-path validation and SharedDirRepository
status: open
priority: P2
created: "2026-09-16T20:29:32.014791104Z"
updated: "2026-09-16T20:51:51.896745441Z"
tags:
  - orchestrator
  - projects
depends_on:
  - z4u4e
parent: pkaee
---

## Summary
Deliver the shared-directory domain layer: the `SharedDir` model with the complete name and `container_path` validation from `SPEC.md`, and `SharedDirRepository` with list, insert (mapping both uniqueness constraints to distinct 409 messages) and delete. The launcher (Session lifecycle epic) reads the list at every launch, so the model must be the single place that decides whether a path is mountable.

## Documents
- `SPEC.md` "Shared directories (`/api/projects/{pid}/shared-dirs`)": `SharedDir` shape, `name` 1–64 chars `[a-z0-9][a-z0-9_-]*`, `container_path` rules (absolute, normalised, not `/data` or below, neither equal to nor an ancestor of `/session/work`, `/session/home`, `/session/log`, `/session/mcp.json`; may lie inside `/session/work`), 409 for a used name or path.
- `docs/data-model.md` `project_shared_dirs` (`PRIMARY KEY (project_id, name)`, `UNIQUE (project_id, container_path)`).
- `ARCHITECTURE.md` "Storage" → "Shared directories"; ADR 0015; `README.md` "Operating notes" table of recommended entries.

## Acceptance criteria
- [ ] `models/shared_dir.rs` defines `SharedDir { name, container_path, created_at }` (plus `project_id`, `#[serde(skip)]`), `SharedDirInput { name, container_path }` and `SharedDirError`.
- [ ] `name` validation: trimmed, 1–64 chars, matches `^[a-z0-9][a-z0-9_-]*$`.
- [ ] `container_path` validation, in this order with a distinct message each: non-empty and ≤ 4096 bytes; starts with `/`; no segment is empty (`//`), `.` or `..`; no trailing `/`; is not `/data` and does not start with `/data/`; is not equal to and is not an ancestor of any of `/session/work`, `/session/home`, `/session/log`, `/session/mcp.json` (so `/`, `/session`, `/session/work`, `/session/home`, `/session/log`, `/session/mcp.json` are all rejected); is not a descendant of `/session/mcp.json` (a file); may be a descendant of `/session/work`, `/session/home` or `/session/log`.
- [ ] `SharedDirRepository<'a>` provides `list(project_id) -> Vec<SharedDir>` ordered by `name`, `insert(tx, project_id, &SharedDirInput) -> SharedDir`, `get(project_id, name) -> Option<SharedDir>`, `delete(tx, project_id, name) -> bool`.
- [ ] Unique violations map by constraint name: primary key → `Error::Conflict("shared directory name already used")`, `project_shared_dirs_project_id_container_path_key` → `Error::Conflict("container path already used")`.
- [ ] `insert` returns `Error::NotFound` when the project does not exist (FK violation on `project_id` mapped, or an explicit existence check in the same transaction).
- [ ] `.sqlx/` refreshed and committed.

## Implementation notes
- Files: `orchestrator/src/models/shared_dir.rs`, `orchestrator/src/repositories/shared_dirs.rs`, `orchestrator/src/prelude/error.rs` (`#[from] SharedDirError`).
- Implement the path check on the string, not with `std::path::Path::components` alone, because normalisation must reject what `Path` would silently collapse; a helper `is_ancestor_or_equal(candidate, reserved)` compares segment vectors.
- The recommended README entries (`target → /session/work/target`, `cargo-registry → /session/home/.cargo/registry`, `npm-cache → /session/home/.npm`, `go-mod → /session/home/go/pkg/mod`, `go-build → /session/home/.cache/go-build`, `uv-cache → /session/home/.cache/uv`, `m2 → /session/home/.m2`, `gradle → /session/home/.gradle`) must all validate; use them as the positive test set.
- The launcher mounts shared directories "parents before children"; expose `SharedDir::sort_for_mount(&mut Vec<SharedDir>)` that orders by segment count then path, so the Session lifecycle epic does not re-derive it.

## Edge cases
- Two names differing only in case are distinct (`name` is lower-case only, so upper case is rejected outright).
- Paths that are byte-identical after trimming are duplicates; do not trim internal whitespace, reject it (`container_path` must not contain whitespace or control characters).
- `SPEC.md` does not explicitly forbid a descendant of `/session/mcp.json`; the mount can never succeed, so reject it with the same 400 and add the phrase "or below `/session/mcp.json`" to `SPEC.md` "Shared directories" in the same commit.

## Testing
- Unit tests: every rule above with at least one rejected and one accepted example, the README table as accepted cases, `sort_for_mount` ordering (`/session/work` mounts are owned by the launcher, but `/session/work/target` sorts after `/session/work/t`).
- Integration tests through `TestApp` at repository level: insert/list/get/delete; duplicate name → "shared directory name already used"; duplicate path under a new name → "container path already used"; insert for an unknown project → not found.
- `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` must pass.

## Documentation
- `SPEC.md` "Shared directories": add the "or below `/session/mcp.json`" clause (one phrase) in the same commit as the validator.

## Assumes from other epics
- "Database schema, models, repositories and test harness": `project_shared_dirs` migration, a `SharedDir` model skeleton with the path rules started (this task completes and tests them), `TestApp`.