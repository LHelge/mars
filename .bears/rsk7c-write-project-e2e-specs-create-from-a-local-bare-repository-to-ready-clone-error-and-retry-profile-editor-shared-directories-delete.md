---
id: rsk7c
title: "Write project E2E specs: create from a local bare repository to ready, clone error and retry, profile editor, shared directories, delete"
status: done
priority: P1
created: "2026-09-16T20:43:21.620546626Z"
updated: "2026-09-20T18:56:41.566816524Z"
tags:
  - frontend
  - projects
  - tests
depends_on:
  - ku8up
parent: "6s8j7"
attempts: 1
---

## Summary
Cover the "Projects" and "Agent profiles" feature paragraphs: creating a project from a `file://` bare repository through the UI and watching it go from `cloning` to `ready`, a project whose remote does not exist reaching `error` with a message and being retried, the ready project page (default branch, last fetch time, profiles, sessions, shared directories, states, board tabs), editing the default profile and creating an ephemeral one, shared-directory add/remove/clear including the refusal while a session runs, fetch-now, and project deletion.

## Documents
- `SPEC.md` "User-facing features", "Projects" and "Agent profiles" paragraphs.
- `SPEC.md` "Projects" table (`POST /projects` → 201 `status: cloning`; `Project = { id, name, remote_url, default_branch, status, status_message, last_fetched_at, max_attempts, created_at, has_credential }`; `POST /projects/{id}/retry-clone` only from `error`; `POST /projects/{id}/fetch`; `GET /projects/{id}/branches` → `Branch = { name, kind: "head" | "upstream" | "session", commit, session_id? }` with `main` and `origin/main`; `DELETE /projects/{id}` → 204, 409 while a session is `running` or `creating`; `PUT /projects/{id}` `{name?, default_branch?, max_attempts?}`, `max_attempts` 1–20).
- `SPEC.md` "Shared directories" table (`POST` `{name, container_path}` → 201, 400 invalid, 409 duplicate; `clear` → 204 or 409 while a session runs; `DELETE` → 204 or 409; `name` matches `[a-z0-9][a-z0-9_-]*`, `container_path` absolute and normalised, not under `/data`, not equal to or an ancestor of `/session/work`, `/session/home`, `/session/log`, `/session/mcp.json`).
- `SPEC.md` "Agent profiles" table (`Profile` fields; `ProfileInput`; `permission_mode` must be `bypass`; `serves_states` must be `queue` state names, default `["ready"]`; `DELETE` 409 if default or has sessions).
- `ARCHITECTURE.md` "Git model", "Project clone" (bare mirror, `ls-remote --symref` discovery, integration heads seeded from upstream branches).
- `SPEC.md` "Frontend" routes `/projects`, `/projects/:id` (tabs for sessions, board, profiles, shared directories, states and secrets).

## Acceptance criteria
- [ ] `frontend/tests/projects.spec.ts`; each test logs in a fresh user with `loginViaToken` and creates its own bare repository.
- [ ] `create a project from a bare repository and reach ready`: on `/projects` fill name and `remote_url` (`file://...`), submit; the list shows the project with `cloning`; within 60 s the row shows `ready` without a manual reload (the list polls or refetches; if the foundation uses a refetch interval, wait on it); the project page shows `default_branch: main`, a `last_fetched_at` timestamp, one profile named `default` with `is_default`, and empty sessions and shared-directory tabs.
- [ ] `clone failure shows error and retry works`: `remote_url` `file:///nonexistent/<hex>.git`; the project reaches `error` with a non-empty `status_message` visible on the page; create the bare repository at exactly that path with the helper, click "Retry clone", reach `ready`.
- [ ] `branches tab lists integration head and upstream ref`: after a `commitToBareRepo` and "Fetch now", the branch list shows `main` (kind `head`) unchanged and `origin/main` (kind `upstream`) at the new commit (compare with `gitRevParse(repo.path, "main")`).
- [ ] `edit the default profile and create an ephemeral one`: on the profiles tab open `default`, change name to `planner`, system prompt, `serves_states` to `["backlog"]`, toggle partial messages off, idle timeout to 300; save; reload; values persist. Create profile `oneshot` with kind `ephemeral`; the list shows both; deleting `planner` (the default) is refused with the 409 message shown; deleting `oneshot` succeeds.
- [ ] `profile validation errors are surfaced`: `serves_states` containing `nope` shows the 400 error text from the API; an unknown `mcp_tools` entry likewise.
- [ ] `shared directories: add, duplicate, invalid path, clear, remove`: add `target` at `/session/work/target` → listed; adding `target` again → 409 shown; adding `bad` at `/session/work` → 400 shown; `clear` → succeeds while no session runs; remove → gone. Assert the directory `<dataDir>/projects/<pid>/shared/target` exists after a session launch in a later spec (session task), not here.
- [ ] `shared directory actions are refused while a session runs`: launch a session with `launchSession` (default profile, stub), wait `running`; `clear` and remove show the 409 error and the row stays; end the session; remove succeeds.
- [ ] `project settings: rename and max_attempts`: change name and `max_attempts` to 5; header and value update; `max_attempts` 0 shows a validation error.
- [ ] `delete project`: refused with 409 while a session is `running` (message shown); after ending the session, delete succeeds, `/projects` no longer lists it, and `<dataDir>/projects/<pid>` is gone.

## Implementation notes
- Files: `frontend/tests/projects.spec.ts`.
- Use the helpers' `createBareRepo`, `commitToBareRepo`, `createProject` (for tests that do not test creation itself), `launchSession`/`waitForSessionState`, `api(...).post("/sessions/{id}/end")`.
- Sessions launched here use the project's `default` profile, whose image is `SESSION_IMAGE_DEFAULT` (the stub) because the stack sets it; do not edit the image in these tests.
- `test.setTimeout(120_000)` on tests that launch a container.
- The file-system assertions (`shared/target`, project directory removal) read `PLAYWRIGHT_DATA_DIR`; they are valid only because the orchestrator runs on the host in the E2E stack.

## Edge cases
- `cloning` may be too fast to observe on the list; assert `ready` as the terminal state and only assert `cloning` when the initial 201 response, captured with `page.waitForResponse`, says so.
- The `file://` remote for the failure test must not exist before creation and must be created before retry; use the helper's deterministic path option (`createBareRepo(name, { path })`; add that option if missing).
- `last_fetched_at` is null until the first fetch completes; wait for `ready` before asserting it.

## Testing
- The spec file; run twice against one stack to prove independence.
- Command: `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:e2e`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Frontend project and session views": `ProjectsPage`, `ProjectPage` tabs, `ProfileEditorPage`, shared-directory controls, fetch/retry/delete actions with API errors surfaced.
- "Projects, agent profiles and shared directories", "Git operations": the routes above, including `file://` remotes and the background clone job.