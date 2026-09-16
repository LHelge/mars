# Functional specification, v1

This is the contract for the first release: what a user can do, every endpoint and stream, the event schemas the frontend consumes, the MCP tools agents call, and what v1 deliberately does not do. Each section is written so that an implementation task can cite it. Design rationale lives in `ARCHITECTURE.md` and `docs/decisions/`; the schema in `docs/data-model.md`.

## User-facing features

**Login and invites.** There is no self-registration. The database is seeded with one administrator (`admin`, default password in `README.md`) who must change the password at first login before doing anything else. Admins invite people by email: the invitee receives a link (sent through Resend, or written to the orchestrator log when no API key is configured), opens it, chooses a username and password, and is logged in. Invites expire after 7 days and can be revoked. Login returns a 15-minute JWT access token in the response body and a 30-day refresh token in an HTTP-only cookie. Password reset works the same way as invites: a link by email, a single-use token.

**Projects.** A user creates a project from a remote URL and, for a private repository, a personal access token. Creation returns immediately with `status: cloning`; the project list shows progress and turns `ready` or `error`. A ready project shows its default branch, last fetch time, its profiles, sessions, shared directories, task states and task board. Shared directories are named directories that every session of the project gets mounted read-write at a container path of the user's choice, typically a build directory such as Cargo's `target` (ADR 0015); the project page lists them, adds and removes them, and can empty one while no session is running. Deleting a project deletes its sessions, tasks, secrets, shared directories, CLI state directory and mirror.

**Agent profiles.** Every project starts with a `default` conversational profile using the built-in Claude image and serving the `ready` task state. Users can edit a profile's name, model, system prompt, image, runtime, MCP tool allow-list, declared secrets, served task states, partial-message flag (default on for conversational, off for ephemeral) and idle timeout, and create further conversational profiles. A profile's served states and its system prompt together define its role: a planner serves `backlog` and hands tasks to `ready`, a reviewer serves `review` and hands them to `merge` or back to `ready`. Ephemeral profiles can be created and edited but cannot be launched from the UI in v1.

**Sessions.** A user launches a session from a profile and a base ref (default: the project's default branch), optionally with a first message, and optionally for one task: the session then starts holding that task, and the task card links to the session. From a task's detail view the same launch is one click. The session view shows the full transcript with per-tool rendering and nested subagents, a composer for sending messages (including mid-turn), a stop button, and session metadata (state, branch, container, CLI session id). Sessions keep running when nobody is watching; anyone opening a session later sees everything that happened. A parked session looks like a running one that is waiting; sending a message relaunches it. Users can end a session, sync its branch into the mirror, and open a terminal into a running session's container.

**Task board.** Per project, a board whose columns are the project's own task states (default `backlog`, `ready`, `review`, `merge`, `needs_human`, `done`, `cancelled`), with dependencies, labels, parent tasks, comments, assignee, the session holding each task, attempt count, and links to the sessions that touched it. Users create, edit, comment, add and remove dependencies, release held tasks, move tasks between states, open a task in a new session, and edit the state list itself. Changes made by agents appear live.

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
| PUT | `/projects/{id}` | JWT | `{name?, default_branch?, max_attempts?}` → `Project` |
| DELETE | `/projects/{id}` | JWT | → 204 (refused with 409 while any session is `running` or `creating`) |
| POST | `/projects/{id}/retry-clone` | JWT | → `Project` (only from `error`) |
| POST | `/projects/{id}/fetch` | JWT | → `Project` (runs a mirror fetch now) |
| GET | `/projects/{id}/branches` | JWT | → `Branch[]` (mirror heads and session refs) |

`Project = { id, name, remote_url, default_branch, status, status_message, last_fetched_at, max_attempts, created_at, has_credential }`. `max_attempts` (1–20, default 3) is how many times a task may be claimed in one state before a release escalates it; see "Tasks". `credential` on create is stored as the project-scoped orchestrator-only secret `GIT_CREDENTIAL` and is never returned. `Branch = { name, kind: "head" | "session", commit, session_id? }`.

### Shared directories (`/api/projects/{pid}/shared-dirs`)

Directories under `/data/projects/{pid}/shared/<name>` that are bind-mounted read-write at `container_path` in every session container of the project (`ARCHITECTURE.md`, "Storage"; ADR 0015). The list is read at each launch; a running session keeps the mounts it started with.

| Method | Path | Auth | Body → Response |
| --- | --- | --- | --- |
| GET | `/projects/{pid}/shared-dirs` | JWT | → `SharedDir[]` |
| POST | `/projects/{pid}/shared-dirs` | JWT | `{name, container_path}` → `SharedDir` (201; 400 for an invalid name or path; 409 when the name or the path is already used in the project) |
| POST | `/projects/{pid}/shared-dirs/{name}/clear` | JWT | → 204 (empties the directory; 409 while any session of the project is `running` or `creating`) |
| DELETE | `/projects/{pid}/shared-dirs/{name}` | JWT | → 204 (removes the directory and its contents; 409 while any session of the project is `running` or `creating`) |

`SharedDir = { name, container_path, created_at }`. `name` is 1–64 characters matching `[a-z0-9][a-z0-9_-]*` and is the directory name on disk. `container_path` is an absolute, normalised path (no `.`, `..`, repeated or trailing slashes) that is not `/data` or below it and is neither equal to nor an ancestor of `/session/work`, `/session/home`, `/session/log` or `/session/mcp.json`; it may lie inside `/session/work`. The recommended entries per ecosystem are in `README.md`, "Operating notes".

### Agent profiles (`/api/projects/{pid}/profiles`)

| Method | Path | Auth | Body → Response |
| --- | --- | --- | --- |
| GET | `/projects/{pid}/profiles` | JWT | → `Profile[]` |
| POST | `/projects/{pid}/profiles` | JWT | `ProfileInput` → `Profile` |
| GET | `/projects/{pid}/profiles/{id}` | JWT | → `Profile` |
| PUT | `/projects/{pid}/profiles/{id}` | JWT | `ProfileInput` → `Profile` |
| DELETE | `/projects/{pid}/profiles/{id}` | JWT | → 204 (409 if default or has sessions) |

`Profile = { id, project_id, name, kind, backend, model, system_prompt, permission_mode, image, runtime, mcp_tools, secrets, serves_states, partial_messages, idle_timeout_secs, is_default, created_at, updated_at }`. `ProfileInput` is the same without ids and timestamps; `permission_mode` must be `bypass`; `mcp_tools` entries must be known tool names; `serves_states` entries must be names of the project's `queue` states (400 otherwise) and default to `["ready"]`.

### Sessions (`/api/projects/{pid}/sessions`, `/api/sessions/{id}`)

| Method | Path | Auth | Body → Response |
| --- | --- | --- | --- |
| GET | `/projects/{pid}/sessions` | JWT | `?state=` → `Session[]` |
| POST | `/projects/{pid}/sessions` | JWT | `{profile_id, base_ref?, title?, message?, task_id?}` → `Session` (201, `state: creating`; 400 if `base_ref` does not resolve in the mirror; 409 for an `ephemeral` profile in v1; 409 if `task_id` names a task that is held, blocked or in a terminal state) |
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

`Session = { id, project_id, profile_id, created_by, title, task_id, state, base_ref, branch, container_id, cli_session_id, last_seq, last_activity_at, error, created_at, parked_at, ended_at }`.

With `task_id`, the session claims the task in the same transaction that creates the session row, regardless of the profile's served states (the user chose), and the first input delivered to the CLI is a generated message naming the task (`You hold task #12: <title>. Call get_task to read it before starting.`), followed by `message` if given. The session holds the task until it hands it off, releases it, or ends.

### Task states (`/api/projects/{pid}/task-states`)

The columns of a project's board, in order. Every project starts with the default set listed in `docs/data-model.md`, `task_states`. A state's `kind` says what it means to the orchestrator: `queue` states are where agents pick work up, the single `human` state is where escalations land, `terminal` states close a task and satisfy dependencies.

| Method | Path | Auth | Body → Response |
| --- | --- | --- | --- |
| GET | `/projects/{pid}/task-states` | JWT | → `TaskState[]` ordered by `position` |
| POST | `/projects/{pid}/task-states` | JWT | `{name, kind, position?}` → `TaskState` (201; 400 for an invalid name or kind; 409 if the name is taken or the project already has a `human` state) |
| PUT | `/projects/{pid}/task-states/{name}` | JWT | `{name?, position?}` → `TaskState` (400 if `kind` is given: kind is immutable; 409 if the new name is taken) |
| DELETE | `/projects/{pid}/task-states/{name}` | JWT | → 204 (409 while any task is in the state, for the `human` state, and for the last `terminal` state) |

`TaskState = { id, project_id, name, kind: "queue" | "human" | "terminal", position, created_at }`. `name` is 1–32 characters matching `[a-z0-9][a-z0-9_-]*`. A missing `position` appends; an explicit one shifts the states at and after it. Renaming a state renames it everywhere at once, since tasks and profiles reference states by id. Every change emits a `states_changed` `TaskEvent`.

### Tasks (`/api/projects/{pid}/tasks`)

| Method | Path | Auth | Body → Response |
| --- | --- | --- | --- |
| GET | `/projects/{pid}/tasks` | JWT | `?state=&label=&priority=&parent=&held=` → `Task[]` ordered by priority, then number |
| POST | `/projects/{pid}/tasks` | JWT | `{title, description?, state?, priority?, labels?, parent_id?, depends_on?: id[]}` → `Task` (201; 400 for an unknown state) |
| GET | `/projects/{pid}/tasks/{id}` | JWT | → `TaskDetail` |
| PUT | `/projects/{pid}/tasks/{id}` | JWT | `{title?, description?, state?, priority?, labels?, parent_id?, assignee_user_id?}` → `Task` |
| DELETE | `/projects/{pid}/tasks/{id}` | JWT | → 204 |
| POST | `/projects/{pid}/tasks/{id}/dependencies` | JWT | `{depends_on: id, kind?: "blocks"}` → `Task` (409 on cycle) |
| DELETE | `/projects/{pid}/tasks/{id}/dependencies/{dep}` | JWT | → `Task` |
| POST | `/projects/{pid}/tasks/{id}/comments` | JWT | `{body}` → `Comment` |
| POST | `/projects/{pid}/tasks/{id}/release` | JWT | → `Task` (clears the lease, keeps the state; 409 if nobody holds it) |
| GET | `/projects/{pid}/tasks/stream` | JWT (`?token=`) | SSE of `TaskEvent`; see "SSE" |

`{id}` and `{dep}` accept a task's UUID or its per-project number.

`Task = { id, project_id, number, title, description, state, priority, blocked, labels, parent_id, assignee_user_id, lease_holder_session_id, lease_since, attempts, needs_human_reason, depends_on: {task_id, kind}[], blocks: id[], created_at, updated_at, closed_at }`. `state` is the state's name. `depends_on` lists every outgoing dependency with its kind; `blocks` lists the tasks that have a `blocks` dependency on this one. `TaskDetail = Task & { comments: Comment[], children: Task[], sessions: {session_id, first_touched_at, last_touched_at}[] }`. `Comment = { id, task_id, author_user_id, author_session_id, system, body, created_at }`.

A task's state is the queue it waits in, and the lease says who is working on it (`ARCHITECTURE.md`, "Task tracker"). There is no in-progress state: a held task is in progress in whatever state it is in. A user may set any state through `PUT` and is not bound by leases. Setting a state on a held task hands it off, which clears the lease and resets `attempts`; a terminal state sets `closed_at` and unblocks dependants; a non-terminal state on a closed task reopens it. A user `release` keeps the state and never escalates; agent and reaper releases escalate to the `human` state once `attempts` reaches the project's `max_attempts`. `priority` is 0 (critical) to 3 (low), default 2. `parent_id` nests one level: a task that has children cannot be given a parent (400). A `blocks` dependency on a non-terminal task marks the dependant `blocked`; the other kinds are informational.

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
  task_id: string | null;      // null on "states_changed"
  actor: { kind: "user"; user_id: string } | { kind: "session"; session_id: string } | { kind: "system" };
  kind: "created" | "updated" | "state_changed" | "claimed" | "released" | "escalated"
      | "blocked" | "unblocked" | "commented" | "dependency_added" | "dependency_removed"
      | "deleted" | "states_changed";
  task?: Task;                 // full task after the change; absent on "deleted" and "states_changed"
  comment?: Comment;           // on "commented"
  from?: string; to?: string;  // state names, on "state_changed" and "escalated"
  reason?: string;             // on "released": "given_back" | "session_ended" | "stalled" | "user"; on "escalated": free text
  states?: TaskState[];        // on "states_changed": the project's full state list after the change
}
```

`state_changed` is any hand-off, by an agent or a user. `escalated` is a move into the project's `human` state, whether an agent asked for it (`needs_human`) or the reaper ran out of attempts; it carries `from`/`to` like a state change plus the reason. `released` is a lease cleared without a state change. `blocked`/`unblocked` fire when a dependency change or a hand-off flips a dependant's `blocked` flag.

The board reducer applies `task` wholesale on any event that carries it, so ordering within a task is safe and a missed event is healed by the next one. On `states_changed` it replaces its column list.

## MCP tool contracts

Served at `http://orchestrator:7001/mcp` (Streamable HTTP), bearer-authenticated per session. The session context supplies `session_id`, `project_id` and the profile. Tools are listed to a session only if allowed for its profile; task tools are always allowed. Every mutation writes a `TaskEvent` and a `task_sessions` row.

Tool descriptions are part of the contract because they steer the agent. They are reproduced verbatim.

Every `task` argument accepts a task's UUID or its per-project number (as a number or a string such as `"12"` or `"#12"`). A task's `state` in inputs and outputs is the state's name. The tracker's rules (state as queue, lease as worker, attempts) are in `ARCHITECTURE.md`, "Task tracker".

### `ready`

Description: "List tasks you could start now, in the states your profile serves. Claim one with `claim` before doing any work on it. If you were launched for a task you already hold it: call `get_task` on it instead."

Input `{ limit?: number = 20 }`. Output `{ tasks: TaskSummary[] }` where `TaskSummary = { id, number, title, state, priority, labels, description_excerpt, attempts, depends_on_count }`. Returns tasks in the calling profile's served states that are not blocked and have no lease holder, ordered by `priority` then `number`. A profile that serves no states gets an empty list.

### `claim`

Description: "Claim a task before you start it. You hold it until you hand it off with `update`, give it back with `release`, or your session ends. If the claim fails someone else has it: pick another."

Input `{ task: string | number }`. Output `{ task: Task }` or MCP error `conflict` with message "task is not claimable" when the atomic update returns zero rows. Only tasks in the profile's served states can be claimed; a task outside them returns `conflict` with message "task is not in a state this profile serves".

### `get_task`

Description: "Read a task in full: description, comments, dependencies, sub-tasks and which sessions worked on it. Read it before you start; the comments are where earlier agents and humans left context for you."

Input `{ task: string | number }`. Output `{ task: TaskDetail }` (same shape as REST).

### `update`

Description: "Update a task you hold. Setting `state` hands the task to whoever serves that state and ends your hold, so do it last: commit your work, leave a comment saying what you did and what the next agent should look at, then set the state."

Input `{ task: string | number, state?: string, title?: string, description?: string, priority?: number, labels?: string[], parent?: string | number | null, add_depends_on?: string[], remove_depends_on?: string[] }`. Output `{ task: Task }`. Rules: the caller must hold the lease, except for `title`, `description`, `labels`, `add_depends_on` and `remove_depends_on` on tasks the caller created that nobody holds. `state` must be one of the project's states; an unknown name returns `invalid_argument` with the valid names in the message. A state change releases the lease and resets `attempts`; a terminal state also sets `closed_at` and recomputes `blocked` on dependants. `add_depends_on` creates `blocks` dependencies; a cycle returns `invalid_argument`.

### `release`

Description: "Give a task back without finishing it, and say why. Use this when you cannot make progress. The task stays in its state for another agent; after too many attempts it goes to a human instead."

Input `{ task: string | number, reason: string }`. Output `{ task: Task }`. The caller must hold the lease. The reason is recorded as a comment. If `attempts` has reached the project's `max_attempts`, the task moves to the project's `human` state with `needs_human_reason` set, and the output `task` shows that state.

### `comment`

Description: "Leave a comment on a task. This is how you talk to other agents and to humans: say what you found, what you changed, what you need. Comment before you hand off."

Input `{ task: string | number, body: string }`. Output `{ comment: Comment }`. Any session in the project may comment on any task.

### `needs_human`

Description: "Hand a task to a human when you are blocked on a decision, credentials, or anything you must not decide alone. Say exactly what you need."

Input `{ task: string | number, reason: string }`. Output `{ task: Task }`. Moves the task to the project's `human` state, sets `needs_human_reason`, releases the lease and resets `attempts`. Requires holding the lease or the task being unheld.

### `create_task`

Description: "Create a task when you discover work outside what you hold: a follow-up, a bug, or a sub-task of a plan. Put it in the state that matches how ready it is (`backlog` if it still needs planning). Link it with `depends_on` if it must wait, and with `parent` if it is part of a larger task."

Input `{ title: string, description?: string, state?: string, priority?: number = 2, labels?: string[], parent?: string | number, depends_on?: string[] }`. Output `{ task: Task }`. `state` defaults to the project's default state (its first `queue` state); an unknown name returns `invalid_argument`. `depends_on` creates `blocks` dependencies. `created_by_session_id` is set. If the caller holds a task and that task is not the new task's parent, a `discovered_from` dependency from the new task to the held one is recorded so the provenance is visible.

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
├── tasks/          TaskBoard, TaskCard, TaskDetail, TaskStatesEditor, taskStore (Zustand), useTaskStream
├── services/       apiClient (fetch wrapper with refresh-on-401), auth, projects, profiles, sessions, tasks, taskStates, secrets, users, git
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

**Task board.** `useTaskStream(projectId)` opens the SSE stream with `Last-Event-ID = store.lastSeq`, loads the project's task states and the initial task list through REST, and applies `TaskEvent`s to the task store. Columns are the project's states in `position` order; `states_changed` replaces them. Cards show priority, labels, the holding session (with a link), attempts when above one, assignee, blocked and dependency indicators, and a parent badge. A detail drawer shows description, comments (system comments styled apart), dependencies by kind, children, the sessions that touched the task with links into their transcripts, and actions: move to a state, release, and "open in session", which picks a conversational profile (default: the first that serves the task's state) and launches it with `task_id`. The session view shows the task the session was launched for and the tasks it touched in a side panel. `TaskStatesEditor` on the project page lists, adds, renames, reorders and removes states, with the deletion rules from the API surfaced as disabled actions.

**Composer.** A text area with submit on Enter (Shift+Enter for newline), disabled only when the session is `done`/`failed`. While a turn is in progress the button reads "Interject". A stop button sends `stop`. When a `prompt` event is pending, the composer switches to answer mode and sends an `answer` with `reply_to`.

## Non-goals for v1

- Automatic launching of agents: a dispatcher that starts an ephemeral session when a served state has claimable work, and scheduled agents that run a profile on a cron expression (a daily tech-debt scan that files tasks, an agent that turns GitHub issues into backlog tasks). The task model is built for both and needs no change for them (`ARCHITECTURE.md`, "Task tracker", "After v1"). In v1 work reaches an agent in exactly one way: a user launches a conversational session, optionally for a task, and that agent calls `ready` and `claim`.
- GitHub App credentials, GitHub login, webhooks.
- Egress restriction for session containers and sandboxed runtimes as defaults. `runtime` is configurable per profile; installing and choosing one is the operator's job.
- Per-project authorisation. Every user sees every project.
- Multi-node orchestration or more than one orchestrator instance.
- A second agent backend. The `AgentBackend` trait exists so one can be added; GitHub Copilot CLI is the candidate and lives on the roadmap in `README.md`.
- Self-registration. Users exist only through invites.
- Secret injection through files instead of environment variables (documented hardening step).
- Transcript export, search across sessions, cost dashboards.
- Mobile layouts beyond "does not break".
