# Functional specification, v1

This is the contract for the first release: what a user can do, every endpoint and stream, the event schemas the frontend consumes, the MCP tools agents call, and what v1 deliberately does not do. Each section is written so that an implementation task can cite it. Design rationale lives in `ARCHITECTURE.md` and `docs/decisions/`; the schema in `docs/data-model.md`.

## User-facing features

**Login and invites.** There is no self-registration. The database is seeded with one administrator (`admin`, default password in `README.md`) who must change the password at first login before doing anything else. Admins invite people by email: the invitee receives a link (sent through Resend, or written to the orchestrator log when no API key is configured), opens it, chooses a username and password, and is logged in. Invites expire after 7 days and can be revoked. Login returns a 15-minute JWT access token in the response body and a 30-day refresh token in an HTTP-only cookie. Password reset works the same way as invites: a link by email, a single-use token.

**Projects.** A user creates a project from a remote URL and, for a private repository, a personal access token. Creation returns immediately with `status: cloning`; the project list shows progress and turns `ready` or `error`. A ready project shows its default branch, last fetch time, its profiles, sessions and task board. Deleting a project deletes its sessions, tasks, secrets and mirror.

**Agent profiles.** Every project starts with a `default` conversational profile using the built-in Claude image. Users can edit a profile's name, model, system prompt, image, runtime, MCP tool allow-list, declared secrets, partial-message flag (default on for conversational, off for ephemeral) and idle timeout, and create further conversational profiles. Ephemeral profiles can be created and edited but cannot be launched from the UI in v1.

**Sessions.** A user launches a session from a profile and a base ref (default: the project's default branch), optionally with a first message. The session view shows the full transcript with per-tool rendering and nested subagents, a composer for sending messages (including mid-turn), a stop button, and session metadata (state, branch, container, CLI session id). Sessions keep running when nobody is watching; anyone opening a session later sees everything that happened. A parked session looks like a running one that is waiting; sending a message relaunches it. Users can end a session, sync its branch into the mirror, and open a terminal into a running session's container.

**Task board.** Per project, a board of tasks with states `blocked`, `ready`, `in_progress`, `needs_human`, `done`, dependencies, comments, role, assignee, lease holder, and links to the sessions that touched each task. Users create, edit, comment, add and remove dependencies, release leases, and move tasks between states. Changes made by agents appear live.

**Git operations.** From a session or from the project page, a user can list session branches in the mirror, merge a session branch into a target branch, rebase a session branch onto a target, and push a branch upstream. Conflicts are reported with the conflicting paths.

**Secrets.** A write-only manager at global, project and user scope: create, replace, rename, delete, and list names with metadata (scope, created, updated, orchestrator-only, key version, last use). The value is never shown after entry.

**Users (admin).** List and delete users, toggle admin, send and revoke invites.

## Authentication

Identical in shape to a conventional JWT plus refresh cookie scheme:

- `POST /api/auth/login` and `POST /api/auth/accept-invite` return `{ user, access_token }` and set the `refresh_token` cookie (`HttpOnly`, `SameSite=Lax`, `Path=/`, `Secure` when `PUBLIC_URL` is https).
- The access token is a JWT signed with `JWT_SECRET`, claims `sub` (user id), `username`, `admin`, `must_change_password`, `iat`, `exp`. It is sent as `Authorization: Bearer`.
- While `must_change_password` is true, every endpoint other than `POST /auth/login`, `POST /auth/logout`, `POST /auth/refresh`, `GET /users/me` and `POST /users/{id}/password` answers 403 with error `password change required`. The frontend routes such a user to the change-password page. Changing the password clears the flag and issues a fresh token pair.
- `POST /api/auth/refresh` rotates the refresh token (old one revoked) and returns a new pair.
- WebSocket and SSE endpoints cannot receive headers from the browser, so they accept the access token as `?token=` and validate it exactly like the header. nginx must not log query strings for those locations.
- The MCP listener uses per-session bearer tokens, never user JWTs (`ARCHITECTURE.md`, "MCP design").

## REST API

All routes are under `/api`. Responses are bare JSON: arrays for lists, objects for single resources. Creates return `201` with the created resource; deletes return `204`; everything else `200`. Errors are `{ "status": <u16>, "error": "<message>" }` with the same status code on the response.

| Status | Used for |
| --- | --- |
| 400 | Validation failures and malformed input. |
| 401 | Missing or invalid token. |
| 403 | Authenticated but not permitted (non-admin on admin routes, tool not allowed for profile). |
| 404 | Unknown resource. |
| 409 | State conflicts: claim lost, session not in a state that accepts the action, duplicate name, dependency cycle. |
| 422 | Git operation failed with conflicts; body includes `conflicts: string[]`. |
| 500 | Unexpected. Never carries internal detail. |

Timestamps are RFC 3339 strings. Ids are UUID strings.

### Auth (`/api/auth`)

| Method | Path | Auth | Body → Response |
| --- | --- | --- | --- |
| POST | `/auth/login` | — | `{username, password}` → `{user, access_token}` |
| POST | `/auth/refresh` | cookie | — → `{user, access_token}` |
| POST | `/auth/logout` | cookie | — → 204 |
| GET | `/auth/invite/{token}` | — | → `{email, admin, expires_at}` (400 if expired, used or unknown) |
| POST | `/auth/accept-invite` | — | `{token, username, password}` → `{user, access_token}` (201) |
| POST | `/auth/request-password-reset` | — | `{identifier}` → 204 (always) |
| POST | `/auth/reset-password` | — | `{token, password}` → 204 |

### Users (`/api/users`)

| Method | Path | Auth | Body → Response |
| --- | --- | --- | --- |
| GET | `/users/me` | JWT | → `User` |
| GET | `/users` | admin | → `User[]` |
| GET | `/users/{id}` | JWT | → `User` |
| PUT | `/users/{id}` | admin | `{username, admin}` → `User` |
| DELETE | `/users/{id}` | admin | → 204 (409 for the last admin or yourself) |
| POST | `/users/{id}/password` | JWT (self) or admin | `{current_password?, password}` → `{user, access_token}` (self) or 204 (admin) |
| GET | `/users/invites` | admin | → `Invite[]` (open invites) |
| POST | `/users/invites` | admin | `{email, admin?}` → `Invite` (201; 409 if the email has a user or an open invite) |
| DELETE | `/users/invites/{id}` | admin | → 204 (revoke) |
| POST | `/users/invites/{id}/resend` | admin | → `Invite` (new token and expiry, email sent again) |

`User = { id, username, email, admin, must_change_password, created_at }`. `Invite = { id, email, admin, invited_by, expires_at, created_at }`; the token is only ever in the email. Changing your own password returns a fresh token pair because the `must_change_password` claim changes.

### Projects (`/api/projects`)

| Method | Path | Auth | Body → Response |
| --- | --- | --- | --- |
| GET | `/projects` | JWT | → `Project[]` |
| POST | `/projects` | JWT | `{name, remote_url, default_branch?, credential?}` → `Project` (201, `status: cloning`) |
| GET | `/projects/{id}` | JWT | → `Project` |
| PUT | `/projects/{id}` | JWT | `{name?, default_branch?}` → `Project` |
| DELETE | `/projects/{id}` | JWT | → 204 (refused with 409 while any session is `running` or `creating`) |
| POST | `/projects/{id}/retry-clone` | JWT | → `Project` (only from `error`) |
| POST | `/projects/{id}/fetch` | JWT | → `Project` (runs a mirror fetch now) |
| GET | `/projects/{id}/branches` | JWT | → `Branch[]` (mirror heads and session refs) |

`Project = { id, name, remote_url, default_branch, status, status_message, last_fetched_at, created_at, has_credential }`. `credential` on create is stored as the project-scoped orchestrator-only secret `GIT_CREDENTIAL` and is never returned. `Branch = { name, kind: "head" | "session", commit, session_id? }`.

### Agent profiles (`/api/projects/{pid}/profiles`)

| Method | Path | Auth | Body → Response |
| --- | --- | --- | --- |
| GET | `/projects/{pid}/profiles` | JWT | → `Profile[]` |
| POST | `/projects/{pid}/profiles` | JWT | `ProfileInput` → `Profile` |
| GET | `/projects/{pid}/profiles/{id}` | JWT | → `Profile` |
| PUT | `/projects/{pid}/profiles/{id}` | JWT | `ProfileInput` → `Profile` |
| DELETE | `/projects/{pid}/profiles/{id}` | JWT | → 204 (409 if default or has sessions) |

`Profile = { id, project_id, name, kind, backend, model, system_prompt, permission_mode, image, runtime, mcp_tools, secrets, partial_messages, idle_timeout_secs, is_default, created_at, updated_at }`. `ProfileInput` is the same without ids and timestamps; `permission_mode` must be `bypass`; `mcp_tools` entries must be known tool names.

### Sessions (`/api/projects/{pid}/sessions`, `/api/sessions/{id}`)

| Method | Path | Auth | Body → Response |
| --- | --- | --- | --- |
| GET | `/projects/{pid}/sessions` | JWT | `?state=` → `Session[]` |
| POST | `/projects/{pid}/sessions` | JWT | `{profile_id, base_ref?, title?, message?}` → `Session` (201, `state: creating`; 400 if `base_ref` does not resolve in the mirror; 409 for an `ephemeral` profile in v1) |
| GET | `/sessions/{id}` | JWT | → `Session` |
| PUT | `/sessions/{id}` | JWT | `{title}` → `Session` |
| DELETE | `/sessions/{id}` | JWT | → 204 (must be `done` or `failed`; removes the session directory) |
| GET | `/sessions/{id}/events` | JWT | `?before=<seq>&limit=<n≤500>` → `{events: AgentEvent[], has_more}` newest-last, ending just before `before` |
| POST | `/sessions/{id}/input` | JWT | `SessionInput` → 202 (same as sending over the socket; relaunches if parked) |
| POST | `/sessions/{id}/stop` | JWT | → 202 (SIGINT then SIGTERM after grace) |
| POST | `/sessions/{id}/end` | JWT | → `Session` (stop, fetch-back, `done`) |
| POST | `/sessions/{id}/retry` | JWT | → `Session` (from `failed`: `parked`, then relaunch if `message` given) |
| POST | `/sessions/{id}/sync` | JWT | → `{ref, commit}` (fetch the session branch into the mirror) |
| GET | `/sessions/{id}/tasks` | JWT | → `Task[]` touched by this session |

`Session = { id, project_id, profile_id, created_by, title, state, base_ref, branch, container_id, cli_session_id, last_seq, last_activity_at, error, created_at, parked_at, ended_at }`.

### Tasks (`/api/projects/{pid}/tasks`)

| Method | Path | Auth | Body → Response |
| --- | --- | --- | --- |
| GET | `/projects/{pid}/tasks` | JWT | `?state=&role=&priority=` → `Task[]` ordered by priority, then number |
| POST | `/projects/{pid}/tasks` | JWT | `{title, description?, priority?, role?, depends_on?: id[]}` → `Task` |
| GET | `/projects/{pid}/tasks/{id}` | JWT | → `TaskDetail` |
| PUT | `/projects/{pid}/tasks/{id}` | JWT | `{title?, description?, priority?, role?, state?, assignee_user_id?}` → `Task` |
| DELETE | `/projects/{pid}/tasks/{id}` | JWT | → 204 |
| POST | `/projects/{pid}/tasks/{id}/dependencies` | JWT | `{depends_on: id}` → `Task` (409 on cycle) |
| DELETE | `/projects/{pid}/tasks/{id}/dependencies/{dep}` | JWT | → `Task` |
| POST | `/projects/{pid}/tasks/{id}/comments` | JWT | `{body}` → `Comment` |
| POST | `/projects/{pid}/tasks/{id}/release` | JWT | → `Task` (clears lease, state to `ready`) |
| GET | `/projects/{pid}/tasks/stream` | JWT (`?token=`) | SSE of `TaskEvent`; see "SSE" |

`Task = { id, project_id, number, title, description, state, priority, role, assignee_user_id, assignee_session_id, lease_holder_session_id, lease_expires_at, needs_human_reason, depends_on: id[], blocks: id[], created_at, updated_at, closed_at }`. `TaskDetail = Task & { comments: Comment[], sessions: {session_id, first_touched_at, last_touched_at}[] }`. `Comment = { id, task_id, author_user_id, author_session_id, body, created_at }`.

State changes through `PUT` follow the same rules as the MCP `update` tool except that a user may set any state and is not bound by leases; setting `done` on a task with a lease clears the lease. `priority` is 0 (critical) to 3 (low), default 2.

The assignment fields (`role`, `assignee_user_id`, `assignee_session_id`) are provisional pending a dedicated task-structure planning session (`docs/open-questions.md`).

### Git (`/api/projects/{pid}/git`)

| Method | Path | Auth | Body → Response |
| --- | --- | --- | --- |
| GET | `/projects/{pid}/git/session-branches` | JWT | → `SessionBranch[]` |
| POST | `/projects/{pid}/git/merge` | JWT | `{source, target, message?}` → `{commit}` or 422 `{conflicts}` |
| POST | `/projects/{pid}/git/rebase` | JWT | `{branch, onto}` → `{commit}` or 422 `{conflicts}` |
| POST | `/projects/{pid}/git/push` | JWT | `{ref, remote_branch?, force?: false}` → `{remote_branch, commit}` |

`SessionBranch = { session_id, ref: "refs/sessions/<id>", commit, ahead, behind, base: default_branch, updated_at }`. `source`, `branch`, `ref` accept either a session id or a mirror branch name; `target`/`onto` accept a mirror branch name.

### Secrets (`/api/secrets`)

| Method | Path | Auth | Body → Response |
| --- | --- | --- | --- |
| GET | `/secrets` | JWT | `?scope=&scope_id=` → `SecretMeta[]` (user scope returns only the caller's) |
| POST | `/secrets` | JWT | `{scope, scope_id?, name, value, orchestrator_only?}` → `SecretMeta` (201; 409 if exists) |
| PUT | `/secrets/{id}` | JWT | `{value}` → `SecretMeta` (replace value) |
| PATCH | `/secrets/{id}` | JWT | `{name?, orchestrator_only?}` → `SecretMeta` (rename re-encrypts under new AAD) |
| DELETE | `/secrets/{id}` | JWT | → 204 |
| GET | `/secrets/{id}/uses` | JWT | `?limit=` → `{session_id, at}[]` |

`SecretMeta = { id, scope, scope_id, name, orchestrator_only, key_version, created_by, created_at, updated_at, last_used_at }`. No response ever contains `value`.

### Health

`GET /api/health` → `{ orchestrator: true, database: bool, engine: bool }` with 200 or 503.

## WebSocket: session stream

`GET /ws/sessions/{id}?after=<seq>&token=<jwt>`

The server replays every event with `seq > after` from the database, then streams live events. The client keeps the highest `seq` seen and dedupes on it. Messages are JSON text frames except terminal data, which uses binary frames.

Server to client:

| Message | Shape |
| --- | --- |
| event | `{ "type": "event", "event": AgentEvent }` |
| session | `{ "type": "session", "session": Session }` on every state change |
| input_accepted | `{ "type": "input_accepted", "client_id": string, "seq": number }` |
| input_rejected | `{ "type": "input_rejected", "client_id": string, "reason": string }` |
| terminal | binary frame: raw bytes from the exec PTY |
| terminal_closed | `{ "type": "terminal_closed", "exit_code": number }` |
| error | `{ "type": "error", "message": string }` followed by close |

Client to server:

| Message | Shape |
| --- | --- |
| input | `{ "type": "input", "client_id": string, "input": SessionInput }` |
| stop | `{ "type": "stop" }` |
| terminal_open | `{ "type": "terminal_open", "cols": number, "rows": number }` (session must be `running`) |
| terminal_resize | `{ "type": "terminal_resize", "cols": number, "rows": number }` |
| terminal data | binary frame: bytes to the PTY |
| terminal_close | `{ "type": "terminal_close" }` |

`SessionInput` is one of:

```ts
type SessionInput =
  | { kind: "message"; text: string }
  | { kind: "answer"; reply_to: number; text: string };   // reply_to = seq of the prompt event
```

`client_id` is a client-generated string echoed back so optimistic UI can reconcile. A `message` is always accepted for a session not in `done`/`failed`; an `answer` whose `reply_to` prompt has already been consumed is rejected. The orchestrator sends WebSocket pings every 30 seconds and closes after two missed pongs.

The terminal is an `exec` with a PTY into the session container running `/bin/bash -l` as the `agent` user, multiplexed onto the same socket with binary frames. It is an escape hatch for inspection; nothing it does is recorded as events.

## SSE: task stream

`GET /api/projects/{pid}/tasks/stream?token=<jwt>` with optional `Last-Event-ID: <seq>` header (or `?after=<seq>` for the first connection).

Each SSE message has `id: <seq>`, `event: task`, and `data: <TaskEvent JSON>`. A `: keepalive` comment is sent every 15 seconds. On reconnect the browser's `EventSource` resends `Last-Event-ID` and the server replays from there. nginx must serve this location with buffering off.

## AgentEvent

The single schema the frontend sees for every backend. Stored in `events.payload` without `seq`, `ts`, `kind` (those are columns) and reassembled on read. Payload fields whose names start with `_` (such as `_offset`, the transcript byte offset used for recovery) are internal and are stripped before an event leaves the orchestrator.

```ts
interface AgentEventBase {
  seq: number;                 // per-session, monotonic, starts at 1
  ts: string;                  // RFC 3339, orchestrator observation time
  parent_tool_use_id?: string; // present on everything emitted inside a subagent
  message_id?: string;         // backend message id when the backend provides one
}

type AgentEvent = AgentEventBase & (
  | { kind: "init";               cli_session_id: string; model?: string; tools: string[]; mcp_servers: {name: string; status: string}[]; resumed: boolean }
  | { kind: "user_message";       text: string; user_id: string | null; client_id?: string; reply_to?: number }
  | { kind: "text_delta";         text: string }                                  // partial messages only
  | { kind: "text";               text: string }                                  // complete assistant text block
  | { kind: "thinking";           text: string; redacted: boolean }
  | { kind: "tool_call";          tool_use_id: string; name: string; input: unknown }
  | { kind: "tool_result";        tool_use_id: string; content: string | unknown; is_error: boolean; truncated: boolean }
  | { kind: "permission_denied";  tool_use_id?: string; name: string; reason: string }
  | { kind: "prompt";             prompt_id: string; text: string; options?: string[] }   // needs an "answer" input
  | { kind: "subagent_start";     tool_use_id: string; description: string; agent_type?: string }
  | { kind: "subagent_end";       tool_use_id: string; is_error: boolean }
  | { kind: "result";             subtype: string; is_error: boolean; num_turns: number; duration_ms: number; cost_usd?: number; usage?: unknown; permission_denials: unknown[] }
  | { kind: "error";              message: string; fatal: boolean }
  | { kind: "state_change";       from: SessionState; to: SessionState; reason: string; signal?: "SIGINT" | "SIGTERM" }
  | { kind: "launch_warning";     message: string }                              // e.g. undeclared secret
  | { kind: "git";                op: "sync" | "merge" | "rebase" | "push"; ok: boolean; detail: unknown }
  | { kind: "raw";                backend: "claude"; native: unknown }            // untranslated native line
);
```

Translation rules for the Claude backend, from `stream-json` lines:

- `system`/`init` → `init`; `session_id` is stored as `cli_session_id`; `resumed` is true when the launch used `--resume`.
- `system`/`permission_denied` → `permission_denied`.
- `assistant` messages → one event per content block: `text`, `thinking`, `tool_call`. A `tool_call` whose name is the subagent tool additionally emits `subagent_start`. The subagent tool has been called `Task` and `Agent` in different CLI versions; the translator matches both from one constant, and a fixture per pinned CLI version proves the constant is still right.
- `user` messages produced by the CLI (tool results) → `tool_result` per block; a result for a subagent tool call additionally emits `subagent_end`. User messages the orchestrator itself wrote are recorded as `user_message` at write time and the CLI's echo, if any, is dropped by matching on content hash.
- `stream_event` (only with `--include-partial-messages`) → `text_delta` for text deltas; other stream events are dropped because the complete block follows.
- `result` → `result`. For an ephemeral session the owner then stops the container and emits a `state_change` to `done`.
- Anything else → `raw`.

Every event emitted from a native message that carries `parent_tool_use_id` copies it. The frontend groups events by that id under the corresponding `tool_call`.

## TaskEvent

```ts
interface TaskEvent {
  seq: number;                 // per-project, monotonic
  ts: string;
  task_id: string | null;
  actor: { kind: "user"; user_id: string } | { kind: "session"; session_id: string } | { kind: "system" };
  kind: "created" | "updated" | "state_changed" | "claimed" | "released" | "lease_expired"
      | "commented" | "dependency_added" | "dependency_removed" | "needs_human" | "deleted";
  task?: Task;                 // full task after the change, absent on "deleted"
  comment?: Comment;           // on "commented"
  from?: TaskState; to?: TaskState;   // on "state_changed"
  reason?: string;             // on "needs_human", "lease_expired"
}
```

The board reducer applies `task` wholesale on any event that carries it, so ordering within a task is safe and a missed event is healed by the next one.

## MCP tool contracts

Served at `http://orchestrator:7001/mcp` (Streamable HTTP), bearer-authenticated per session. The session context supplies `session_id`, `project_id` and the profile. Tools are listed to a session only if allowed for its profile; task tools are always allowed. Every mutation writes a `TaskEvent` and a `task_sessions` row.

Tool descriptions are part of the contract because they steer the agent. They are reproduced verbatim.

### `ready`

Description: "List tasks you could start now. Claim one with `claim` before doing any work on it."

Input `{ role?: string, limit?: number = 20 }`. Output `{ tasks: TaskSummary[] }` where `TaskSummary = { id, number, title, priority, role, description_excerpt, depends_on_count }`. Returns tasks in state `ready` with no lease, filtered by `role` if given (a task with `role = null` matches any filter), ordered by `priority` then `number`.

### `claim`

Description: "Claim a task before you start it. You get a lease; keep working and commenting to hold it. If the claim fails someone else has it, pick another."

Input `{ task_id: string, lease_minutes?: number = 30 }` (1–240). Output `{ task: Task }` or MCP error `conflict` with message "task is not claimable" when the atomic update returns zero rows.

### `update`

Description: "Update a task you hold. Move it to `done` only after your work is committed and you have left a comment saying what you did."

Input `{ task_id: string, state?: "in_progress" | "done" | "ready", title?: string, description?: string, priority?: number, add_depends_on?: string[], remove_depends_on?: string[], assignee_role?: string | null }`. Output `{ task: Task }`. Rules: the caller must hold the lease, except for `add_depends_on`/`remove_depends_on`/`title`/`description` on tasks the caller created and nobody holds. `state: "ready"` releases the lease (giving the task back). `state: "done"` releases the lease, sets `closed_at`, and recomputes `blocked` dependants. Extends the lease by the original TTL. Dependency cycles return error `invalid_argument`.

### `comment`

Description: "Leave a comment on a task. This is how you talk to other agents and to humans: say what you found, what you changed, what you need. Comment before you finish."

Input `{ task_id: string, body: string }`. Output `{ comment: Comment }`. Any session in the project may comment on any task; commenting on a held task extends the lease.

### `needs_human`

Description: "Hand a task to a human when you are blocked on a decision, credentials, or anything you must not decide alone. Say exactly what you need."

Input `{ task_id: string, reason: string }`. Output `{ task: Task }`. Sets `state = needs_human`, `needs_human_reason`, releases the lease, keeps `assignee_session_id`. Requires holding the lease or the task being unclaimed.

### `create_task`

Description: "Create a follow-up task when you discover work that is out of scope for what you hold. Link it with `depends_on` if it must wait."

Input `{ title: string, description?: string, priority?: number = 2, role?: string, depends_on?: string[] }`. Output `{ task: Task }`. `created_by_session_id` is set.

### `list_session_branches` (git; profile-gated)

Description: "List session branches in the project mirror with how far ahead/behind they are relative to the default branch."

Input `{}`. Output `{ branches: SessionBranch[] }` (same shape as REST).

### `merge` (git; profile-gated)

Description: "Merge a source branch into a target branch in the mirror. Fails with the conflicting paths if it cannot be done cleanly; never resolves conflicts for you."

Input `{ source: string, target: string, message?: string }`. Output `{ commit: string }` or error `conflict` with `data: { conflicts: string[] }`. The calling session's own branch is synced first.

### `rebase` (git; profile-gated)

Description: "Rebase a branch onto another in the mirror. Use it to bring a session branch up to date with the default branch before merging."

Input `{ branch: string, onto: string }`. Output `{ commit: string }` or `conflict`. If `branch` is the calling session's branch, the session work tree is updated afterwards when clean.

### `push` (git; profile-gated)

Description: "Push a mirror branch to the upstream remote. Only do this when a human or the task explicitly asks for it."

Input `{ ref: string, remote_branch?: string, force?: boolean = false }`. Output `{ remote_branch: string, commit: string }`. Force pushes are refused unless `force` is true and the profile has `push` in `mcp_tools`; session refs are pushed as `refs/heads/session/<id>` by default.

Error codes used across tools: `unauthorized` (bad token), `forbidden` (tool not in profile), `not_found`, `conflict`, `invalid_argument`, `internal`.

## Frontend

Vite, React 19, TypeScript strict, Tailwind CSS 4, React Router 7, TanStack Query, Zustand for per-session and per-project reducers, `@tanstack/react-virtual` for the transcript, `react-markdown` for text, a diff renderer for edit tools, `xterm.js` for the terminal view, Playwright for end-to-end tests.

Structure:

```
frontend/src/
├── components/     reusable UI: FormField, SubmitButton, Alert, LoadingState, EmptyState, PageLayout, AuthLayout, ProtectedRoute, AdminRoute, ...
├── pages/          route-level: LoginPage, AcceptInvitePage, ChangePasswordPage, ForgotPasswordPage, ResetPasswordPage, ProjectsPage, ProjectPage, SessionPage, TasksPage, SecretsPage, ProfileEditorPage, AdminPage (users + invites)
├── session/        SessionView, Transcript, Composer, TerminalView, tool renderers, sessionStore (Zustand), useSessionSocket
├── tasks/          TaskBoard, TaskCard, TaskDetail, taskStore (Zustand), useTaskStream
├── services/       apiClient (fetch wrapper with refresh-on-401), auth, projects, profiles, sessions, tasks, secrets, users, git
├── hooks/          useAuth, useFormSubmit
├── types/          TypeScript mirrors of every API shape in this document
└── utils/
```

Rules: components never call `fetch`; every request goes through `services/`. Auth state lives in `services/auth` with an in-memory access token mirrored to `localStorage`, and a `useAuth()` hook that subscribes to it. `ProtectedRoute` redirects a user whose `must_change_password` claim is set to the change-password page.

**Session state.** `useSessionSocket(sessionId)` opens the WebSocket with `after = store.lastSeq`, fetches older history through REST when the user scrolls up, and dispatches every event to the session store. The store never keeps the event list; it folds events into:

```ts
interface SessionState {
  session: Session | null;
  status: "connecting" | "live" | "reconnecting";
  lastSeq: number;
  order: string[];                         // message ids in display order
  messages: Record<string, Message>;       // id -> folded message
  pendingTools: Record<string, string>;    // tool_use_id -> message id awaiting a result
  subagents: Record<string, string[]>;     // parent_tool_use_id -> message ids nested under it
  pendingPrompt: { seq: number; prompt_id: string } | null;
}

type Message =
  | { id; kind: "user"; text; pending?: boolean }
  | { id; kind: "assistant_text"; text; streaming: boolean }
  | { id; kind: "thinking"; text }
  | { id; kind: "tool"; name; input; result?; is_error?; running: boolean; children?: string[] }
  | { id; kind: "system"; text; level: "info" | "warn" | "error" }
  | { id; kind: "result"; ... };
```

`text_delta` appends to the current streaming assistant message; the following `text` replaces it and clears `streaming`. `tool_call` creates a tool message and registers it in `pendingTools`; `tool_result` completes it. Events carrying `parent_tool_use_id` are placed under the tool message with that id instead of at the top level. `user_message` with a `client_id` matching an optimistic message replaces it.

**Transcript rendering.** One renderer per tool family, chosen by tool name: markdown for assistant text; a side-by-side or unified diff for edit and write tools (computed from `old_string`/`new_string` or file content); monospace with ANSI stripping for shell tools; a collapsed one-line summary for read, glob and grep tools that expands on click; a nested, collapsible transcript for subagents; a JSON tree for anything else and for `raw`. Long tool results are collapsed above 40 lines.

**Task board.** `useTaskStream(projectId)` opens the SSE stream with `Last-Event-ID = store.lastSeq`, loads the initial list through REST, and applies `TaskEvent`s to the task store. Cards show state, role, lease holder (with a link to the session), assignee and dependency counts; a detail drawer shows description, comments, dependencies and the sessions that touched the task with links into their transcripts. The session view shows the tasks its session touched in a side panel.

**Composer.** A text area with submit on Enter (Shift+Enter for newline), disabled only when the session is `done`/`failed`. While a turn is in progress the button reads "Interject". A stop button sends `stop`. When a `prompt` event is pending, the composer switches to answer mode and sends an `answer` with `reply_to`.

## Non-goals for v1

- Multiple role profiles acting together (implementer, reviewer, QA, merge agents) and any policy that spawns sessions automatically. The tables allow it; nothing launches an ephemeral session from the UI.
- GitHub App credentials, GitHub login, webhooks.
- Egress restriction for session containers and sandboxed runtimes as defaults. `runtime` is configurable per profile; installing and choosing one is the operator's job.
- Per-project authorisation. Every user sees every project.
- Multi-node orchestration or more than one orchestrator instance.
- A second agent backend. The `AgentBackend` trait exists so one can be added; GitHub Copilot CLI is the candidate and lives on the roadmap in `README.md`.
- Self-registration. Users exist only through invites.
- Secret injection through files instead of environment variables (documented hardening step).
- Transcript export, search across sessions, cost dashboards.
- Mobile layouts beyond "does not break".
