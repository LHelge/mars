---
id: "98che"
title: Add the shared-directories route module with clear and delete refused while sessions run
status: done
priority: P2
created: "2026-09-16T20:33:15.407841193Z"
updated: "2026-09-18T11:59:14.835341414Z"
tags:
  - orchestrator
  - projects
depends_on:
  - "7ac22"
  - maq3y
  - cgj5v
parent: pkaee
assignee: claude-opus-subagent
attempts: 1
---

## Summary
Create `routes/shared_dirs.rs` exporting `routes() -> Router<AppState>` with list, create, clear and delete under `/projects/{pid}/shared-dirs`. Create writes only the row (the directory is created lazily at launch); clear empties the on-disk directory and delete removes the row and the directory, both refused with 409 while any session of the project is `running` or `creating`. Filesystem work goes through `ProjectLayout` and happens after the short, project-locked check transaction.

## Documents
- `SPEC.md` "Shared directories (`/api/projects/{pid}/shared-dirs`)": all four rows (201 / 400 / 409 on create; 204 and 409 while running for clear and delete), `SharedDir` shape, "The list is read at each launch; a running session keeps the mounts it started with."
- `ARCHITECTURE.md` "Storage" → "Shared directories" (created at launch if missing; clear and delete refused while `running` or `creating` because a build may hold files open).
- `docs/data-model.md` `project_shared_dirs` ("directories themselves are created lazily at launch and removed with the row or the project").
- `README.md` "Operating notes" (empty from the project page when disk gets tight; both actions refused while a session runs); ADR 0015.

## Acceptance criteria
- [ ] `GET /api/projects/{pid}/shared-dirs` → 200 `SharedDir[]` (`{name, container_path, created_at}`) ordered by name; 404 unknown project.
- [ ] `POST /api/projects/{pid}/shared-dirs` with `{name, container_path}` → 201 `SharedDir`; 400 with the model's message for an invalid name or path; 409 "shared directory name already used" or "container path already used"; 404 unknown project. No directory is created.
- [ ] `POST /api/projects/{pid}/shared-dirs/{name}/clear` → 204 after emptying `DATA_DIR/projects/<pid>/shared/<name>` (a missing directory is still 204); 409 "project has running sessions" while `count_live_for_project > 0`; 404 when the row does not exist.
- [ ] `DELETE /api/projects/{pid}/shared-dirs/{name}` → 204 after deleting the row and removing the directory recursively (missing directory is fine); 409 while sessions run; 404 unknown row.
- [ ] Sequence for clear and delete: `BEGIN` → `ProjectRepository::lock_for_update(pid)` (false → 404) → `SharedDirRepository::get` (none → 404) → `count_live_for_project` (> 0 → rollback, 409) → for delete only: `SharedDirRepository::delete` → `COMMIT` → filesystem operation through `ProjectLayout`. A filesystem failure after commit is logged with `project_id`, `name` and the path and answered 500 with the generic message for clear (the user can retry), and 204 for delete (the row is gone; leftovers are an operator concern).
- [ ] Path segment `{name}` is validated with the model's name rule before any lookup (400 otherwise), so `..` can never reach the filesystem helper.
- [ ] 401 without a token and 403 under `must_change_password` for every endpoint.

## Implementation notes
- Files: `orchestrator/src/routes/shared_dirs.rs` (new), `orchestrator/src/routes/mod.rs` (mount).
- Reuse `SharedDirRepository`, `SessionRepository::count_live_for_project`, `ProjectLayout::clear_shared_dir` / `remove_shared_dir`.
- The running-session check is a database check under the project row lock; the launcher's session insert commits before it starts creating directories, so a launch that committed first is seen. A launch that commits after the check may create the directory again while it is being emptied; both sides tolerate that (`ensure_shared_dir` is idempotent, `clear` removes whatever exists). Document this narrow window in a code comment; it is within the documented contract.
- No `task_events`, no git lock: shared directories are not part of the repository.

## Edge cases
- Clearing a directory containing a symlink to outside the tree removes the link, not its target (see the layout task).
- Creating a shared directory for a project in `cloning` or `error` is allowed; it only takes effect at launch.
- Two shared dirs with nested container paths (`/session/work/target` and `/session/work/target/debug`) are allowed by the rules; the launcher mounts parents first.
- A name that is valid but whose directory was never created (no launch since) → clear and delete are 204 with no filesystem change.

## Testing
- Integration tests in `orchestrator/tests/shared_dirs.rs` via `TestApp` (project via `POST /projects`, no clone needed):
  - create `target → /session/work/target` → 201; list shows it; create with the same name → 409 name message; different name same path → 409 path message; bad name (`Target`, `-x`, 65 chars) → 400; bad paths (`relative`, `/session/work`, `/session`, `/data/x`, `/a//b`, `/a/../b`, `/a/`) → 400 each; README table entries all 201.
  - clear: create the directory and files under `DATA_DIR/projects/<pid>/shared/target/` directly, `clear` → 204, directory exists and is empty; with a `running` session row → 409 and the files remain; `creating` → 409; `parked` → 204; unknown name → 404; missing directory → 204.
  - delete: → 204, row gone, directory gone; with a running session → 409 and row and directory remain; unknown → 404; `{name}` of `..` → 400.
  - 401 for each endpoint without a token; unknown project → 404.
- `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` must pass.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Authentication, users, invites and email": the current-user extractor and gate.
- "Session lifecycle": the launcher reads `SharedDirRepository::list` at each launch, calls `ProjectLayout::ensure_shared_dir` and mounts `SharedDir::sort_for_mount` order; nothing in this task depends on it at runtime.