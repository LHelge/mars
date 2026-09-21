# Functional specification, v1

This is the contract for the first release: what a user can do, every endpoint and stream, the event schemas the frontend consumes, the MCP tools agents call, and what v1 deliberately does not do. Each section is written so that an implementation task can cite it. Design rationale lives in `ARCHITECTURE.md` and `docs/decisions/`; the schema in `docs/data-model.md`.

## User-facing features

**Login and invites.** There is no self-registration. The database is seeded with one administrator (`admin`, default password in `README.md`) who must change the password at first login before doing anything else. Admins invite people by email: the invitee receives a link (sent through Resend, or written to the orchestrator log when no API key is configured), opens it, chooses a username and password, and is logged in. Invites expire after 7 days and can be revoked. Login returns a 15-minute JWT access token in the response body and a 30-day refresh token in an HTTP-only cookie. Password reset works the same way as invites: a link by email, a single-use token, valid for 1 hour. Passwords are 10–128 characters. Login is throttled: after 10 failed attempts for one username or one client address within 15 minutes, the endpoint answers 429 for the next 15 minutes; password-reset requests are limited to 3 per identifier per hour and still answer 204. Each user can turn off the escalation emails described under "Task board" on their own settings page.

**Projects.** A user creates a project from a remote URL and, for a private repository, a personal access token. Creation returns immediately with `status: cloning`; the project list shows progress and turns `ready` or `error`. A ready project shows its default branch, last fetch time, its profiles, sessions, shared directories, task states and task board. Shared directories are named directories that every session of the project gets mounted read-write at a container path of the user's choice, typically a build directory such as Cargo's `target` (ADR 0015); the project page lists them, adds and removes them, and can empty one while no session is running. Deleting a project deletes its sessions, tasks, secrets, shared directories, CLI state directory and mirror.

**Agent profiles.** Every project starts with a `default` conversational profile using the built-in Claude image and serving the `ready` task state. Users can edit a profile's name, model, system prompt, image, runtime, MCP tool allow-list, declared secrets (extra ones the agent's job needs; the agent's own credential is never declared), served task states, partial-message flag (default on for conversational, off for ephemeral) and idle timeout, and create further conversational profiles. A profile's served states and its system prompt together define its role: a planner serves `backlog` and hands tasks to `ready`, a reviewer serves `review` and hands them to `merge` or back to `ready`. An ephemeral profile runs one prompt and ends: a user launches it from a task's detail view ("run once") or from the project page with a message, and its session shows the transcript, result and cost but has no composer.

**Sessions.** A user launches a session from a profile and a base ref (default: the task's current hand-off commit when present, otherwise the project's default branch), optionally with a first message, and optionally for one task: the session then starts holding that task, and the task card links to the session. From a task's detail view the same launch is one click. The session view shows the full transcript with per-tool rendering and nested subagents, a composer for sending messages (including mid-turn), a stop button, and session metadata (state, branch, container, CLI session id). Sessions keep running when nobody is watching; anyone opening a session later sees everything that happened. A parked session looks like a running one that is waiting; sending a message relaunches it. A session's title defaults to the task's title when launched for a task, otherwise to the first line of the first message (at most 80 characters). The session view also shows cost and token usage so far and a "Changes" panel with the diff of the session branch against its base. Users can end a session, sync its branch into the mirror, and open a terminal into a running session's container.

**Task board.** Per project, a board whose columns are the project's own task states (default `backlog`, `ready`, `review`, `merge`, `needs_human`, `done`, `cancelled`), with dependencies, labels, parent tasks, comments, assignee, the session holding each task, attempt count, and links to the sessions that touched it. Users create, edit, comment, add and remove dependencies, release held tasks, move tasks between states, open a task in a new session, and edit the state list itself. A search field filters cards across columns by title or exact task number (`42` or `#42`). Changes made by agents appear live. A task with open children is blocked until they close and closes itself when the last one does. Every escalation into the human state emails the task's assignee, or every admin when there is none; each user can opt out.

**Git operations.** From a session or from the project page, a user can list session branches in the mirror, merge a session branch into a target branch, rebase a session branch onto a target, and push a branch upstream. Conflicts are reported with the conflicting paths. The diff of any branch against its base can be viewed, and after a push to a GitHub remote the UI links to the compare page for opening a pull request.

**Secrets.** A write-only manager at global, project and user scope: create, replace, rename, delete, and list names with metadata (scope, created, updated, orchestrator-only, key version, last use). The value is never shown after entry.

**Agent credentials.** What agents authenticate with — for Claude Code a subscription token or an Anthropic API key — is set up on the Secrets page through a guided form: pick the kind, paste the value, choose whether it applies to you, to one project or to everyone. No name is typed and no profile is edited; every session picks up the most specific credential that applies to the user who launches it. The profile editor and the launch form say which credential a launch would use, or that there is none, before anything is launched (ADR 0036).

**Users (admin).** List and delete users, toggle admin, send and revoke invites.

**Dashboard.** The home page lists running and parked sessions across all projects and every task waiting in a human state.

## Authentication

Identical in shape to a conventional JWT plus refresh cookie scheme:

- `POST /api/auth/login` and `POST /api/auth/accept-invite` return `{ user, access_token }` and set the `refresh_token` cookie (`HttpOnly`, `SameSite=Lax`, `Path=/`, `Secure` when `PUBLIC_URL` is https).
- The access token is a JWT signed with `JWT_SECRET`, claims `sub` (user id), `auth_version`, `username`, `admin`, `must_change_password`, `iat`, `exp`. It is sent as `Authorization: Bearer`. Every authenticated HTTP request validates the signature and expiry, loads the current user, and requires the claim's `auth_version` to match `users.auth_version`; a missing user or version mismatch returns 401. Authorization uses the current database values of `admin` and `must_change_password`, not the token snapshots. Demotion takes effect on the next request (403 for an admin-only action); deletion takes effect on the next request (401). Requests already authorized before the change may finish.
- While the current user's `must_change_password` is true, every authenticated endpoint other than `POST /auth/login`, `POST /auth/logout`, `POST /auth/refresh`, `GET /users/me` and `POST /users/{id}/password` answers 403 with error `password change required`. The frontend routes such a user to the change-password page. Changing the password clears the flag; a self-service password change issues a fresh token pair under the revocation rules below.
- Three error strings are part of the contract and are sent byte for byte, because clients may match on them (the frontend routes on `password change required`): `authentication required` (401, every failed authentication — a missing or malformed `Authorization` header, a token that does not verify, an expired token, a deleted user, a superseded `auth_version`, and a missing, unknown, revoked or expired refresh cookie alike, so a caller cannot learn which), `admin required` (403, both an administrator-only route and a self-service-or-administrator route aimed at somebody else) and `password change required` (403, the gate above). One spelling each: the orchestrator defines them once, in its prelude.
- `POST /api/auth/refresh` rotates the refresh token (old one revoked) and returns a new pair using the current user values. A missing user or expired/revoked refresh token returns 401 and clears the cookie. Whether the presented token is still usable — unrevoked and unexpired — is decided by the lookup query that re-reads it inside the transaction holding the user-row lock, never by a check on a row read before that lock.
- Every successful password change or reset updates the password hash, increments `users.auth_version`, revokes all existing refresh tokens and invalidates all outstanding password-reset links in one transaction. This applies to self-service changes, an admin changing another user's password, and reset links. A self-service change requires `current_password` and creates a replacement refresh token in that transaction, then returns its cookie and a matching access token after commit; this browser stays signed in. Changing another user's password returns 204 without changing the acting admin's credentials. Reset by link returns 204 without logging the user in; they then log in with the new password. Role changes alone preserve ordinary logins. Login, refresh, reset-link issuance and password mutations serialize on the user row and revalidate credentials after locking, so a concurrent refresh or login cannot escape revocation with an old credential (ADR 0025).
- The throttled "client address" is the leftmost `X-Forwarded-For` entry when that header is present — nginx sets it in the documented deployment — and otherwise the socket peer address, falling back to the loopback address when there is neither. A malformed header value falls back the same way. `POST /auth/login` answers 429 with error `too many login attempts` while either the submitted username or the client address is blocked, without verifying the password and without extending the block; the username key is trimmed and case-sensitive, the password-reset identifier is trimmed and lower-cased, and a throttled reset request is dropped silently and still answers 204. Both counters live in orchestrator process memory: v1 runs a single instance ("Non-goals for v1"), so there is no shared store and a restart clears them.
- WebSocket and SSE endpoints cannot receive headers from the browser, so they accept the access token as `?token=` and validate it exactly like the header. nginx must not log query strings for those locations.
- Token signature and expiry are checked when a stream opens; an open stream is not closed merely because that token later expires. Account existence, `auth_version` and the password-change gate are checked again before every incoming WebSocket application message (including terminal bytes), and at the existing WebSocket ping / SSE keepalive ticks (30 / 15 seconds). A failed authorization check closes the stream; WebSocket sends the existing `error` message with `authentication required` and closes with code 1008. Dispose of that socket's terminal attachment and stop forwarding its input/output. Already-running agent sessions are unaffected. Database failures must not authorize input or continued streaming; close and let normal retry handle them. On reconnect the client authenticates again: the frontend hooks disable the browser's automatic retry (`EventSource` would resend the stale URL forever), refresh the access token and reopen with a fresh token and the last cursor. A 401 from refresh clears local authentication, closes other streams and routes to login instead of retrying indefinitely; transient network/server failures retain normal backoff. A successful self-service password change installs its new pair before reopening streams. Revocation detection on idle streams can lag by one heartbeat interval; there is no separate revocation broadcast service.
- The MCP listener uses per-session bearer tokens, never user JWTs (`ARCHITECTURE.md`, "MCP design"). Each actual session process launch, resume or conversational retry generates a fresh token, replaces its stored hash and writes matching MCP configuration before starting the process. Subsequent requests using the replaced token fail authentication. Reconnecting the orchestrator to an already-running process preserves its token; browser reconnection and user-login changes do not rotate it (ADR 0029).

## REST API

All routes are under `/api`. Responses are bare JSON: arrays for lists, objects for single resources. By default, creates return `201` with the created resource, deletes return `204`, and everything else returns `200`; explicit statuses in the endpoint tables take precedence, including `202` for asynchronous input and stop requests. Errors are `{ "status": <u16>, "error": "<message>" }` with the same status code on the response; git-conflict responses also include `conflicts`.

| Status | Used for |
| --- | --- |
| 400 | Validation failures and malformed input. |
| 401 | Missing or invalid token. |
| 403 | Authenticated but not permitted (non-admin on admin routes, tool not allowed for profile). |
| 404 | Unknown resource. |
| 409 | State conflicts: claim lost, session not in a state that accepts the action, duplicate name, dependency cycle. |
| 422 | Git operation failed with conflicts; body includes `conflicts: string[]`. |
| 429 | Login throttled. Password-reset requests still return 204 when throttled. |
| 500 | Unexpected. Never carries internal detail. |

Timestamps are RFC 3339 strings. Ids are UUID strings.

### Auth (`/api/auth`)

| Method | Path | Auth | Body → Response |
| --- | --- | --- | --- |
| POST | `/auth/login` | — | `{username, password}` → `{user, access_token}` (429 when throttled) |
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
| PATCH | `/users/me` | JWT | `{notify_email?}` → `User` |
| GET | `/users` | admin | → `User[]` ordered by `username` |
| GET | `/users/{id}` | JWT | → `User` |
| PUT | `/users/{id}` | admin | `{username, admin}` → `User` (409 if demoting the last admin) |
| DELETE | `/users/{id}` | admin | → 204 (409 for the last admin or yourself) |
| POST | `/users/{id}/password` | JWT (self) or admin | `{current_password?, password}` → `{user, access_token}` (self) or 204 (admin) |
| GET | `/users/invites` | admin | → `Invite[]` (open invites) |
| POST | `/users/invites` | admin | `{email, admin?}` → `Invite` (201; 409 if the email has a user or an open invite) |
| DELETE | `/users/invites/{id}` | admin | → 204 (revoke) |
| POST | `/users/invites/{id}/resend` | admin | → `Invite` (new token and expiry, email sent again) |

`User = { id, username, email, admin, must_change_password, notify_email, created_at }`. `Invite = { id, email, admin, invited_by, expires_at, created_at }`; the token is delivered in the email, or in the full logged link when email is unconfigured, and is never included in the `Invite` response. Changing your own password returns a fresh token pair for the new `auth_version`; all previous token pairs are invalidated (see "Authentication").

When `RESEND_API_KEY` is unset, `LogEmailClient` intentionally writes the full invitation or password-reset message, including the usable token-bearing link, to the operator's log at `info`. This is the local-development delivery mechanism and an explicit exception to the secret-logging rule, with no extra opt-in flag. `ResendClient` does not log these links, and email-provider failures do not fall back to logging them. The exception applies only to invitation/reset delivery, not to passwords, access/refresh tokens, session MCP credentials, API keys, or ordinary request/error logging (ADR 0026).

At least one administrator must remain. Both deleting an administrator and changing `admin` from true to false are rejected with 409 if they would remove the last administrator. Self-demotion is allowed when another administrator remains; self-deletion remains prohibited. The check and mutation are one transaction, serialized with other user deletions and administrator-role changes, so concurrent requests cannot each remove one of the final two administrators. A rejected request changes no fields.

### Projects (`/api/projects`)

| Method | Path | Auth | Body → Response |
| --- | --- | --- | --- |
| GET | `/projects` | JWT | → `Project[]` ordered by `created_at` |
| POST | `/projects` | JWT | `{name, remote_url, default_branch?, credential?}` → `Project` (201, `status: cloning`) |
| GET | `/projects/{id}` | JWT | → `Project` |
| PUT | `/projects/{id}` | JWT | `{name?, default_branch?, max_attempts?}` → `Project` |
| DELETE | `/projects/{id}` | JWT | → 204 (refused with 409 while any session is `running` or `creating`) |
| POST | `/projects/{id}/retry-clone` | JWT | → `Project` (only from `error`) |
| POST | `/projects/{id}/fetch` | JWT | → `Project` (runs a mirror fetch now) |
| GET | `/projects/{id}/branches` | JWT | → `Branch[]` (integration heads, upstream-tracking refs and session refs) |

`Project = { id, name, remote_url, default_branch, status, status_message, last_fetched_at, max_attempts, created_at, has_credential }`. `default_branch` is null while discovery is pending if the caller omitted it; it must resolve to an integration head before the project becomes `ready`. Changing it with `PUT` on a `ready` project checks the name against the project repository and moves that repository's `HEAD` to it in the same request; a name that is not an integration head of the project is 400. On a `cloning` or `error` project the value is stored as given and the clone job validates it. `max_attempts` (1–20, default 3) is how many times a task may be claimed in one state before a release escalates it; see "Tasks". `credential` on create is stored as the project-scoped orchestrator-only secret `GIT_CREDENTIAL` and is never returned. `Branch = { name, kind: "head" | "upstream" | "session", commit, session_id? }`. Integration heads use names such as `main`; upstream-tracking refs use `origin/main`; session refs use their full name `refs/sessions/<id>`. Fetching refreshes upstream-tracking refs without moving integration heads or session refs. Without a task hand-off, the default session base is the integration head named by `default_branch`; callers may explicitly choose an upstream-tracking ref, tag, session ref or commit id instead.

### Shared directories (`/api/projects/{pid}/shared-dirs`)

Directories under `/data/projects/{pid}/shared/<name>` that are bind-mounted read-write at `container_path` in every session container of the project (`ARCHITECTURE.md`, "Storage"; ADR 0015). The list is read at each launch; a running session keeps the mounts it started with.

| Method | Path | Auth | Body → Response |
| --- | --- | --- | --- |
| GET | `/projects/{pid}/shared-dirs` | JWT | → `SharedDir[]` |
| POST | `/projects/{pid}/shared-dirs` | JWT | `{name, container_path}` → `SharedDir` (201; 400 for an invalid name or path; 409 when the name or the path is already used in the project) |
| POST | `/projects/{pid}/shared-dirs/{name}/clear` | JWT | → 204 (empties the directory; 409 while any session of the project is `running` or `creating`) |
| DELETE | `/projects/{pid}/shared-dirs/{name}` | JWT | → 204 (removes the directory and its contents; 409 while any session of the project is `running` or `creating`) |

`SharedDir = { name, container_path, created_at }`. `name` is 1–64 characters matching `[a-z0-9][a-z0-9_-]*` and is the directory name on disk. `container_path` is an absolute, normalised path of at most 4096 bytes (no `.`, `..`, repeated or trailing slashes, no whitespace or control characters) that is not `/data` or below it and is neither equal to nor an ancestor of `/session/work`, `/session/home`, `/session/log` or `/session/mcp.json`, and is not below `/session/mcp.json`, which is a file; it may lie inside `/session/work`. Both fields are trimmed of surrounding whitespace before they are validated and stored. The recommended entries per ecosystem are in `README.md`, "Operating notes".

### Agent profiles (`/api/projects/{pid}/profiles`)

| Method | Path | Auth | Body → Response |
| --- | --- | --- | --- |
| GET | `/projects/{pid}/profiles` | JWT | → `Profile[]` (oldest first) |
| POST | `/projects/{pid}/profiles` | JWT | `ProfileInput` → `Profile` |
| GET | `/projects/{pid}/profiles/{id}` | JWT | → `Profile` |
| PUT | `/projects/{pid}/profiles/{id}` | JWT | `ProfileInput` → `Profile` |
| DELETE | `/projects/{pid}/profiles/{id}` | JWT | → 204 (409 if default or has sessions) |

`Profile = { id, project_id, name, kind, backend, model, system_prompt, permission_mode, image, runtime, mcp_tools, secrets, serves_states, partial_messages, idle_timeout_secs, is_default, created_at, updated_at }`. `ProfileInput` is the same without ids and timestamps; `permission_mode` must be `bypass`; `mcp_tools` entries must be known tool names; `secrets` entries must be secret names and must not be an agent credential name of any backend (400 `<NAME> is an agent credential and is injected automatically`; "Secrets"); `serves_states` entries must be names of the project's `queue` states (400 otherwise) and default to `["ready"]`. Only `name` is required: `kind` defaults to `conversational`, `backend` to `claude`, `permission_mode` to `bypass`, `image` to `SESSION_IMAGE_DEFAULT` (`README.md`, "Configuration"), `idle_timeout_secs` to 1800, `partial_messages` to the kind's default and `is_default` to `false` on `POST` and to the stored value on `PUT`; `name` is 1–64 characters, `model` at most 100, `image` at most 255, `system_prompt` at most 64 KiB, `idle_timeout_secs` at least 1, and `model` and `runtime` must not be blank when given. Repeated `mcp_tools`, `secrets` and `serves_states` entries are stored once. `is_default: true` moves the flag from the project's current default; a project always keeps one, so clearing it on the default is 409. `PUT` replaces the whole profile rather than patching it, so every field the body omits takes its default again — `serves_states` back to `["ready"]`, `mcp_tools` and `secrets` back to empty, `model` and `runtime` back to null — with `is_default` the one exception.

### Sessions (`/api/projects/{pid}/sessions`, `/api/sessions/{id}`)

| Method | Path | Auth | Body → Response |
| --- | --- | --- | --- |
| GET | `/projects/{pid}/sessions` | JWT | `?state=` → `Session[]` |
| POST | `/projects/{pid}/sessions` | JWT | `{profile_id, base_ref?, title?, message?, task_id?}` → `Session` (201, `state: creating`; 400 if `base_ref` does not resolve in the mirror; 400 for an ephemeral profile with neither `task_id` nor `message`; 409 if the project is not `ready`; 409 if `task_id` names a task that is held, blocked or in a terminal state) |
| GET | `/sessions` | JWT | `?state=` → `Session[]` across all projects (dashboard) |
| GET | `/sessions/{id}` | JWT | → `Session` |
| PUT | `/sessions/{id}` | JWT | `{title}` → `Session` |
| DELETE | `/sessions/{id}` | JWT | → 204 (must be `done` or `failed`; removes the session directory) |
| GET | `/sessions/{id}/events` | JWT | `?before=<seq>&limit=<n≤500>` → `{events: AgentEvent[], has_more}` newest-last, ending just before `before` |
| POST | `/sessions/{id}/input` | JWT | `SessionInput` plus an optional `client_id` (`{kind, text, client_id?}`) → 202 (same as sending over the socket, `client_id` and all; relaunches if parked; 400 for blank `text`, `text` over 1 MiB or a `client_id` over 128 bytes; 409 for an ephemeral session) |
| POST | `/sessions/{id}/stop` | JWT | → 202 (SIGINT then SIGTERM after grace) |
| POST | `/sessions/{id}/end` | JWT | → `Session` (stop, fetch-back, `done`; from `creating` the launch is cancelled first; `failed` when the run ended that way during the stop; 409 from `done` and `failed`) |
| POST | `/sessions/{id}/retry` | JWT | `{message?}` → `Session` (conversational only, from `failed`: `parked`, then relaunched at once when `message` is given; 409 for an ephemeral session) |
| POST | `/sessions/{id}/sync` | JWT | → `{ref, commit}` (fetch the session branch into the mirror) |
| GET | `/sessions/{id}/tasks` | JWT | → `Task[]` touched by this session |

`Session = { id, project_id, profile_id, kind, created_by, title, task_id, handoff_id, state, base_ref, branch, container_id, cli_session_id, last_seq, last_activity_at, cost_usd, input_tokens, output_tokens, error, created_at, parked_at, ended_at }`. `kind` is the profile's kind at launch. `cost_usd` and the token counters are accumulated from `result` events (`ARCHITECTURE.md`, "Cost accounting"). A session becomes `running` as soon as its container is started and stdin is attached; `cli_session_id` is null until the CLI's first `init` event, which a conversational session produces only once it has been sent a message, so `running` with `cli_session_id: null` is an ordinary state and clients render it as such (`ARCHITECTURE.md`, "Launch sequence"; ADR 0032).

With `task_id`, the session claims the task in the same transaction that creates the session row, regardless of the profile's served states (the user chose), and the first input delivered to the CLI is a generated message naming the task (`You hold task #12: <title>. Call get_task to read it before starting.`), followed by `message` if given. The session holds the task until it hands it off, releases it, or ends. If `base_ref` is omitted and the task has a hand-off, selection and claim happen atomically: `handoff_id` records that hand-off and `base_ref` records its full commit id. Otherwise `handoff_id` is null and the explicit base or project default is used. The generated message also includes the current hand-off id, source branch, commit, review status and comment, including when an explicit base overrides it: a second paragraph reading `Current hand-off <handoff id> from session <source_session_id> on branch <source_branch> at commit <commit> (review: <review_status>). Hand-off comment: <comment body>`, with the source-session clause left out when that session was deleted and the comment sentence left out when the comment was. When an explicit `base_ref` overrides the hand-off, a third paragraph reads `Your checkout starts from <base_ref>, not from the hand-off commit; fetch refs/handoffs/<handoff id> before continuing that work.` For a conversational profile the generated message is queued ahead of `message` and is recorded as a `user_message` with `user_id: null`, because nobody typed it. For an ephemeral profile the generated message and `message` are joined by a blank line into the `-p` prompt and no further input is accepted. `task_id` accepts a task's UUID, its per-project number as a string, or that number as a JSON integer; a value that addresses no task of this project is a 404. `title`, when omitted, is the task's title, else the first line of `message` truncated to 80 characters, else null. A `title` given explicitly, on `POST` or on `PUT /sessions/{id}`, is 1 to 200 characters after trimming; anything else is a 400.

The `text` of an input must not be blank and is at most 1 MiB: blank text is a 400 `text must not be empty` and longer text a 400 `text too long`, on this route and over the socket alike. The cap is generous enough for a pasted log and is what keeps one request from deciding how much the orchestrator buffers. `POST /sessions/{id}/retry` accepts an absent, empty or `{}` body as "no message"; its response is the `parked` row the retry produced, even when a `message` has already started the relaunch. `POST /sessions/{id}/end` answers `done` for a conversational session, and `failed` when the run ended that way while the stop was in flight — which is every ephemeral session, since an ephemeral session is never parked (ADR 0003) — because the lifecycle has no `failed → done` edge (`ARCHITECTURE.md`, "Session lifecycle"). Either way the container is gone, the branch has been fetched back and the session holds no tasks. `end` is accepted while the session is still `creating` — a user who launched by mistake does not wait for the container — and answers `done`: the launch is cancelled, whatever container it had got as far as creating is removed, and the tasks the session held are released exactly as any other end releases them (`ARCHITECTURE.md`, "Session lifecycle", "A session ended while it is creating"). Only `done` and `failed` are refused, with `session is <state>`. A launch that reaches `running` first is ended from there instead, so the answer is the ordinary `done` and the call takes as long as that stop does.

`base_ref` on `POST` records the name the caller gave — an integration head, an upstream-tracking ref, a tag or a commit id — and not the commit it resolved to; a name that does not resolve in the project repository is a 400. `state` on either list endpoint is one of the five lifecycle values and anything else is a 400. On `GET /sessions/{id}/events`, `limit` defaults to 100 and a `limit` outside 1–500, or a `before` below 1, is a 400; `before` is exclusive, so paging backwards passes the lowest `seq` of the page just received.

### Task states (`/api/projects/{pid}/task-states`)

The columns of a project's board, in order. Every project starts with the default set listed in `docs/data-model.md`, `task_states`. A state's `kind` says what it means to the orchestrator: `queue` states are where agents pick work up, the single `human` state is where escalations land, `terminal` states close a task and satisfy dependencies.

| Method | Path | Auth | Body → Response |
| --- | --- | --- | --- |
| GET | `/projects/{pid}/task-states` | JWT | → `TaskState[]` ordered by `position` |
| POST | `/projects/{pid}/task-states` | JWT | `{name, kind, position?}` → `TaskState` (201; 400 for an invalid name or kind; 409 if the name is taken or the project already has a `human` state) |
| PUT | `/projects/{pid}/task-states/{name}` | JWT | `{name?, position?}` → `TaskState` (400 if `kind` is given: kind is immutable; 409 if the new name is taken) |
| DELETE | `/projects/{pid}/task-states/{name}` | JWT | → 204 (409 while any task is in the state, for the `human` state, for the last `queue` state, and for the last `terminal` state) |

`TaskState = { id, project_id, name, kind: "queue" | "human" | "terminal", position, created_at }`. `name` is 1–32 characters matching `[a-z0-9][a-z0-9_-]*`. A missing `position` appends; an explicit one shifts the states at and after it. Renaming a state renames it everywhere at once, since tasks and profiles reference states by id. Every change emits a `states_changed` `TaskEvent`. The board refreshes states and tasks together through the same event-triggered REST refresh path used for other task changes (see "Frontend"; ADR 0022).

### Tasks (`/api/projects/{pid}/tasks`)

| Method | Path | Auth | Body → Response |
| --- | --- | --- | --- |
| GET | `/tasks` | JWT | `?state_kind=human` → `Task[]` across all projects (dashboard) |
| GET | `/projects/{pid}/tasks` | JWT | `?state=&label=&priority=&parent=&held=` → `Task[]` ordered by priority, then number |
| POST | `/projects/{pid}/tasks` | JWT | `{title, description?, state?, priority?, labels?, parent_id?, depends_on?: id[]}` → `Task` (201; 400 for an unknown state) |
| GET | `/projects/{pid}/tasks/{id}` | JWT | → `TaskDetail` |
| PUT | `/projects/{pid}/tasks/{id}` | JWT | `{title?, description?, state?, priority?, labels?, parent_id?, assignee_user_id?, handoff?: HandoffInput}` → `Task` |
| DELETE | `/projects/{pid}/tasks/{id}` | JWT | → 204 |
| POST | `/projects/{pid}/tasks/{id}/dependencies` | JWT | `{depends_on: id, kind?: "blocks" \| "discovered_from" \| "related"}` → `Task` (`kind` defaults to `blocks`; 409 on cycle) |
| DELETE | `/projects/{pid}/tasks/{id}/dependencies/{dep}` | JWT | `?kind=blocks` → `Task` (200; removes only that kind; accepts `blocks`, `discovered_from`, `related`; 404 `dependency not found` when no edge of that kind exists) |
| POST | `/projects/{pid}/tasks/{id}/comments` | JWT | `{body}` → `Comment` (201) |
| POST | `/projects/{pid}/tasks/{id}/release` | JWT | → `Task` (clears the lease, keeps the state; 409 if nobody holds it) |
| GET | `/projects/{pid}/tasks/stream` | JWT (`?token=`) | SSE of `TaskEvent`; see "SSE" |

`{id}` and `{dep}` accept a task's UUID or its per-project number.

Tracker mutations from REST, MCP and background jobs are serialized per project and validated against state read after acquiring the project lock. Each logical change, its dependency/parent effects, comments and events commit together. A concurrent request waits, then either succeeds against the resulting state or returns the existing validation/conflict error; it cannot publish a partial change. Different projects proceed independently (ADR 0021).

`Task = { id, project_id, number, title, description, state, priority, blocked, labels, parent_id, assignee_user_id, lease_holder_session_id, lease_since, attempts, needs_human_reason, handoff: Handoff | null, depends_on: {task_id, kind}[], blocks: id[], created_at, updated_at, closed_at }`. `state` is the state's name. `depends_on` lists every outgoing dependency with its kind; `blocks` lists the tasks that have a `blocks` dependency on this one. `TaskDetail = Task & { comments: Comment[], handoffs: Handoff[], children: Task[], sessions: {session_id, first_touched_at, last_touched_at}[] }`. Hand-offs are ordered oldest first. `Comment = { id, task_id, author_user_id, author_session_id, system, body, created_at }`.

A task's state is the queue it waits in, and the lease says who is working on it (`ARCHITECTURE.md`, "Task tracker"). There is no in-progress state: a held task is in progress in whatever state it is in. A user may set any state through `PUT` and is not bound by leases. Changing to a different state hands the task off, which clears the lease and resets `attempts`; a terminal state sets `closed_at` and unblocks dependants; a non-terminal state on a closed task reopens it. Setting the current state is a state no-op: it preserves the lease, `attempts` and `closed_at`, and emits no state-change or escalation event. Other supplied fields are still updated normally; a request with no effective changes emits no task event. A user `release` keeps the state and never escalates; agent and reaper releases escalate to the `human` state once `attempts` reaches the project's `max_attempts`. `priority` is 0 (critical) to 3 (low), default 2. `parent_id` nests one level: a task that has children cannot be given a parent, and a task that already has a parent cannot receive children (400). The parent must be a different task in the same project. These checks include terminal children and apply to both creation and re-parenting. On `PUT`, `parent_id: null` makes a task top-level again and `assignee_user_id: null` unassigns it; an `assignee_user_id` naming no user is 400 `unknown assignee`. Deleting a task is not blocked by its lease, and its children survive it as top-level tasks; losing its last open child does not close a parent, since closure follows a state change. A `blocks` dependency on a non-terminal task marks the dependant `blocked`; the other kinds are informational. A non-terminal child blocks its parent the same way, and when the last open child closes the parent is moved to the project's first terminal state by the system (`state_changed` with actor `system`); a parent is never reopened automatically (`ARCHITECTURE.md`, "Task tracker", "Parents"). `number` is assigned from the project's counter and never reused.

Dependencies are identified by `(task, depends_on, kind)`: the same pair may carry both `blocks` and `discovered_from`, and removing one leaves the other intact. Removing an edge that is not there answers 404 `dependency not found`, which is distinct from the generic 404 `not found` an unknown project, `{id}` or `{dep}` gets: both tasks resolved, so a caller can read the named body as "it is already gone" rather than as a task that vanished. Only `blocks` edges participate in dependency-cycle checks. Deleting a prerequisite removes its incident edges and recomputes every surviving dependant’s `blocked` flag in the same transaction, taking remaining prerequisites and children into account. Emit `dependency_removed` for each affected surviving dependant, plus `blocked`/`unblocked` on any flag change. The deleted task retains its event identity as specified under "TaskEvent" (ADR 0022).

### Code hand-offs and review

A task's current `handoff` identifies committed work for the next agent. State names do not determine whether something is code or whether it has been approved. A planning-only state move needs no hand-off and preserves any existing one.

```ts
type HandoffInput =
  | { kind: "revision"; source_session_id?: string; commit: string; comment: string }
  | { kind: "forward"; handoff_id: string; comment: string;
      review?: "approved" | "changes_requested" };
```

`handoff` requires a different target `state` in the same update (400 otherwise) and a non-empty `comment`. For REST revision publication, `source_session_id` is required and must belong to the task's project. A REST caller is a user and is therefore not lease-bound: they may hand off a task nobody holds or one another session holds — the lease is cleared by the state change — and may name any session of the project with a synced branch as the source, not only a session linked to the task. An MCP caller is a session and may only hand off a task it holds. For MCP it is omitted and derived from the caller; a supplied source session id is rejected. `commit` is a full object id, not a moving ref: sync must produce that exact tip or return 409 / MCP `conflict`. The commit is retained under an internal immutable ref before the task moves. Uncommitted changes are excluded; committing them is the agent's responsibility. The hand-off comment, record, state change, lease release and task events commit together. Sync failure leaves the task and lease unchanged.

The 400 messages for these input rules are exact, and the same for REST and MCP: `handoff requires a different target state` (no `state`, or the state the task is already in), `comment body must not be empty`, `commit must be a full lowercase hexadecimal git object id`, `revision hand-off requires source_session_id`, `source_session_id is derived from the calling session` (an MCP caller supplied one), `source_session_id must name a session of this project` (a revision naming another project's session, or one since deleted), `session has no synced branch yet` (a source session that has produced no branch, such as one still `creating`), and `review applies to forward hand-offs only` (a `review` sent with a `revision`). A malformed body — an unknown `kind`, a missing required field — is a 400 too. Unknown fields are ignored, as elsewhere in the API.

The 409 messages of publication are exact in the same way: `project is not ready` (the project repository is not usable yet, checked before any git work), `task is not held by the calling session` (an MCP caller that does not hold the lease), `session branch tip <tip> does not match commit <commit>` (the synced revision source is not at the requested commit), `handoff_id is not the task's current hand-off` (forwarding anything but the current record, including a task that has none), `commit is not present in the project repository` (the commit is unknown to the mirror or is not a commit), and `task changed during hand-off publication; re-read it and retry` (the task's state, lease holder or current hand-off moved between preparation and publication). MCP reports each of them as `conflict` with the same text, and the 400 messages above as `invalid_argument`.

Forwarding requires `handoff_id` to equal the task's current hand-off (409 / MCP `conflict` otherwise). It reuses the source branch and pinned commit, and optionally records an explicit review decision on that commit. No decision means the previous review status and reviewer are carried forward. A new revision always resets review status to `unreviewed`; old approvals stay in history. Ordinary moves, releases and automatic parent closure do not change the current hand-off or grant approval.

`Handoff = { id, task_id, source_session_id, source_branch, commit, comment_id, review_status: "unreviewed" | "approved" | "changes_requested", reviewed_by_user_id, reviewed_by_session_id, reviewed_at, created_by_user_id, created_by_session_id, created_at }`. Actor and source-session ids may become null after deletion; the source branch, commit and review timestamp remain. Review and creation actors come from authenticated context, never input. `comment_id` points to the comment written with this hand-off. Every state event carries the full task including its current hand-off; the accompanying `commented` event carries the comment.

Example: an implementer moves a task to `review` with a revision hand-off at commit A. The reviewer launches from A and forwards that hand-off to `merge` with `review: "approved"`. The task merge uses A, even if the implementer's branch has since advanced to B. To include B, publish a new revision and review it. A rejection forwards A with `changes_requested` to the chosen implementation state; the next implementer starts from A and publishes a new unreviewed revision after fixes.

### Git (`/api/projects/{pid}/git`)

| Method | Path | Auth | Body → Response |
| --- | --- | --- | --- |
| GET | `/projects/{pid}/git/session-branches` | JWT | → `SessionBranch[]` |
| GET | `/projects/{pid}/git/diff` | JWT | `?head=&base=` or `?handoff_id=&base=` → `Diff` (`base` defaults to the project's default branch) |
| POST | `/projects/{pid}/git/merge` | JWT | `MergeInput` → `{commit}` or 422 `{status, error, conflicts}` (409 for stale or unapproved task hand-off) |
| POST | `/projects/{pid}/git/rebase` | JWT | `{branch, onto}` → `{commit}` or 422 `{status, error, conflicts}` |
| POST | `/projects/{pid}/git/push` | JWT | `{ref, remote_branch?, force?: boolean = false}` → `{remote_branch, commit}` |

`SessionBranch = { session_id, ref: "refs/sessions/<id>", commit, ahead, behind, base: default_branch, updated_at }`. Ahead/behind is measured against the Mars integration head named by `default_branch`. `source`, `head` accept a session id, integration branch name or upstream-tracking name such as `origin/main`; `onto`, `base` accept integration or upstream-tracking branch names. `branch`, `ref` accept a session id or integration branch name; `target` accepts an integration branch name. Fully qualified refs in these namespaces are accepted to disambiguate names; upstream-tracking refs cannot be mutation targets or push sources (400). Merging `origin/main` into `main` explicitly integrates fetched upstream changes. `Diff = { base, head, merge_base, files: {path, status, additions, deletions}[], patch, truncated }`; a session `head` is synced first, and `patch` is the unified diff from `merge_base` to `head`, truncated above 1 MiB. A non-fast-forward push returns 409 and preserves local work; merge/rebase conflicts remain 422 with conflicting paths. A push updates only the selected upstream branch, never all refs. REST force-pushes require explicit `force: true`; MCP additionally requires the profile's `push` permission.

`MergeInput = { target, message? } & ({ source } | { task_id, handoff_id })`; the alternatives are mutually exclusive (400 otherwise). The task form requires the task's current hand-off to match `handoff_id` and be `approved`, and merges its exact retained commit without syncing a newer source-session tip; a hand-off that is not the task's current one is 409 `handoff_id is not the task's current hand-off` and one that is not approved is 409 `hand-off is not approved`, in both cases with the target unchanged, and a `task_id` this project does not have is 404. Without `message` the task form's merge commit reads `Merge handoff <handoff_id> (<source_branch>) into <target>`. The branch form is an explicit generic git action and does not imply task approval. The same profile gate applies to both MCP forms. Internal hand-off refs are not returned as branches; task details expose them by hand-off id. Diff accepts exactly one of `head` or `handoff_id` (400 otherwise); a hand-off must belong to the URL project (404 otherwise, whether the id is another project's or no hand-off at all) and selects its immutable commit without fetch-back.

### Secrets (`/api/secrets`)

| Method | Path | Auth | Body → Response |
| --- | --- | --- | --- |
| GET | `/secrets` | JWT | `?scope=&scope_id=` → `SecretMeta[]` (user scope returns the caller's; admins may select another user with `scope_id`) |
| POST | `/secrets` | JWT | `{scope, scope_id?, name, value, orchestrator_only?}` → `SecretMeta` (201; 409 if exists; 409 if `name` is an agent credential and the scope already holds one for the same backend; 400 for an agent credential with `orchestrator_only: true`) |
| PUT | `/secrets/{id}` | JWT | `{value}` → `SecretMeta` (replace value) |
| PATCH | `/secrets/{id}` | JWT | `{name?, orchestrator_only?}` → `SecretMeta` (rename re-encrypts under new AAD; the two agent-credential rules of `POST` apply to the resulting row) |
| DELETE | `/secrets/{id}` | JWT | → 204 |
| GET | `/secrets/{id}/uses` | JWT | `?limit=` → `{session_id, user_id, purpose, at}[]` |
| GET | `/projects/{pid}/agent-credentials` | JWT | → `AgentCredentialStatus[]`, one entry per backend (404 for an unknown project) |

`SecretMeta = { id, scope, scope_id, name, orchestrator_only, key_version, created_by, created_at, updated_at, last_used_at, credential_for }`. No response ever contains `value`. `credential_for` is the backend (`"claude"`) whose agent credential the name is, else null; it is derived from the name and not stored. User-scoped secrets are listed, changed and deleted only by their owner or an admin (403 otherwise); global and project secrets by any user. With no `scope`, `GET /secrets` returns the global secrets, every project's and the caller's own user-scoped ones — every user's for an admin; a `scope_id` without a `scope` narrows nothing. `GET /secrets/{id}/uses` returns 50 uses without `?limit=` and never more than 500: a larger `?limit=` is reduced to 500 rather than refused, and `?limit=0` is a 400.

**Agent credentials** (ADR 0036; `ARCHITECTURE.md`, "Secrets", Agent credentials). Each backend names the secrets its CLI authenticates with; for `claude` they are `CLAUDE_CODE_OAUTH_TOKEN` and `ANTHROPIC_API_KEY`. They are created, replaced and deleted through the endpoints above like any secret, under three rules. One scope (`scope` with `scope_id`) holds at most one credential per backend: a create or a rename that would give it a second is 409 `this scope already has an agent credential (<NAME>); replace or delete it first`. A credential is never `orchestrator_only`: 400 `an agent credential cannot be orchestrator-only`. And it is injected into every session of its backend without being declared, so a profile may not list it.

`AgentCredentialStatus = { backend, credential: { secret_id, name, scope } | null }`. `GET /projects/{pid}/agent-credentials` answers, for each backend, which credential a session of that project launched by the caller would be given: the row at the most specific of the caller's `user` scope, the project's scope and `global`, whichever of the backend's names it carries, or null when there is none. It decrypts nothing, writes no `secret_uses` row and never contains a value. A user-scoped row in the answer is always the caller's own.

### Health

`GET /api/health` → `{ orchestrator: true, database: bool, engine: bool }` with 200 or 503. Unauthenticated, for compose health checks.

### Test-only routes

Compiled only with the `integration-tests` cargo feature, never into a release build.

| Method | Path | Auth | Body → Response |
| --- | --- | --- | --- |
| POST | `/test/users` | — | `{username, email, password, admin?}` → `{user, access_token}` (201; sets the refresh cookie; `must_change_password` false) |
| GET | `/test/stream-whoami` | `?token=` | → `{user_id}` (200). The stream `?token=` check on an ordinary request, so the shared authentication contract of the WebSocket and SSE endpoints can be asserted without opening a stream: 401 `authentication required`, 403 `password change required` and 200 exactly as they answer. |

Playwright creates its users through this route; sessions in end-to-end tests use the stub image (`ARCHITECTURE.md`, "Session image").

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

The server also sends one `session` message immediately after the upgrade, before replay, so the client has the current state.

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
  | { kind: "message"; text: string };   // the only kind: the agent never asks the host a question
```

`message` is the only kind, and an unknown one is refused (400 over REST, `input_rejected` over the socket); the same holds for blank `text` and for `text` over the 1 MiB cap (see "Sessions"). The CLI is launched with `--permission-mode bypassPermissions --permission-prompts none` and, as recorded against the pinned version, never waits on stdin for an answer, so there is no `answer` input and no `prompt` event to answer (ADR 0033; `ARCHITECTURE.md`, "Claude Code invocation"). `client_id` is a client-generated string echoed back so optimistic UI can reconcile, at most 128 bytes (over it: 400 over REST, `input_rejected` with `client_id too long` over the socket). `seq` in `input_accepted` is the highest committed sequence at acceptance; the `user_message` event recording the input follows later with the same `client_id`. `POST /sessions/{id}/input` accepts the same `client_id` beside the `SessionInput` fields and echoes it on that `user_message` in the same way, so a client falling back to REST while its socket is closed still reconciles; a body without one is unchanged and produces a `user_message` with no `client_id`. A `message` is accepted for a conversational session in `creating`, `running` or `parked`; ephemeral sessions reject all additional input, matching the REST contract, and a refused input is answered with `input_rejected` carrying the reason. The orchestrator sends WebSocket pings every 30 seconds and closes after two missed pongs.

**Known v1 restart limitation (ADR 0020).** `input_accepted` and HTTP 202 from `/sessions/{id}/input` acknowledge acceptance by the orchestrator, not guaranteed delivery to or execution by the CLI. Input queues are in memory; a restart can lose queued messages or leave delivery uncertain even when a `user_message` is in history. `client_id` is not a durable idempotency key. Reconnecting replays output events but does not automatically resend inputs. There is no new delivery-status UI in v1; users may inspect and manually resend, accepting that this can repeat work.

The terminal is an `exec` with a PTY into the session container running `/bin/bash -l` as the `agent` user, multiplexed onto the same socket with binary frames. It is an escape hatch for inspection; nothing it does is recorded as events.

A `terminal_open` that cannot be honoured (session not `running`, container gone) answers `terminal_closed` with `exit_code: -1`. The socket stays open: `error` is followed by a close, and a terminal that could not start is not a reason to end the transcript stream. The same `exit_code: -1` is what a terminal that ended without the engine reporting a code carries. A second `terminal_open` while one is open, a `terminal_resize` or `terminal_close` with none open, and binary frames with none open are all ignored.

## SSE: task stream

`GET /api/projects/{pid}/tasks/stream?token=<jwt>` with optional `Last-Event-ID: <seq>` header (or `?after=<seq>` for the first connection).

Each SSE message has `id: <seq>`, `event: task`, and `data: <TaskEvent JSON>`. The response body opens with a `: ready` comment, written before any replay so the first body byte follows the headers immediately: a stream with nothing to replay must not wait for its first keepalive, because a proxy that holds a response's headers until its first body byte would delay the client's `open` event by that long. Clients ignore comment frames. A `: keepalive` comment is sent every 15 seconds, on its own cadence, unaffected by the opening comment. The server supports replay from `Last-Event-ID`; the browser client disables automatic reconnect and explicitly reopens with a refreshed token and `?after=<lastSeq>` as specified under "Authentication". nginx must serve this location with buffering off.

The server subscribes to project notifications before opening the SSE response, then replays events and follows live changes, with the existing periodic safety read for missed notifications. The board waits for the stream's open event before its initial REST load; when explicitly recreating `EventSource` with a refreshed token it passes `?after=<lastSeq>`. It refreshes authoritative board data on open/reopen and on each new task event, using the loading rules under "Frontend". Historical event `task_id` values, including `deleted` events, survive deletion of the task row.

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
  | { kind: "user_message";       text: string; user_id: string | null; client_id?: string }
  | { kind: "text_delta";         text: string }                                  // partial messages only
  | { kind: "text";               text: string }                                  // complete assistant text block
  | { kind: "thinking";           text: string; redacted: boolean }
  | { kind: "tool_call";          tool_use_id: string; name: string; input: unknown }
  | { kind: "tool_result";        tool_use_id: string; content: string | unknown; is_error: boolean; truncated: boolean }
  | { kind: "permission_denied";  tool_use_id?: string; name: string; reason: string }
  | { kind: "subagent_start";     tool_use_id: string; description: string; agent_type?: string }
  | { kind: "subagent_end";       tool_use_id: string; is_error: boolean }
  | { kind: "result";             subtype: string; terminal_reason?: string; is_error: boolean; num_turns: number; duration_ms: number; cost_usd?: number; usage?: unknown; permission_denials: unknown[] }
  | { kind: "error";              message: string; fatal: boolean }
  | { kind: "state_change";       from: SessionState; to: SessionState; reason: string; signal?: "SIGINT" | "SIGTERM" }
  | { kind: "launch_warning";     message: string }                              // e.g. a declared secret no scope defines, or no agent credential
  | { kind: "git";                op: "sync" | "merge" | "rebase" | "push"; ok: boolean; detail: unknown }
  | { kind: "raw";                backend: "claude"; native: unknown }            // untranslated native line
);
```

Agent/tool output and user-provided transcript content may contain secrets. v1 applies no automatic secret detection or redaction before storing or displaying this content, including `tool_call`, `tool_result`, `text`, `thinking`, `user_message` and `raw` payloads (ADR 0027). Normal translation and size limits still apply. The `thinking.redacted` flag reflects backend-provided redaction; it is not a Mars secret-filtering guarantee. Orchestrator-generated diagnostics and event metadata must not copy values from credential handling.

The `git` event's `detail` is one shape per `op`:

```ts
type GitDetail =
  // op: "sync"
  | { ref: string; commit?: string; error?: string }
  // op: "merge"
  | { source: string; target: string; commit?: string; fast_forward?: boolean; conflicts?: string[]; requested_by: string; error?: string }
  // op: "rebase"
  | { branch: string; onto: string; commit?: string; conflicts?: string[]; work_tree?: "updated" | "reconciliation_required" | "not_applicable"; requested_by: string; error?: string }
  // op: "push"
  | { ref: string; remote_branch: string; commit?: string; force: boolean; compare_url?: string; requested_by: string; error?: string };
```

Optional fields are omitted rather than null: `commit`, `fast_forward` and `compare_url` appear when `ok` is true, `conflicts` and `error` when it is false. `ref`, `source`, `target`, `branch` and `onto` are API ref names, except a task merge's `source`, which is the hand-off id. `requested_by` is `user:<uuid>`, `session:<uuid>` or `system`, the same value the commit's `Requested-By` trailer carries. `error` is the generic user-facing message from the same failure the REST call would answer with — never git's stderr and never a credential. The event is written on every session whose ref took part and on the calling session when an agent asked; a task merge takes no session ref, so its participant is the hand-off's source session, when that session still exists (`ARCHITECTURE.md`, "Git model", Merge, rebase, push).

Translation rules for the Claude backend, from `stream-json` lines. Every rule below is measured against the native shapes recorded from the pinned CLI version (`images/claude/VERIFY.md`, "Observed on 2.1.274"; the recording itself is `images/stub/fixtures/default.jsonl`), and a line the translator drops is still in the transcript file on the session volume.

- `system`/`init` → `init`, **once per process**. The CLI writes an `init` line at the start of every turn, all carrying the one `session_id` of the process; under `--input-format stream-json` even the first of them arrives only after the process has read a stdin line, so the event follows the session's first message rather than its launch (`ARCHITECTURE.md`, "Launch sequence"). The first line becomes the event, and each later line that repeats that `session_id` only refreshes the translator's state and produces nothing. A line reporting a different `session_id` is a different conversation and produces a new `init`. `session_id` is stored as `cli_session_id`; `resumed` is true when the launch used `--resume`, which no native field reports — a resumed launch's `init` carries the same field set and repeats the resumed `session_id` (`ARCHITECTURE.md`, "Claude Code invocation"). `model` and `tools` are both present on every recorded `init` of the pinned version; `model` stays optional in the schema so a backend that omits it needs no new kind, `tools` does not. Each `mcp_servers` entry contributes its `name` and its `status` verbatim; the native entry's `source` (where the CLI found the server, `dynamic` for one passed with `--mcp-config`) is not forwarded. An unreachable server is reported with `status: "failed"`, which is one of the statuses the launcher's `launch_warning` covers (`ARCHITECTURE.md`, "MCP design").
- `system`/`permission_denied` → `permission_denied`, taking `name` from `tool_name`, `tool_use_id` from `tool_use_id` and `reason` from `message`. The line carries no `decision_reason`, and `decision_reason_type` is not forwarded. A `tool_result` with `is_error: true` follows and is translated as any other tool result.
- `system` subtypes that report the CLI's own progress — `status`, `thinking_tokens`, `api_retry`, `task_started`, `task_progress`, `task_updated`, `task_notification`, `vcs_state_changed` — produce no event: the `task_*` run is what `subagent_start`, the subagent's own events and `subagent_end` already describe, and the rest is per-request telemetry. An `api_retry` line still feeds the authentication rule below. Any other `system` subtype → `raw`.
- `rate_limit_event` (a top-level type) produces no event: it reports the account's rate-limit windows and utilisation, which belongs to the credential rather than to this session's transcript. Like every native line it is never logged above `debug` and never with its fields.
- `assistant` messages → one event per content block: `text`, `thinking`, `tool_call`. A `thinking` block whose text is empty produces nothing — the pinned CLI forwards a `signature` and no text, and an event with nothing to read is not stored — while a `redacted_thinking` block still produces `thinking` with `redacted: true`. A `tool_call` whose name is the subagent tool additionally emits `subagent_start`. The subagent tool has been called `Task` and `Agent` in different CLI versions (2.1.274 lists it as `Task` in `init.tools` and names it `Agent` in the `tool_use` frames it writes); the translator matches both from one constant, and a fixture per pinned CLI version proves the constant is still right.
- `user` messages produced by the CLI (tool results) → `tool_result` per block; a result for a subagent tool call additionally emits `subagent_end`. A `user` line never produces a `user_message`: that kind is written by the owner when it writes the input. A `user` line whose content is only text is one of three things: the CLI's echo of a message the orchestrator wrote, dropped by matching on content hash; `[Request interrupted by user]`, the line a stop produces, dropped because the `state_change` written for the stop already says it (`ARCHITECTURE.md`, "Stop semantics"); or a message the orchestrator did not write, kept as `raw`. The first frame of a subagent is such a line, carrying the subagent's prompt and its `parent_tool_use_id`; a line with a `parent_tool_use_id` is never matched against the input hashes, because an input the orchestrator wrote goes to the top-level conversation.
- `stream_event` (only with `--include-partial-messages`) → `text_delta` for a `content_block_delta` whose delta is a `text_delta`. Every other stream event is dropped because the complete block follows in the `assistant` message: `message_start`, `message_delta`, `message_stop`, `content_block_start`, `content_block_stop`, and the `thinking_delta`, `signature_delta` and `input_json_delta` deltas.
- `result` → `result`, with `cost_usd` taken from the native `total_cost_usd` and `terminal_reason` passed through as the CLI wrote it (`completed` for a turn that ended by itself, `aborted_streaming` for one a stop interrupted). `total_cost_usd` is cumulative for the process, so the owner accumulates its increase over the previous `result`, while `usage` is the turn's own and is summed as it arrives (`ARCHITECTURE.md`, "Cost accounting"). Each entry of `permission_denials` — `{tool_name, tool_use_id, tool_input}` — additionally produces a `permission_denied` event, unless the turn's own `system`/`permission_denied` line already reported that `tool_use_id`. For an ephemeral session the owner then stops the container and emits a `state_change` to `done`.
- Authentication failure → one `error` with `fatal: true`, at most once per process. The CLI reports a rejected credential as a run of `system`/`api_retry` lines with `error_status: 401` and `error: "authentication_failed"`, then a synthetic assistant message and a `result`, and exits 1; the event is emitted on the first line that reports it and names the injected variable and its scope, never a value (`ARCHITECTURE.md`, "Claude Code invocation", Credentials). A `result` is only read for this when it has `is_error: true`, so an agent that discussed an authentication error cannot park its own session.
- Anything else → `raw`.

Every event emitted from a native message that carries `parent_tool_use_id` copies it. The frontend groups events by that id under the corresponding `tool_call`.

## TaskEvent

```ts
interface TaskEvent {
  seq: number;                 // per-project, monotonic
  ts: string;
  task_id: string | null;      // original task UUID, retained after deletion; null on "states_changed"
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

`state_changed` is any hand-off, by an agent, a user, or the system when a parent closes with its last child. `escalated` is a move into the project's `human` state, whether an agent asked for it (`needs_human`) or the reaper ran out of attempts; it carries `from`/`to` like a state change plus the reason. `released` is a lease cleared without a state change. `blocked`/`unblocked` fire when a dependency change or a hand-off flips a dependant's `blocked` flag.

`deleted` carries the original task UUID in `task_id` and omits `task`; earlier events keep their original identifiers and payloads. The board deduplicates by `seq` and uses events as refresh signals, never applies historical `task` or `states` payloads over a REST snapshot, and stores no raw event queue (ADR 0022). This includes `states_changed`, which refreshes both columns and cards.

## MCP tool contracts

Served at `http://orchestrator:7001/mcp` (Streamable HTTP), bearer-authenticated per session. The session context supplies `session_id`, `project_id` and the profile. Tools are listed to a session only if allowed for its profile; task tools are always allowed. Successful tracker changes commit their `TaskEvent` rows and the calling session's link to the directly changed task together. `ready` and `get_task` do not write tracker events, create session-task links, or advance touch timestamps. Rejected tracker operations and updates with no effective changes likewise leave tracker history and links unchanged. `list_session_branches` emits no session `git` event; git operations retain their existing outcome-event contract. Backend-reported tool calls/results remain part of the ordinary transcript, including reads and failures. There is no separate persistent MCP read-audit log in v1 (ADR 0030).

Tool descriptions are part of the contract because they steer the agent. They are reproduced verbatim.

Every `task` argument accepts a task's UUID or its per-project number (as a number or a string such as `"12"` or `"#12"`). A task's `state` in inputs and outputs is the state's name. The tracker's rules (state as queue, lease as worker, attempts) are in `ARCHITECTURE.md`, "Task tracker".

Where a tool section below quotes no message, the answer is the tracker's or the git service's own, in the words the REST endpoint behind the same operation gives: `task is not held by this session` for a caller that holds neither the lease nor the creator exception on `update` and `release` (the hand-off path keeps the `task is not held by the calling session` of "Code hand-offs and review"), `unknown state "<name>"; valid states are: <names in position order>` as `invalid_argument` for an unknown `state`, and `parent must be a top-level task of the same project` as `invalid_argument` for an unusable `parent` on `create_task` or `update`, including one that names no task at all. A `task`, `parent`, `depends_on`, `add_depends_on` or `remove_depends_on` value that is not a task reference at all is `invalid_argument` `task must be a UUID, a number, or "#<number>"`; any other argument that does not fit its type is `invalid_argument` `invalid arguments: <reason>`.

### `ready`

Description: "List tasks you could start now, in the states your profile serves. Claim one with `claim` before doing any work on it. If you were launched for a task you already hold it: call `get_task` on it instead."

Input `{ limit?: number = 20 }`. Output `{ tasks: TaskSummary[] }` where `TaskSummary = { id, number, title, state, priority, labels, description_excerpt, attempts, depends_on_count }`. Returns tasks in the calling profile's served states that are not blocked and have no lease holder, ordered by `priority` then `number`. A profile that serves no states gets an empty list.

An explicit `limit` must be an integer from 1 through 100 inclusive; other values return `invalid_argument` and are never clamped. To produce `description_excerpt`, trim the description, replace each newline sequence (CRLF, LF or CR) with one space, then take the first 200 Unicode scalar values and trim trailing whitespace, without adding an ellipsis. `depends_on_count` counts outgoing dependencies of every kind.

### `claim`

Description: "Claim a task before you start it. You hold it until you hand it off with `update`, give it back with `release`, or your session ends. If the claim fails someone else has it: pick another."

Input `{ task: string | number }`. Output `{ task: Task }` or MCP error `conflict` with message "task is not claimable" when the atomic update returns zero rows. Only tasks in the profile's served states can be claimed; a task outside them returns `conflict` with message "task is not in a state this profile serves". The returned task includes its hand-off. Claiming in an existing session never resets its checkout; the agent must fetch and inspect the recorded commit before reviewing or continuing that work. The retained ref can be fetched from the session clone's `origin` as `refs/handoffs/<handoff.id>`.

### `get_task`

Description: "Read a task in full: description, comments, dependencies, sub-tasks and which sessions worked on it. Read it before you start; the comments are where earlier agents and humans left context for you."

Input `{ task: string | number }`. Output `{ task: TaskDetail }` (same shape as REST).

### `update`

Description: "Update a task you hold. Changing to a different `state` ends your hold. When handing over code, commit it first and include a revision hand-off with the exact commit and a comment describing the work and checks. When reviewing, forward the existing hand-off with your decision; do not substitute your own branch."

Input `{ task: string | number, state?: string, title?: string, description?: string, priority?: number, labels?: string[], parent?: string | number | null, add_depends_on?: string[], remove_depends_on?: string[], handoff?: HandoffInput }`. Output `{ task: Task }`. Rules: the caller must hold the lease, except for `title`, `description`, `labels`, `add_depends_on` and `remove_depends_on` on tasks the caller created that nobody holds. `state` must be one of the project's states; an unknown name returns `invalid_argument` with the valid names in the message. Changing to a different state releases the lease and resets `attempts`; a terminal state also sets `closed_at` and recomputes `blocked` on dependants. Setting the current state preserves the lease and counters, as in REST. The same one-level parent rules apply; invalid nesting returns `invalid_argument`. Hand-off inputs obey "Code hand-offs and review", whose exact 400 and 409 messages are returned here as `invalid_argument` and `conflict` with the same text; revision publication uses the calling session and requires no git-tool permission, because it only syncs and retains committed work inside the project. `add_depends_on` creates `blocks` dependencies; a cycle returns `invalid_argument`, an entry naming no task of the project returns the generic `not_found` the REST dependency endpoint gives an unknown `{dep}`, an entry whose end is in another project returns `invalid_argument` `dependency must reference tasks of the same project`, and an edge that is already there returns `conflict` `dependency already exists`. `remove_depends_on` removes only `blocks` edges, preserving any provenance or related edge for the same pair; a named task with no `blocks` edge to remove returns `not_found` with the same `dependency not found` message the REST endpoint gives, so the agent can tell it apart from a task it named wrong.

### `release`

Description: "Give a task back without finishing it, and say why. Use this when you cannot make progress. The task stays in its state for another agent; after too many attempts it goes to a human instead."

Input `{ task: string | number, reason: string }`. Output `{ task: Task }`. The caller must hold the lease. The reason is recorded as a comment. If `attempts` has reached the project's `max_attempts`, the task moves to the project's `human` state with `needs_human_reason` set, and the output `task` shows that state.

### `comment`

Description: "Leave a comment on a task. This is how you talk to other agents and to humans: say what you found, what you changed, what you need. Comment before you hand off."

Input `{ task: string | number, body: string }`. Output `{ comment: Comment }`. Any session in the project may comment on any task.

### `needs_human`

Description: "Hand a task to a human when you are blocked on a decision, credentials, or anything you must not decide alone. Say exactly what you need."

Input `{ task: string | number, reason: string }`. Output `{ task: Task }`. Moves the task to the project's `human` state, sets `needs_human_reason`, releases the lease and resets `attempts`. Requires holding the lease or the task being unheld. If it is already in the human state, record the reason and explicitly release any held lease, but preserve `attempts`; emit `commented`, `updated` and, when applicable, `released`, with no new `escalated` event or escalation email. This explicit release action is distinct from assigning the current state through `update`.

### `create_task`

Description: "Create a task when you discover work outside what you hold: a follow-up, a bug, or a sub-task of a plan. Put it in the state that matches how ready it is (`backlog` if it still needs planning). Link it with `depends_on` if it must wait, and with `parent` if it is part of a larger task. If you hold several tasks, identify the originating task with `discovered_from`."

Input `{ title: string, description?: string, state?: string, priority?: number = 2, labels?: string[], parent?: string | number, depends_on?: string[], discovered_from?: string | number }`. Output `{ task: Task }`. `state` defaults to the project's default state (its first `queue` state); an unknown name returns `invalid_argument`. `depends_on` creates `blocks` dependencies; unlike `update`'s `add_depends_on`, creation resolves every entry inside the project, so an entry naming no task of it — whatever the reason — returns `invalid_argument` `dependency must reference tasks of the same project`, as `POST /projects/{pid}/tasks` does. `created_by_session_id` is set. Resolve provenance after taking the project lock: with no held tasks, omission creates no provenance edge; with exactly one, omission selects that task; with multiple, `discovered_from` is required or the entire request fails with `invalid_argument`. An explicit origin must be a task currently held by the caller in this project. If the selected origin is the new task's parent, the parent link already records provenance and no redundant `discovered_from` edge is added; otherwise add that edge from the new task to the origin. A `blocks` edge to the same origin may coexist. The same one-level parent validation as REST applies; any validation failure creates neither a task nor edges.

### `list_session_branches` (git; profile-gated)

Description: "List session branches in the project mirror with how far ahead/behind they are relative to the default branch."

Input `{}`. Output `{ branches: SessionBranch[] }` (same shape as REST).

### `merge` (git; profile-gated)

Description: "Merge into an integration branch. For reviewed task work, pass task_id and handoff_id to merge the exact approved commit. For an explicit generic branch merge, pass source. Fails with conflicting paths; never resolves conflicts for you."

Input `MergeInput` (same as REST; `task_id` also accepts a per-project task number). Output `{ commit: string }` or error `conflict`, with `data: { conflicts: string[] }` for merge conflicts. The task form rejects stale or unapproved hand-offs as `conflict` with the same `handoff_id is not the task's current hand-off` and `hand-off is not approved` messages the REST endpoint gives, and uses the pinned commit without syncing the source branch. For the branch form, ref names follow the REST git contract: `origin/main` is a permitted source, while the target must be a Mars integration head; every session ref involved is synced first.

### `rebase` (git; profile-gated)

Description: "Rebase a branch onto another in the mirror. Use it to bring a session branch up to date with the default branch before merging."

Input `{ branch: string, onto: string }`. Output `{ commit: string }` or `conflict`. Ref names follow the REST git contract: an upstream-tracking ref may be `onto`, never the branch being rewritten. If `branch` is the calling session's branch, the session work tree is updated afterwards when clean.

### `push` (git; profile-gated)

Description: "Push a mirror branch to the upstream remote. Only do this when a human or the task explicitly asks for it."

Input `{ ref: string, remote_branch?: string, force?: boolean = false }`. Output `{ remote_branch: string, commit: string }`. Only integration heads or session refs may be pushed, and only the selected upstream branch is updated. A non-fast-forward rejection returns `conflict` without changing local refs. Force pushes are refused unless `force` is true and the profile has `push` in `mcp_tools`; session refs are pushed as `refs/heads/session/<id>` by default.

Error codes used across tools: `unauthorized` (bad token), `forbidden` (tool not in profile), `not_found`, `conflict`, `invalid_argument`, `internal`. A tool failure is a JSON-RPC error object, never a result with `is_error`; its `message` is the text quoted in the tool sections above, and its `data.code` is the string code:

| `data.code` | JSON-RPC `code` |
| --- | --- |
| `unauthorized` | -32001 |
| `forbidden` | -32003 |
| `not_found` | -32004 |
| `conflict` | -32009 |
| `invalid_argument` | -32602 |
| `internal` | -32603 |

`data.code` is always present; `data.conflicts` (a list of paths, in git's order) is present only on a `merge` or `rebase` that stopped on conflicting paths. Every `internal` failure is logged with its detail and answered with the message `internal error` and no other `data` keys, so no internal detail, credential or git stderr reaches the agent.

## Frontend

Vite, React 19, TypeScript strict, Tailwind CSS 4, React Router 7, TanStack Query, Zustand for per-session and per-project reducers, `@tanstack/react-virtual` for the transcript, `react-markdown` for text, a diff renderer for edit tools, `xterm.js` for the terminal view, Playwright for end-to-end tests.

Structure:

```
frontend/src/
├── components/     reusable UI: FormField, SubmitButton, Alert, LoadingState, EmptyState, PageLayout, AuthLayout, ProtectedRoute, AdminRoute, ...
├── pages/          route-level: LoginPage, AcceptInvitePage, ChangePasswordPage, ForgotPasswordPage, ResetPasswordPage, DashboardPage, ProjectsPage, ProjectPage, SessionPage, SecretsPage, SettingsPage, ProfileEditorPage, AdminPage (users + invites)
├── session/        SessionView, Transcript, Composer, TerminalView, tool renderers, sessionStore (Zustand), useSessionSocket
├── tasks/          TaskBoard, TaskCard, TaskDetail, TaskStatesEditor, taskStore (Zustand), useTaskStream
├── services/       apiClient (fetch wrapper with refresh-on-401), auth, projects, profiles, sessions, tasks, taskStates, secrets, users, git
├── hooks/          useAuth, useFormSubmit
├── types/          TypeScript mirrors of every API shape in this document
└── utils/
```

Rules: components never call `fetch`; every request goes through `services/`. Auth state lives in `services/auth` with an in-memory access token mirrored to `localStorage`, and a `useAuth()` hook that subscribes to it. `ProtectedRoute` redirects a user whose current `must_change_password` is set to the change-password page. Token role/flag snapshots are UI hints only; the backend always checks the current user. Load `GET /users/me` at authenticated startup and refresh the current user after an authorization 403 so a demotion updates admin navigation. A failed refresh with 401 clears the access token from memory and `localStorage`, clears authenticated query and stream stores, closes streams and returns to login. Self-service password changes replace the token pair and reconnect streams without replaying pending inputs.

Routes: `/login`, `/invite/:token`, `/change-password`, `/forgot-password`, `/reset-password/:token`, `/` (DashboardPage), `/projects`, `/projects/:id` (ProjectPage, with tabs for sessions, board, profiles, shared directories, states and secrets), `/projects/:id/tasks/:number` (the board with that task's drawer open), `/sessions/:id`, `/secrets`, `/settings` (own password and `notify_email`), `/admin`. The project page's tab is the `?tab=` search parameter — `sessions` (the default), `board`, `profiles`, `shared-dirs`, `states`, `secrets`, with an unknown value falling back to `sessions` — and `/projects/:id/tasks/:number` is the board tab with that task's drawer open.

**Code splitting.** The entry bundle carries only what a first paint needs: the route table, the auth pages and the dashboard. `/projects`, `/projects/:id`, `/sessions/:id`, `/secrets`, `/settings` and `/admin` are `React.lazy` imports of their own page file — never of the `pages/` barrel, which would defeat the split — behind one `Suspense` in `App.tsx` whose fallback is `LoadingState` inside `PageLayout`. Within the session view the `Terminal` panel is lazy too, so `xterm.js` and its stylesheet are fetched only when that tab is first opened; `SidePanel` renders the active panel inside a `Suspense`. `npm run build` therefore emits one chunk per lazily loaded route and no chunk-size warning.

**Agent credentials.** `SecretsPage` opens with an `Agent credentials` section above the general list. It lists the secrets whose `credential_for` is set, each under its label rather than its name (`Claude subscription token` for `CLAUDE_CODE_OAUTH_TOKEN`, `Anthropic API key` for `ANTHROPIC_API_KEY`) with the scope it applies to and its last use, and the general list leaves them out. The scopes it reads are the ones a launch resolves over — the caller's own user scope (`You`), every project (its name) and `global` (`Everyone`) — not the one scope the picker below has selected, so "is there a credential at all" is answered wherever the page is pointed; an administrator who has selected another user's scope sees that scope too, under that user's username. Each row offers the same value replacement and deletion as an ordinary secret, and no rename: the name is the credential. With no credential at any of those scopes the section shows, in the warning colour, that sessions cannot authenticate without one. `Add agent credential` is a form of three fields: the kind (the subscription token carries the hint that `claude setup-token` prints it and needs a Pro or Max subscription), the value (a password-type field, cleared once it is stored), and `Applies to` — `Me` (the default), one project chosen from a select, or `Everyone` — which become `scope` and `scope_id` of an ordinary `POST /secrets`; the name is never typed and there is no orchestrator-only control. `A project` is disabled, with the reason, when there are no projects. A 409 is shown with the body's `error`, which names the credential already there. The labels, hints and names are a constant table per backend in `src/secrets/`, the one place the frontend knows them.

`AgentCredentialNotice` renders one entry of `GET /projects/{pid}/agent-credentials` (TanStack Query key `["projects", pid, "agent-credentials"]`, invalidated by every secrets mutation) for the backend of the profile at hand: `Authenticates with your Claude subscription token`, `… the project's Anthropic API key`, `… the shared …`, or, in the warning colour, `No agent credential: sessions of this profile will fail to authenticate` with a link to `/secrets`. `ProfileEditorPage` shows it read-only beside the secrets field, whose picker leaves agent credential names out; the launch form shows it for the selected profile, and when there is none the primary action becomes `Add credential` while `Launch anyway` stays available, since the stub image and images with their own authentication need none.

**Copy links.** Add a `Copy link` action to the task-detail drawer header and session header. Copy an absolute URL using the current frontend origin and the existing canonical route: `/projects/{project_id}/tasks/{number}` for a task and `/sessions/{id}` for a session. Build the link from the displayed resource, even when the task drawer was opened from the board; omit search/filter parameters, fragments and authentication tokens. The link continues to identify the same resource after title or state changes.

Show `Link copied` only after the clipboard write succeeds. If clipboard access is unavailable or denied, display the same URL in a selectable field for manual copying. Opening the link uses normal authentication; preserve the internal destination through login and any required first-login password change, then open that task drawer or session. Only accept same-origin application paths as return destinations. Deleted or inaccessible resources use the normal not-found/authorization handling. Copying a link creates no shared-access token, permission change, tracker event or network mutation. This is a frontend action using existing routes, with no new endpoint or schema.

**Dashboard.** `DashboardPage` loads `GET /sessions?state=running`, `GET /sessions?state=parked` and `GET /tasks?state_kind=human` through TanStack Query with a 30-second refetch interval, and links each row to its session or task.

**Session state.** `useSessionSocket(sessionId)` opens the WebSocket with `after = store.lastSeq`, reconnects itself with a fresh access token on close (see "Authentication"), fetches older history through REST when the user scrolls up, and dispatches every event to the session store. The store never keeps the event list; it folds events into:

```ts
interface SessionState {
  session: Session | null;
  status: "connecting" | "live" | "reconnecting";
  lastSeq: number;
  order: string[];                         // message ids in display order
  messages: Record<string, Message>;       // id -> folded message
  pendingTools: Record<string, string>;    // tool_use_id -> message id awaiting a result
  subagents: Record<string, string[]>;     // parent_tool_use_id -> message ids nested under it
  oldestSeq: number | null;                // lowest seq held: the `?before=` cursor for older history
  hasMore: boolean;                        // older history remains behind oldestSeq
  gitEventSeq: number;                     // seq of the last `git` event; the Changes panel refreshes on it
  turnActive: boolean;                     // a turn is in progress: the composer's button reads "Interject"
  lastRejection: { client_id: string; reason: string } | null;  // latest `input_rejected`, cleared by the next send
}

type Message =
  | { id; kind: "user"; text; pending?: boolean; rejected?: string }
  | { id; kind: "assistant_text"; text; streaming: boolean }
  | { id; kind: "thinking"; text; redacted: boolean }
  | { id; kind: "tool"; tool_use_id; name; input; result?; is_error?; truncated?; running: boolean;
      children?: string[]; subagent?: { description; agent_type?; is_error? } }
  | { id; kind: "system"; text; level: "info" | "warn" | "error"; detail?: unknown }
  | { id; kind: "result"; subtype; is_error; num_turns; duration_ms; cost_usd?; usage? }
  | { id; kind: "raw"; native: unknown };
```

`text_delta` appends to the current streaming assistant message; the following `text` replaces it and clears `streaming`. `tool_call` creates a tool message and registers it in `pendingTools`; `tool_result` completes it. Events carrying `parent_tool_use_id` are placed under the tool message with that id instead of at the top level. `user_message` with a `client_id` matching an optimistic message replaces it, keeping its place in `order`. Event-derived messages have the id `e<seq>` and an optimistic one `client:<client_id>`. A `tool_result` whose `tool_call` has not been seen becomes a tool message named `unknown`, and a subagent event whose parent has not been seen hangs under a placeholder tool message named `Agent` with the id `tool:<tool_use_id>`, so nothing is dropped; when an older history page later supplies the real `tool_call`, the two halves are reconciled into one message at the older position, so one `tool_use_id` is always one message. Older pages are folded on their own and prepended; `lastSeq` only ever moves forward and an event at or below it is ignored, which is what makes reconnect replay idempotent. A message the client itself has to show, such as a socket `error` frame, is appended as a `system` message with a `local:<n>` id and touches no cursor.

**Transcript rendering.** One renderer per tool family, chosen by tool name: markdown for assistant text; a side-by-side or unified diff for edit and write tools (computed from `old_string`/`new_string` or file content); monospace with ANSI stripping for shell tools; a one-line summary over the input and result for read, glob and grep tools; a nested, collapsible transcript for subagents; a JSON tree for anything else and for `raw`. Every tool row starts collapsed to its header — the tool's name, one line saying what it was asked (the shell command's description or first line, the file path, the search pattern) and its status — and a subagent's nested transcript starts collapsed whether the subagent is running or has ended, the header's status saying which; one click opens a row's full body, result included. A failed call keeps its failed colouring while collapsed. Long tool results are collapsed above 40 lines. The list is virtualised and follows the tail: it stays on the newest row as events arrive and as the virtualizer corrects its own height estimates, and stops following only when the reader moves the viewport upwards, which reveals a "Jump to latest" control carrying the number of top-level messages appended since, and re-follows when the reader returns to within 48 px of the bottom or presses that control.

**Changes panel.** The diff endpoint's internal fetch-back emits no separate `git` event, preventing a refresh loop; explicit sync actions still emit their outcome events. A tab beside the transcript fetches `GET /projects/{pid}/git/diff?head=<session id>` when opened and again on every `git` event, lists the files with their counts, and renders the patch with the same diff renderer as edit tools. After a successful push to a `github.com` remote the UI shows a link to `https://github.com/<owner>/<repo>/compare/<target>...<remote_branch>?expand=1`, built client-side from `remote_url`, next to the push result and on the session-branch list. The session header shows `cost_usd` and the token counters.

**Task board.** `useTaskStream(projectId)` installs SSE handlers, opens the stream with `?after=store.lastSeq`, and waits for `open` before loading the project's states and tasks through REST. On error it closes the stream and reconnects with a fresh token and the last received sequence, then refreshes again. Each new `TaskEvent` updates the receive cursor and invalidates the board snapshot; its payload is not applied directly to cards or columns. Columns are the project's states in `position` order. State renames, deletions and ordinary mutations all use the same refresh path. Cards show priority, labels, the holding session (with a link), attempts when above one, assignee, blocked and dependency indicators, and a parent badge. A detail drawer shows description, comments (system comments styled apart), dependencies by kind, children, the sessions that touched the task with links into their transcripts, and actions: move to a state, release, "open in session", which picks a conversational profile (default: the first that serves the task's state) and launches it with `task_id`, and "run once", which does the same with an ephemeral profile. Both launch controls are disabled, carrying the reason, for a task that is held, blocked or in a terminal state and for a project that is not `ready` — the four refusals `POST /projects/{pid}/sessions` answers 409 for — so the user reads the reason before the request rather than after it. The session view shows the task the session was launched for and the tasks it touched in a side panel. `TaskStatesEditor` on the project page lists, adds, renames, reorders and removes states, with the deletion rules from the API surfaced as disabled actions.

**Task-board search.** Provide a labelled search field above the board with placeholder `Search title or #number` and a clear action. Trim surrounding whitespace: an empty query shows all tasks; a query consisting only of digits, optionally prefixed with `#`, matches the exact per-project task number; any other query matches a case-insensitive substring of the title. For example, `#42` and `42` match task 42, not 142; `login` matches `Fix Login Redirect`. Search spans all project state columns, including terminal states, and does not search descriptions or comments.

Apply search locally to the complete project task snapshot already loaded by the board. Keep the original snapshot intact; derive the visible cards while retaining column order and normal task ordering. Do not issue REST requests or reconnect SSE when the query changes. Keep the query through snapshot refreshes and reapply it to updated titles, new tasks and deletions. Reset it when changing projects. Preserve columns when there are no matches and show `No matching tasks` with the clear action; distinguish this from initial loading, a failed load or an empty project. Search does not close an already-open task drawer or prevent opening a task through its direct URL. No new API parameter, database index or search service is required (ADR 0031).

**Board refresh ordering.** For each mounted project view, allow at most one refresh at a time. Each refresh makes fresh REST reads of both task states and tasks through the services layer; replace both in the Zustand store together. At the start, capture the connection/view generation and the event generation. Every non-duplicate event increments the event generation. If either generation changed before both reads completed, discard those responses and perform one coalesced follow-up refresh. A successful local mutation also invalidates the view, even if its SSE event has not arrived yet. A reconnect, project change or unmount makes old responses ineligible to update the current view. Read failures retain the previous snapshot and the dirty flag, show the ordinary error/retry state and retry through the query layer; they never install an empty board. Keep the previous snapshot visibly refreshing/reconnecting while it is stale, or a loading state before the first successful load. Delayed events cause another authoritative refresh rather than applying obsolete payloads. Open task details and related queries are invalidated on task events as well. This is eventual consistency through REST refreshes, with no new snapshot-cursor protocol or event buffer in v1.

**Hand-off controls.** Task details show the current source session/branch, pinned commit, comment and review status, plus hand-off history. A code hand-off form selects a source session and exact commit, requires a comment, and submits it with the target state. Review actions forward the current hand-off id with `approved` or `changes_requested` and a comment. Neither action silently selects the reviewer's own branch. "Open in session" and "run once" default to the hand-off commit and visibly disclose any base override. The task's merge action sends `task_id` and `handoff_id` and is enabled only for an approved current hand-off. Approval is labelled with the commit it covers; new revisions appear unreviewed. The diff endpoint accepts `handoff_id` instead of `head` to view the retained commit without syncing a live branch.

**Composer.** A text area with submit on Enter (Shift+Enter for newline), disabled when the session is `done`/`failed` and absent for ephemeral sessions. While a turn is in progress the button reads "Interject". A stop button sends `stop`. There is no answer mode: the agent never asks the host a question (ADR 0033), so the composer only ever sends a `message`, and one sent during a turn is queued by the CLI as the next turn rather than interrupting the current one (`ARCHITECTURE.md`, "Input encoding").

## Non-goals for v1

- Automatic launching of agents: a dispatcher that starts an ephemeral session when a served state has claimable work, and scheduled agents that run a profile on a cron expression (a daily tech-debt scan that files tasks, an agent that turns GitHub issues into backlog tasks). The task model is built for both and needs no change for them (`ARCHITECTURE.md`, "Task tracker", "After v1"). In v1 work reaches an agent in exactly one way: a user launches a session, conversational or ephemeral, optionally for a task, and that agent either holds the task it was launched for or calls `ready` and `claim`.
- GitHub App credentials, GitHub login, webhooks.
- Egress restriction for session containers and sandboxed runtimes as defaults. `runtime` is configurable per profile; installing and choosing one is the operator's job.
- Isolation of orchestrator git operations on agent-controlled checkouts. v1 explicitly accepts the known vulnerability that repository-controlled git configuration can execute commands with orchestrator privileges; remediation is deferred (ADR 0019; `ARCHITECTURE.md`, "Known v1 vulnerability: git execution outside the session container").
- Durable input queues, delivery-status UI and restart-safe input deduplication. Input loss and uncertain or repeated processing around orchestrator restarts are accepted v1 limitations; normal output persistence and replay remain required (ADR 0020).
- Per-project authorisation. Every user sees every project.
- Multi-node orchestration or more than one orchestrator instance.
- A second agent backend. The `AgentBackend` trait exists so one can be added; GitHub Copilot CLI is the candidate and lives on the roadmap in `README.md`.
- Self-registration. Users exist only through invites.
- Secret injection through files instead of environment variables (documented hardening step).
- Automatic secret detection or redaction in transcripts and agent-produced event content. Such content may retain plaintext credentials; the secret-handling rules govern orchestrator diagnostics and credential storage, not a guarantee about arbitrary agent output (ADR 0027).
- Transcript export, search across sessions, cost reporting beyond the per-session counters.
- Per-profile CPU and memory limits.
- An MCP tool for the agent to set its own session title.
- Pagination of task and session lists.
- Mobile layouts beyond "does not break".
