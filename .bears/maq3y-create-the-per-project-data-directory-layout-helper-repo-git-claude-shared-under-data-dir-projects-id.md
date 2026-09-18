---
id: maq3y
title: Create the per-project data-directory layout helper (repo.git, claude/, shared/) under DATA_DIR/projects/<id>/
status: done
priority: P1
created: "2026-09-16T20:27:57.433928689Z"
updated: "2026-09-18T09:36:32.947130532Z"
tags:
  - orchestrator
  - projects
  - docs
depends_on:
  - z4u4e
parent: pkaee
assignee: claude-opus-subagent
attempts: 1
---

## Summary
Introduce a `projects` module in the orchestrator with a `ProjectLayout` helper that knows every on-disk path belonging to a project (`repo.git`, `claude/`, `shared/`, `shared/<name>`) and performs the filesystem operations this epic needs: create the layout, remove it entirely, and empty or remove a single shared directory. The clone job, project deletion and the shared-directory endpoints all go through this one helper so path construction is never duplicated. The module is new relative to the binding tree in `ARCHITECTURE.md`, "Orchestrator internals", so that tree is updated in the same commit.

## Documents
- `ARCHITECTURE.md` "Storage" (directory tree, "Per-project CLI state", "Shared directories", deletion paragraph) and "Orchestrator internals" (module tree, `AppState`, `Config`).
- `README.md` "Configuration" rows `DATA_DIR`, `DATA_DIR_HOST`; "Operating notes" bullets on removing a project and shared directories.
- `docs/data-model.md` `projects` (`id` is the directory name), `project_shared_dirs` (directories created lazily at launch, removed with the row or the project).
- ADR 0015.

## Acceptance criteria
- [ ] `orchestrator/src/projects/mod.rs` exists, does `use crate::prelude::*`, and exports `layout::ProjectLayout`.
- [ ] `ProjectLayout::new(data_dir: &Path, project_id: Uuid)` exposes `root()` = `DATA_DIR/projects/<id>`, `repo_git()` = `root/repo.git`, `claude_dir()` = `root/claude`, `shared_root()` = `root/shared`, `shared_dir(name)` = `root/shared/<name>`, and the matching host-path variants built from `DATA_DIR_HOST` (`repo_git_host()`, `claude_dir_host()`, `shared_dir_host(name)`) for the launcher's bind mounts.
- [ ] `ensure_created()` creates `root`, `claude/` and `shared/` (`create_dir_all`, idempotent) but not `repo.git` (git init creates it).
- [ ] `remove_all()` removes `root` recursively; a missing root is not an error.
- [ ] `clear_shared_dir(name)` removes every entry inside `shared/<name>` and leaves the directory itself in place; a missing directory is a no-op success.
- [ ] `remove_shared_dir(name)` removes `shared/<name>` recursively; missing is a no-op success.
- [ ] `ensure_shared_dir(name)` creates `shared/<name>` if missing (for the launcher's lazy creation).
- [ ] `shared_dir(name)` refuses (returns `Error::BadRequest`) any `name` that does not match `[a-z0-9][a-z0-9_-]*` or exceeds 64 characters, as a defence in depth against path escape even though the model validates first.
- [ ] `ARCHITECTURE.md` "Orchestrator internals" tree gains `projects/   project layout on /data, clone job, deletion`.

## Implementation notes
- Files: `orchestrator/src/projects/mod.rs`, `orchestrator/src/projects/layout.rs`; register `pub mod projects;` in `lib.rs`.
- `Config` (prelude, delivered by the scaffolding/schema epics) already carries `data_dir: PathBuf` and `data_dir_host: PathBuf`; add a convenience `Config::project_layout(&self, id: Uuid) -> ProjectLayout`.
- Use `tokio::fs` for all operations; every function returns `Result<()>` mapping `std::io::Error` to `Error::Internal` with the path in a structured `tracing::error!(path = %p.display(), ...)` field, never in the returned message.
- The session directories (`DATA_DIR/sessions/<sid>/`) belong to the Session lifecycle epic; do not model them here. Project deletion (later task) removes them by iterating the project's session ids and calling a `remove_dir_all` on `DATA_DIR/sessions/<sid>`; expose a small `session_dir(data_dir, session_id)` free function here so both epics agree on the path.
- No database access and no locking in this task; the callers hold the per-project git lock.

## Edge cases
- Symlinks inside a shared directory: `remove_dir_all` on the entry must not follow symlinks out of the directory (Rust's `remove_dir_all` does not follow symlinks; `clear_shared_dir` iterates `read_dir` and removes each entry with `remove_dir_all` for directories and `remove_file` for files and symlinks).
- `ensure_created` on an existing layout is a no-op; partially created layouts from an interrupted clone are completed, not errors.
- Ownership: the orchestrator creates directories as its own uid; under Podman `keep-id` that is uid 1000 in the container, under Docker the orchestrator already runs as uid 1000 (`ARCHITECTURE.md` "Uid contract"). No `chown` is performed.

## Testing
- Unit tests in `layout.rs` using `tempfile::tempdir()`: path construction for both orchestrator and host variants; `ensure_created` idempotency; `clear_shared_dir` removes files, nested dirs and dangling symlinks but keeps the directory; `remove_shared_dir` and `remove_all` succeed on missing paths; `shared_dir("../x")` and `shared_dir("Bad")` are rejected.
- `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` must pass.

## Documentation
- `ARCHITECTURE.md` "Orchestrator internals": add the `projects/` module line to the tree (same commit).

## Assumes from other epics
- "Repository scaffolding, tooling and CI" / "Database schema, models, repositories and test harness": the crate skeleton, `prelude` (`Config` with `data_dir` and `data_dir_host`, `Error`, `Result`) and `tempfile` as a dev-dependency exist.