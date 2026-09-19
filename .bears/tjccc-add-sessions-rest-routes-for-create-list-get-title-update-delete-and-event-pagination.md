---
id: tjccc
title: Add sessions REST routes for create, list, get, title update, delete and event pagination
status: in_progress
priority: P1
created: "2026-09-16T20:33:54.133596109Z"
updated: "2026-09-19T00:35:04.343593262Z"
tags:
  - orchestrator
  - sessions
depends_on:
  - "9wxhs"
parent: s52qg
attempts: 1
---

## Summary
Create `orchestrator/src/routes/sessions.rs` exporting `routes() -> Router<AppState>` with the resource endpoints of the Sessions table: `GET /projects/{pid}/sessions`, `POST /projects/{pid}/sessions` (without `task_id`, which the launch-for-task task adds), `GET /sessions`, `GET /sessions/{id}`, `PUT /sessions/{id}`, `DELETE /sessions/{id}` and `GET /sessions/{id}/events`. `POST` validates the project state, profile, ephemeral prompt rule and base ref, generates the MCP token, inserts the row with the title-defaulting rule, and hands the raw token to the launcher.

## Documents
- `SPEC.md` "REST API" (status table, error body `{status, error}`, 201 for creates, 204 for deletes), "Sessions" table rows for `GET /projects/{pid}/sessions?state=`, `POST /projects/{pid}/sessions` (`{profile_id, base_ref?, title?, message?, task_id?}` → 201 `state: creating`; 400 if `base_ref` does not resolve in the mirror; 400 for an ephemeral profile with neither `task_id` nor `message`; 409 if the project is not `ready`), `GET /sessions?state=`, `GET /sessions/{id}`, `PUT /sessions/{id} {title}`, `DELETE /sessions/{id}` (must be `done` or `failed`), `GET /sessions/{id}/events?before=&limit=` → `{events, has_more}`; the `Session` DTO; title rule ("`title`, when omitted, is the task's title, else the first line of `message` truncated to 80 characters, else null"); "Projects" (default session base is the integration head named by `default_branch`; explicit upstream-tracking ref, tag, session ref or commit id allowed); "Authentication" (`must_change_password` gate; 401 rules).
- `ARCHITECTURE.md` "Launch sequence" (API generates the token, inserts `creating`, returns 201, spawns the owner with the launch token never returned to the UI), "MCP design" (token generated before the row insert on first creation).
- `docs/data-model.md` `sessions` (`kind` copied from the profile, `branch = session/<id>`, `created_by`).
- `CLAUDE.md` "API conventions", "Backend conventions" (routes one module per resource, DTOs private to the module).

## Acceptance criteria
- [ ] `GET /api/projects/{pid}/sessions?state=` → 200 `Session[]` newest first; unknown project → 404; `state` not one of the five values → 400 `unknown state`.
- [ ] `GET /api/sessions?state=` → 200 `Session[]` across all projects (dashboard); same `state` validation.
- [ ] `POST /api/projects/{pid}/sessions` body `{profile_id, base_ref?, title?, message?}`: project missing → 404; `project.status != ready` → 409 `project is not ready`; profile not in this project → 400 `unknown profile`; ephemeral profile with neither `task_id` nor `message` → 400 `an ephemeral session needs a task_id or a message`; `title` given → `validate_title` (400); `base_ref` defaults to `project.default_branch`; the ref is resolved in the project repository under the project git lock (unresolvable → 400 `base_ref does not resolve`); `McpToken::generate()`; insert `NewSessionRow { kind: profile.kind, created_by: caller, title: title.or(default_title(None, message)), base_ref: as given (name, not the resolved commit), branch: session/<id>, mcp_token_hash }`; commit; `launcher.launch(id, LaunchMode::Fresh { token, first_message: message })`; respond 201 with the `Session` (`state: creating`). The first message is delivered as the first queued input (conversational) or as the `-p` prompt (ephemeral) by the launcher; it is recorded as a `user_message` by the owner, not by the route.
- [ ] `GET /api/sessions/{id}` → 200 `Session` or 404.
- [ ] `PUT /api/sessions/{id}` `{title}` → 200 `Session`; 400 for an invalid title; 404.
- [ ] `DELETE /api/sessions/{id}` → 204 via `SessionService::delete`; 409 unless `done`/`failed`; 404.
- [ ] `GET /api/sessions/{id}/events?before=<seq>&limit=<n>` → 200 `{ events: AgentEvent[], has_more: bool }` newest-last ending just before `before`; `limit` default 100, 400 when `limit` is 0 or > 500 or `before` < 1; `_`-prefixed payload fields never appear; 404 for an unknown session.
- [ ] All routes require a JWT (401 otherwise) and honour the `must_change_password` 403 gate through the shared auth extractor; the `Session` DTO never includes `mcp_token_hash`.
- [ ] The router is nested under `/api` in the app router next to the other resource routers.

## Implementation notes
- Files: `orchestrator/src/routes/sessions.rs`, `orchestrator/src/routes/mod.rs`.
- DTOs private to the module: `CreateSessionInput`, `UpdateSessionInput`, `EventsQuery`, `StateQuery`; the response type is `models::session::Session`.
- Base-ref resolution: call the git epic's `resolve_ref(project, base_ref)` under the project git lock for validation only; the launcher resolves again when cloning (the mirror may have been fetched in between; the route's check exists to give 400 instead of a failed session).
- `message` is not stored on the row; pass it to the launcher in `LaunchMode::Fresh`.
- Errors come from `Error`'s `IntoResponse`; use `Error::Conflict`/`BadRequest` with the messages above.

## Edge cases
- `project.default_branch` is null (cannot happen when `ready`, but guard it): 409 `project is not ready`.
- `base_ref` given as `origin/<branch>`, a tag, `refs/sessions/<id>` or a 40-hex commit id all pass validation when present in the mirror.
- Concurrent creates on one project are independent sessions; no lock beyond the git lock during resolution.
- `GET /sessions/{id}/events` on a session with zero events → `{events: [], has_more: false}`.

## Testing
- `orchestrator/tests/sessions_api.rs` with `TestApp` (mock engine, real bare repository, ready project with the default profile): for every endpoint the happy path, 401 unauthenticated, 404 unknown id; `POST` 409 on a `cloning` project, 400 on an unresolvable `base_ref`, 400 ephemeral without message, 400 profile from another project; title defaulting: explicit title kept, `message` "Fix login\nmore" → `Fix login`, an 81+ character first line truncated to 80, no message → null; created row has `branch = session/<id>`, `kind` from the profile, `created_by` = caller, `state = creating`, `mcp_token_hash` set and absent from the JSON; the mock engine records one create after the response; `state` filter on both list endpoints and 400 on a bogus value; `DELETE` 409 on a `running` session and 204 on `failed` with the directory gone; events pagination: seed 7 events, `limit=3` → seqs 5,6,7 with `has_more: true`; `before=5&limit=3` → 2,3,4 with `has_more: true`; `before=2` → 1 with `has_more: false`; `limit=501` → 400; `_offset` stripped.
- `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Documentation
- none: implements the documented contract as written (the 1–200 title rule is documented by the model task).

## Assumes from other epics
- "Authentication, users, invites and email": the JWT extractor / `Claims` and the password-change gate middleware.
- "Projects, agent profiles and shared directories": `ProjectRepository`, `ProfileRepository` and a ready project fixture in tests.
- "Git operations": `resolve_ref` and the project git lock.