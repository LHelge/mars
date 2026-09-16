---
id: "2txez"
title: Add frontend types and services for projects, branches, shared dirs, profiles, sessions, events and git
status: open
priority: P0
created: "2026-09-16T20:40:42.646403323Z"
updated: "2026-09-16T20:51:52.315209963Z"
tags:
  - frontend
  - sessions
  - projects
  - git
depends_on:
  - "2f5u2"
  - h8kw9
parent: cgdc2
---

## Summary
Add the TypeScript mirrors of every API shape the project and session views consume and the `services/` modules that call their endpoints through `apiClient`. This is the typed foundation every other task in the epic imports; it contains no UI. Shapes mirror `SPEC.md` exactly, field names in `snake_case`, and the discriminated unions (`AgentEvent`, `SessionInput`, WebSocket messages, `MergeInput`) are written so the store and hooks can exhaustively `switch` on `kind`/`type`.

## Documents
- `SPEC.md` "Projects" table and the `Project`/`Branch` shapes; "Shared directories" table and `SharedDir`; "Agent profiles" table and `Profile`/`ProfileInput`; "Sessions" table and `Session`; "Git" table and `SessionBranch`, `Diff`, `MergeInput`; "Task states" `TaskState` (read-only use); "WebSocket: session stream" (server and client message tables, `SessionInput`); "AgentEvent" (full union); "REST API" error shape `{status, error}` plus `conflicts` on 422.
- `SPEC.md` "Frontend" structure: `services/ projects, profiles, sessions, tasks, taskStates, git`; rule "components never call `fetch`".
- `CLAUDE.md` "Frontend conventions" (types in `src/types/` mirror `SPEC.md`, `snake_case`; all calls through `services/apiClient.ts`).

## Acceptance criteria
- [ ] `frontend/src/types/project.ts` exports `ProjectStatus = "cloning" | "ready" | "error"`, `Project` (`id, name, remote_url, default_branch: string | null, status, status_message: string | null, last_fetched_at: string | null, max_attempts, created_at, has_credential`), `ProjectCreateInput` (`{name, remote_url, default_branch?, credential?}`), `ProjectUpdateInput` (`{name?, default_branch?, max_attempts?}`), `Branch` (`{name, kind: "head" | "upstream" | "session", commit, session_id?}`), `SharedDir` (`{name, container_path, created_at}`), `SharedDirInput`.
- [ ] `frontend/src/types/profile.ts` exports `ProfileKind = "conversational" | "ephemeral"`, `Profile` (all 17 fields from the SPEC line: `id, project_id, name, kind, backend, model: string | null, system_prompt: string | null, permission_mode, image, runtime: string | null, mcp_tools: string[], secrets: string[], serves_states: string[], partial_messages, idle_timeout_secs, is_default, created_at, updated_at`) and `ProfileInput` (same minus `id, project_id, is_default, created_at, updated_at`; `partial_messages?` optional so the backend default per kind applies).
- [ ] `frontend/src/types/session.ts` exports `SessionState = "creating" | "running" | "parked" | "done" | "failed"`, `Session` (all 23 fields: `id, project_id, profile_id, kind, created_by, title, task_id, handoff_id, state, base_ref, branch, container_id, cli_session_id, last_seq, last_activity_at, cost_usd, input_tokens, output_tokens, error, created_at, parked_at, ended_at`, nullable ones typed `| null`), `SessionCreateInput` (`{profile_id, base_ref?, title?, message?, task_id?}`), `SessionInput` (`{kind: "message", text} | {kind: "answer", reply_to: number, text}`), `EventsPage` (`{events: AgentEvent[], has_more: boolean}`), `SyncResult` (`{ref, commit}`).
- [ ] `frontend/src/types/agentEvent.ts` exports `AgentEventBase` (`seq, ts, parent_tool_use_id?, message_id?`) and the `AgentEvent` union with every kind and field exactly as in `SPEC.md` "AgentEvent": `init, user_message, text_delta, text, thinking, tool_call, tool_result, permission_denied, prompt, subagent_start, subagent_end, result, error, state_change, launch_warning, git, raw`. `AgentEventKind = AgentEvent["kind"]`.
- [ ] `frontend/src/types/sessionSocket.ts` exports `ServerMessage` union (`event`, `session`, `input_accepted {client_id, seq}`, `input_rejected {client_id, reason}`, `terminal_closed {exit_code}`, `error {message}`) and `ClientMessage` union (`input {client_id, input: SessionInput}`, `stop`, `terminal_open {cols, rows}`, `terminal_resize {cols, rows}`, `terminal_close`). Binary frames are not part of these unions.
- [ ] `frontend/src/types/git.ts` exports `SessionBranch` (`{session_id, ref, commit, ahead, behind, base, updated_at}`), `DiffFile` (`{path, status, additions, deletions}`), `Diff` (`{base, head, merge_base, files: DiffFile[], patch, truncated}`), `MergeInput = {target, message?} & ({source} | {task_id, handoff_id})`, `RebaseInput {branch, onto}`, `PushInput {ref, remote_branch?, force?}`, `PushResult {remote_branch, commit}`, `CommitResult {commit}`, `GitConflictError` (`{status: 422, error, conflicts: string[]}`).
- [ ] `frontend/src/types/taskState.ts` exports `TaskState` (`{id, project_id, name, kind: "queue" | "human" | "terminal", position, created_at}`) if the foundation epic did not already create it; otherwise reuse.
- [ ] `frontend/src/services/projects.ts`: `listProjects()`, `getProject(id)`, `createProject(input)`, `updateProject(id, input)`, `deleteProject(id)`, `retryClone(id)`, `fetchProject(id)` (POST `/projects/{id}/fetch`), `listBranches(id)`, `listSharedDirs(id)`, `createSharedDir(id, input)`, `clearSharedDir(id, name)`, `deleteSharedDir(id, name)`.
- [ ] `frontend/src/services/profiles.ts`: `listProfiles(pid)`, `getProfile(pid, id)`, `createProfile(pid, input)`, `updateProfile(pid, id, input)`, `deleteProfile(pid, id)`.
- [ ] `frontend/src/services/sessions.ts`: `listProjectSessions(pid, state?)`, `listSessions(state?)` (if the foundation dashboard did not add it), `getSession(id)`, `createSession(pid, input)`, `updateSession(id, {title})`, `deleteSession(id)`, `listEvents(id, {before?, limit?})` → `EventsPage`, `sendInput(id, input)` (POST, expects 202), `stopSession(id)` (202), `endSession(id)`, `retrySession(id, {message?})`, `syncSession(id)` → `SyncResult`, `listSessionTasks(id)` → `Task[]`.
- [ ] `frontend/src/services/git.ts`: `listSessionBranches(pid)`, `getDiff(pid, {head} | {handoff_id}, base?)`, `merge(pid, input)`, `rebase(pid, input)`, `push(pid, input)`.
- [ ] `frontend/src/services/taskStates.ts#listTaskStates(pid)` and `frontend/src/services/tasks.ts#getTask(pid, idOrNumber)` exist (create the read-only helpers only if the foundation epic did not; the task board epic owns every mutation in those modules).
- [ ] `apiClient` errors expose `status` and, for 422, `conflicts`; add an `isGitConflict(err)` type guard in `services/git.ts` if `apiClient`'s error class does not already carry the parsed body.
- [ ] `cd frontend && npm run lint && npx tsc -b && npm run build` pass.

## Implementation notes
- Files: `frontend/src/types/{project,profile,session,agentEvent,sessionSocket,git,taskState}.ts` re-exported from `frontend/src/types/index.ts`; `frontend/src/services/{projects,profiles,sessions,git,taskStates,tasks}.ts` re-exported from `frontend/src/services/index.ts`.
- Use `apiGet`/`apiPost`/`apiPut`/`apiPatch`/`apiDelete` from `services/apiClient.ts` (foundation epic). Query-string helpers: build with `URLSearchParams`, omit undefined values (`?state=` only when given; `?before=&limit=`).
- `verbatimModuleSyntax` is on: every type-only import is `import type`.
- Paths are the SPEC paths under `/api`: `/projects`, `/projects/{id}/branches`, `/projects/{pid}/shared-dirs[/{name}[/clear]]`, `/projects/{pid}/profiles[/{id}]`, `/projects/{pid}/sessions`, `/sessions[/{id}[/events|/input|/stop|/end|/retry|/sync|/tasks]]`, `/projects/{pid}/git/{session-branches|diff|merge|rebase|push}`, `/projects/{pid}/task-states`, `/projects/{pid}/tasks/{id}`.
- Keep the `Message` (folded transcript) type out of `types/`: it is store-internal and lives in `session/sessionStore.ts` (next task).

## Edge cases
- `POST /sessions/{id}/input` and `/stop` return 202 with no body; the helpers must not try to parse JSON.
- `DELETE` helpers return `void` on 204.
- `getDiff` accepts exactly one of `head`/`handoff_id`; type it as a union so callers cannot pass both.
- `Project.default_branch` is nullable while cloning; every consumer must handle `null`.
- Payload fields starting with `_` never reach the browser; do not model them.

## Testing
- No runtime tests for this task (pure types and thin wrappers); the type-check is the test. `cd frontend && npm run lint && npx tsc -b && npm run build` must pass. The next task's Vitest setup will import these types.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- Frontend foundation epic: `services/apiClient.ts` with `apiGet/apiPost/apiPut/apiPatch/apiDelete`, an error class carrying the HTTP `status` and parsed body; `types/` for `User`, `Task`, `SecretMeta`; possibly `services/tasks.ts` (dashboard) and `services/sessions.ts#listSessions` (dashboard). Extend rather than duplicate whatever exists.
- Repository scaffolding epic: `frontend/` with the directory barrels.