---
id: yay55
title: "Build the shared-directories tab: list, add with path validation, remove and clear with running-session refusals surfaced"
status: open
priority: P2
created: "2026-09-16T20:45:14.419557499Z"
updated: "2026-09-16T20:45:14.419557499Z"
tags:
  - frontend
  - projects
depends_on:
  - dxunp
parent: cgdc2
---

## Summary
Fill the `shared-dirs` tab of `ProjectPage`: the list of the project's shared directories with their container paths, an add form that mirrors the server's name and path validation for immediate feedback, and the `Clear` and `Remove` actions whose 409 refusals while a session is running are surfaced as disabled actions with an explanation. The README's per-ecosystem starting points are offered as one-click presets.

## Documents
- `SPEC.md` "User-facing features", Projects paragraph (shared directories are named directories every session mounts read-write at a container path of the user's choice; the project page lists, adds, removes and can empty one while no session is running).
- `SPEC.md` "Shared directories" table: `GET /projects/{pid}/shared-dirs` → `SharedDir[]`; `POST` `{name, container_path}` → 201 (400 invalid name or path; 409 name or path already used); `POST /projects/{pid}/shared-dirs/{name}/clear` → 204 (409 while any session is `running` or `creating`); `DELETE /projects/{pid}/shared-dirs/{name}` → 204 (same 409). `name` 1–64 chars `[a-z0-9][a-z0-9_-]*`; `container_path` absolute, normalised (no `.`, `..`, repeated or trailing slashes), not `/data` or below, neither equal to nor an ancestor of `/session/work`, `/session/home`, `/session/log`, `/session/mcp.json`; may lie inside `/session/work`.
- `README.md` "Operating notes", shared-directories table (Rust `target` → `/session/work/target`, `cargo-registry` → `/session/home/.cargo/registry`; Node `npm-cache` → `/session/home/.npm`; Go `go-mod`, `go-build`; Python `uv-cache`; JVM `m2`, `gradle`) and the note that emptying and removing are refused while a session runs.
- ADR 0015.

## Acceptance criteria
- [ ] `frontend/src/pages/project/SharedDirsTab.tsx` lists `listSharedDirs(pid)` (key `["projects", pid, "shared-dirs"]`): `name`, `container_path` (monospace), `created_at`, actions `Clear` and `Remove`.
- [ ] `Clear` and `Remove` confirm, call `clearSharedDir` / `deleteSharedDir`, invalidate the list, and on 409 show the server message (`refused while a session of the project is running`) inline; when the sessions list (`["projects", pid, "sessions"]`, if cached) shows a `running`/`creating` session the buttons are pre-disabled with the tooltip `A session is running`.
- [ ] Add form: `name` and `container_path` fields validated client-side by `validateSharedDir(name, path)` in `frontend/src/utils/sharedDir.ts` implementing exactly the SPEC rules (name regex and length; absolute; no `.`/`..` segments; no `//`; no trailing slash except root, which is itself rejected; not `/data` or under it; not equal to or an ancestor of the four reserved paths), with per-field messages; the server's 400/409 messages are still shown if they differ.
- [ ] Preset chips fill the form with the README pairs (`target` → `/session/work/target`, `cargo-registry` → `/session/home/.cargo/registry`, `npm-cache` → `/session/home/.npm`, `go-mod` → `/session/home/go/pkg/mod`, `go-build` → `/session/home/.cache/go-build`, `uv-cache` → `/session/home/.cache/uv`, `m2` → `/session/home/.m2`, `gradle` → `/session/home/.gradle`).
- [ ] Help text states that the list is read at each launch and running sessions keep the mounts they started with.
- [ ] `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:unit` pass.

## Implementation notes
- Files: `frontend/src/pages/project/SharedDirsTab.tsx`, `frontend/src/pages/project/SharedDirForm.tsx`, `frontend/src/utils/sharedDir.ts`, `frontend/src/utils/sharedDir.test.ts`.
- Register the panel in `ProjectTabs` for `tab=shared-dirs`.
- Ancestor check: `reserved === path || reserved.startsWith(path + "/")` for each of `/session/work`, `/session/home`, `/session/log`, `/session/mcp.json`; `/data` check: `path === "/data" || path.startsWith("/data/")`.

## Edge cases
- Name uniqueness and path uniqueness are both 409s with different messages; show them on the matching field by inspecting the message (`name` vs `path`) with a generic fallback.
- Clearing a directory that does not yet exist on disk (created lazily at launch) returns 204; nothing special client-side.
- Windows-style or relative paths are rejected client-side with `must be an absolute path`.

## Testing
- Vitest `sharedDir.test.ts`: valid presets pass; `/session/work` (equal), `/session` (ancestor), `/data/x`, `/a/../b`, `/a//b`, `/a/`, `target` (relative), `Target` (uppercase name), 65-char name all fail with the expected message; `/session/work/target` passes.
- Command: `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:unit`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- Frontend foundation epic: form and alert components.
- Projects epic (backend): shared-directory endpoints with the documented statuses.