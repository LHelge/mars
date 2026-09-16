---
id: zazd5
title: Add task, task-state, hand-off and TaskEvent types and the tasks and taskStates services
status: open
priority: P0
created: "2026-09-16T20:40:01.621395700Z"
updated: "2026-09-16T20:40:01.621395700Z"
tags:
  - frontend
  - tracker
parent: gn4y2
---

## Summary
Give the board, drawer and states editor a single typed contract for everything under `/api/projects/{pid}/tasks` and `/api/projects/{pid}/task-states`: TypeScript mirrors of `Task`, `TaskDetail`, `Comment`, `Handoff`, `HandoffInput`, `TaskState` and `TaskEvent`, plus `services/tasks.ts` and `services/taskStates.ts` wrapping every endpoint through `apiClient`. Also define the TanStack query keys the rest of the epic invalidates. Nothing here renders; every later task in the epic imports from these modules.

## Documents
- `SPEC.md` "Tasks" (endpoint table, `Task`, `TaskDetail`, `Comment` shapes, `{id}` accepting UUID or number)
- `SPEC.md` "Task states" (endpoint table, `TaskState` shape, name rule)
- `SPEC.md` "Code hand-offs and review" (`HandoffInput`, `Handoff` shapes)
- `SPEC.md` "TaskEvent"
- `SPEC.md` "SSE: task stream" (URL and `?token=` / `?after=`)
- `SPEC.md` "Frontend" (services list: `tasks`, `taskStates`; rule that components never call `fetch`; `types/` mirror every API shape)
- `CLAUDE.md` "Frontend conventions" (types in `snake_case` exactly as the API sends them)

## Acceptance criteria
- [ ] `frontend/src/types/tasks.ts` exports, with `snake_case` fields exactly as `SPEC.md` lists them: `TaskStateKind = "queue" | "human" | "terminal"`, `TaskState = { id, project_id, name, kind, position, created_at }`, `TaskDependencyKind = "blocks" | "discovered_from" | "related"`, `Task = { id, project_id, number, title, description, state, priority, blocked, labels, parent_id, assignee_user_id, lease_holder_session_id, lease_since, attempts, needs_human_reason, handoff: Handoff | null, depends_on: {task_id, kind}[], blocks: string[], created_at, updated_at, closed_at }`, `Comment = { id, task_id, author_user_id, author_session_id, system, body, created_at }`, `ReviewStatus = "unreviewed" | "approved" | "changes_requested"`, `Handoff = { id, task_id, source_session_id, source_branch, commit, comment_id, review_status, reviewed_by_user_id, reviewed_by_session_id, reviewed_at, created_by_user_id, created_by_session_id, created_at }`, `HandoffInput` as the two-variant union from `SPEC.md` (`revision` with `source_session_id?`, `commit`, `comment`; `forward` with `handoff_id`, `comment`, `review?`), `TaskSessionLink = { session_id, first_touched_at, last_touched_at }`, `TaskDetail = Task & { comments: Comment[], handoffs: Handoff[], children: Task[], sessions: TaskSessionLink[] }`, `TaskEventKind` (the 13 kinds), `TaskActor` (the three-variant union), `TaskEvent = { seq, ts, task_id, actor, kind, task?, comment?, from?, to?, reason?, states? }`.
- [ ] Input types: `CreateTaskInput = { title, description?, state?, priority?, labels?, parent_id?, depends_on?: string[] }`, `UpdateTaskInput = { title?, description?, state?, priority?, labels?, parent_id?: string | null, assignee_user_id?: string | null, handoff?: HandoffInput }`, `CreateTaskStateInput = { name, kind, position? }`, `UpdateTaskStateInput = { name?, position? }`.
- [ ] `frontend/src/services/tasks.ts` exports `listTasks(projectId, filters?: { state?, label?, priority?, parent?, held? })` → `Task[]` (`GET /projects/{pid}/tasks`), `createTask(projectId, input)` → `Task` (POST, 201), `getTask(projectId, ref: string | number)` → `TaskDetail` (`GET .../tasks/{ref}`), `updateTask(projectId, ref, input)` → `Task` (PUT), `deleteTask(projectId, ref)` → `void` (DELETE, 204), `addDependency(projectId, ref, { depends_on, kind? })` → `Task` (POST), `removeDependency(projectId, ref, dep, kind)` → `Task` (`DELETE .../dependencies/{dep}?kind=`), `addComment(projectId, ref, body)` → `Comment` (POST), `releaseTask(projectId, ref)` → `Task` (POST `.../release`), `taskStreamUrl(projectId, token, after)` → the string `/api/projects/{pid}/tasks/stream?token=<encoded>&after=<n>`.
- [ ] `frontend/src/services/taskStates.ts` exports `listTaskStates(projectId)` → `TaskState[]`, `createTaskState(projectId, input)` → `TaskState`, `updateTaskState(projectId, name, input)` → `TaskState` (`PUT .../task-states/{name}`), `deleteTaskState(projectId, name)` → `void`.
- [ ] `frontend/src/tasks/queryKeys.ts` exports `taskKeys = { all: (pid) => ["projects", pid, "tasks"], detail: (pid, number) => ["projects", pid, "tasks", number] }` and `taskStateKeys = { list: (pid) => ["projects", pid, "task-states"] }`; every later task uses these, never inline arrays.
- [ ] `frontend/src/tasks/index.ts` and `frontend/src/types/index.ts` barrels re-export the new modules.
- [ ] `cd frontend && npm run lint && npx tsc -b && npm run build` pass.

## Implementation notes
- Use `apiGet`/`apiPost`/`apiPut`/`apiDelete` from `frontend/src/services/apiClient.ts`; never `fetch`.
- Build query strings with `URLSearchParams`; omit undefined filters.
- `ref` in task paths is the per-project `number` in every UI call (the URL route uses the number); the UUID is accepted too, so the signature takes `string | number`.
- `taskStreamUrl` is a pure function so the SSE hook and its unit test share it; encode the token with `encodeURIComponent`.
- If the Frontend foundation epic already created any of these types (it owns `types/` for shapes the dashboard needs, e.g. `Task` for `GET /tasks?state_kind=human`), extend the existing file rather than adding a second definition; there must be exactly one `Task` type in `types/`.
- `types/tasks.ts` uses `import type` everywhere (`verbatimModuleSyntax`).

## Edge cases
- `TaskEvent.task_id` is `string | null` (null on `states_changed`); `task`, `comment`, `states` are optional, never nullable.
- `Task.handoff` is `Handoff | null`, never undefined; `Handoff.source_session_id` and the actor ids are `string | null` because deletion nulls them.
- `priority` is a number 0–3; do not model it as a string enum.
- `removeDependency` must always send `?kind=`: the API removes only that kind.

## Testing
- No runtime logic beyond URL building; add a Vitest test only if the store task (which introduces Vitest) has already landed, covering `taskStreamUrl` encoding (`after=0`, token with `+`/`/` characters). Otherwise the acceptance test is the command chain.
- `cd frontend && npm run lint && npx tsc -b && npm run build`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- Frontend foundation epic: `services/apiClient.ts` with `apiGet`/`apiPost`/`apiPut`/`apiPatch`/`apiDelete` (bearer attach, refresh once on 401); `types/` barrel layout.
- Repository scaffolding epic: the `frontend/` Vite project with the directory structure and barrels.