---
id: c3hjz
title: Add TypeScript type mirrors for the auth, users, secrets, sessions, tasks and projects API shapes
status: open
priority: P1
created: "2026-09-16T20:39:01.349288704Z"
updated: "2026-09-16T20:39:01.349288704Z"
tags:
  - frontend
parent: "2f5u2"
---

## Summary
Create the `frontend/src/types/` modules that mirror, field for field and in `snake_case`, every API shape this epic's pages consume: the error envelope, `User`, `Invite`, the `{user, access_token}` auth response, `SecretMeta` and secret uses, `Project`, `Session`, `Task`, `Comment` and `Handoff`. Later frontend epics add `AgentEvent`, `TaskEvent`, `TaskDetail`, `TaskState`, `Profile`, `SharedDir` and `Branch` beside these; nothing here is guessed, every field comes from a `SPEC.md` shape line.

## Documents
- `SPEC.md` "REST API" intro: error body `{ "status": <u16>, "error": "<message>" }`, git conflicts add `conflicts: string[]`; timestamps RFC 3339 strings, ids UUID strings.
- `SPEC.md` "Authentication": login/accept-invite return `{ user, access_token }`.
- `SPEC.md` "Users (`/api/users`)": `User = { id, username, email, admin, must_change_password, notify_email, created_at }`; `Invite = { id, email, admin, invited_by, expires_at, created_at }`.
- `SPEC.md` "Secrets (`/api/secrets`)": `SecretMeta = { id, scope, scope_id, name, orchestrator_only, key_version, created_by, created_at, updated_at, last_used_at }`; uses rows `{session_id, user_id, purpose, at}`.
- `SPEC.md` "Projects (`/api/projects`)": `Project = { id, name, remote_url, default_branch, status, status_message, last_fetched_at, max_attempts, created_at, has_credential }`.
- `SPEC.md` "Sessions": `Session = { id, project_id, profile_id, kind, created_by, title, task_id, handoff_id, state, base_ref, branch, container_id, cli_session_id, last_seq, last_activity_at, cost_usd, input_tokens, output_tokens, error, created_at, parked_at, ended_at }`.
- `SPEC.md` "Tasks": `Task = { id, project_id, number, title, description, state, priority, blocked, labels, parent_id, assignee_user_id, lease_holder_session_id, lease_since, attempts, needs_human_reason, handoff: Handoff | null, depends_on: {task_id, kind}[], blocks: id[], created_at, updated_at, closed_at }`; `Comment = { id, task_id, author_user_id, author_session_id, system, body, created_at }`.
- `SPEC.md` "Code hand-offs and review": `Handoff = { id, task_id, source_session_id, source_branch, commit, comment_id, review_status, reviewed_by_user_id, reviewed_by_session_id, reviewed_at, created_by_user_id, created_by_session_id, created_at }`.
- `docs/data-model.md` "Enums": `project_status` (`cloning`, `ready`, `error`), `profile_kind` (`conversational`, `ephemeral`), `session_state` (`creating`, `running`, `parked`, `done`, `failed`), `task_dependency_kind` (`blocks`, `discovered_from`, `related`), `secret_scope` (`global`, `user`, `project`).
- `CLAUDE.md` "Frontend conventions": types in `src/types/` mirror `SPEC.md` exactly, `snake_case`.

## Acceptance criteria
- [ ] `frontend/src/types/api.ts` exports `ApiErrorBody = { status: number; error: string; conflicts?: string[] }`.
- [ ] `frontend/src/types/users.ts` exports `User`, `Invite`, `AuthResponse = { user: User; access_token: string }`, and the request bodies `LoginRequest`, `AcceptInviteRequest = { token, username, password }`, `InviteLookup = { email, admin, expires_at }`, `PasswordChangeRequest = { current_password?: string; password: string }`, `UpdateMeRequest = { notify_email?: boolean }`, `UpdateUserRequest = { username: string; admin: boolean }`, `CreateInviteRequest = { email: string; admin?: boolean }`.
- [ ] `frontend/src/types/secrets.ts` exports `SecretScope = "global" | "user" | "project"`, `SecretMeta`, `SecretUse = { session_id: string | null; user_id: string | null; purpose: "launch" | "git"; at: string }`, `CreateSecretRequest = { scope, scope_id?, name, value, orchestrator_only? }`, `ReplaceSecretRequest = { value }`, `PatchSecretRequest = { name?, orchestrator_only? }`.
- [ ] `frontend/src/types/projects.ts` exports `ProjectStatus` and `Project`; `frontend/src/types/sessions.ts` exports `SessionState`, `SessionKind` (`profile_kind` values) and `Session`; `frontend/src/types/tasks.ts` exports `TaskDependencyKind`, `ReviewStatus`, `Handoff`, `Comment`, `Task` (with `priority: 0 | 1 | 2 | 3` and `labels: string[]`).
- [ ] Nullable fields (`title`, `task_id`, `handoff_id`, `branch`, `container_id`, `cli_session_id`, `error`, `parked_at`, `ended_at`, `default_branch`, `status_message`, `last_fetched_at`, `scope_id`, `created_by`, `last_used_at`, `invited_by`, `description`, `parent_id`, `assignee_user_id`, `lease_holder_session_id`, `lease_since`, `needs_human_reason`, `closed_at`, `reviewed_*`, `created_by_*`, `source_session_id`, `comment_id`) are typed `T | null`, never optional, because the API always sends the key.
- [ ] `frontend/src/types/index.ts` re-exports every module with `export type`; every import elsewhere uses `import type` (required by `verbatimModuleSyntax`).
- [ ] `cd frontend && npm run lint && npx tsc -b && npm run build` passes.

## Implementation notes
- Files: `frontend/src/types/{api,users,secrets,projects,sessions,tasks,index}.ts`. Use `interface` for object shapes and string-literal unions for enums; no runtime code, no zod.
- Keep the exact key order of the `SPEC.md` shape lines so a reviewer can diff them by eye.
- Timestamps are `string` (RFC 3339); `cost_usd` is `number`; `input_tokens`/`output_tokens`/`last_seq`/`attempts`/`number`/`key_version`/`max_attempts` are `number`.
- Add a two-line header comment in each file naming the `SPEC.md` section it mirrors.

## Edge cases
- `Task.handoff` is `Handoff | null`, `Task.depends_on` is `{ task_id: string; kind: TaskDependencyKind }[]`, `Task.blocks` is `string[]`.
- `Session.kind` is the profile kind at launch (`SessionKind`), not a separate enum.
- Do not add `TaskDetail`, `AgentEvent`, `TaskEvent`, `Profile`, `SharedDir`, `TaskState` or `Branch` here; the project/session and board epics own them and would conflict.

## Testing
- Type-only change: `cd frontend && npm run lint && npx tsc -b && npm run build` must pass. No unit tests.

## Documentation
- none: implements the documented shapes as written. If a shape line in `SPEC.md` is found inconsistent with another section, file a new task against the owning backend epic rather than "fixing" the type.

## Assumes from other epics
- "Repository scaffolding, tooling and CI": `frontend/` scaffold with `src/types/index.ts` barrel and `verbatimModuleSyntax` enabled (task ncv5g).