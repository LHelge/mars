---
id: k4esd
title: "Build the TaskDetail drawer: route, detail query, description, comments, dependencies, children, sessions and Copy link"
status: done
priority: P1
created: "2026-09-16T20:43:12.018839044Z"
updated: "2026-09-20T10:51:24.130486834Z"
tags:
  - frontend
  - tracker
depends_on:
  - "347vr"
parent: gn4y2
attempts: 1
---

## Summary
Deliver the read side of the task drawer at `/projects/:id/tasks/:number`: `TaskDetail` opens over the board, loads `TaskDetail` through TanStack Query keyed by `taskKeys.detail`, and shows the header fields, markdown description, comments with system comments styled apart and a comment form, dependencies grouped by kind, children, the sessions that touched the task with transcript links, and the `Copy link` action with clipboard fallback. Editing and tracker actions are added by the next task; this one establishes the drawer shell, sections and invalidation wiring.

## Documents
- `SPEC.md` "Frontend", "Task board" (detail drawer contents), routes (`/projects/:id/tasks/:number` is the board with that task's drawer open), "Copy links" (both paragraphs), "Board refresh ordering" (open task details are invalidated on task events)
- `SPEC.md` "Tasks" (`GET /projects/{pid}/tasks/{id}` → `TaskDetail`; `POST .../comments {body}` → `Comment`; `Comment.system`; `depends_on` with kinds; `blocks`; `children`; `sessions`)
- `docs/data-model.md` `task_comments` (system comments have no author), `task_sessions`
- `CLAUDE.md` "Frontend conventions" (`react-markdown`, shared components, `/frontend-design` skill)

## Acceptance criteria
- [ ] `TasksPage` renders `<TaskDetail projectId number />` when `:number` is present; closing the drawer (`Close` button, Escape, backdrop click) navigates to the board route of the same project without clearing the search query.
- [ ] `frontend/src/tasks/TaskDetail.tsx` loads with `useQuery({ queryKey: taskKeys.detail(pid, number), queryFn: () => getTask(pid, number) })`; shows `LoadingState`, a not-found state (`Task not found`) on 404, and `Alert` with retry on other errors. Because `useTaskStream` invalidates `taskKeys.all(pid)` on every task event, the drawer refetches automatically; no direct event handling in the drawer.
- [ ] Header: `#<number>`, title, state name, priority `P<n>`, `blocked` marker, holder link (`/sessions/{id}`), `attempts` when `> 1`, assignee (via `["users", id]` query), labels, `needs_human_reason` when set, created/updated/closed timestamps (RFC 3339 rendered relative with the absolute in `title`).
- [ ] Description rendered with `react-markdown` (empty → quiet `No description`).
- [ ] Comments oldest first: author shown as username (user), a session link (`author_session_id`), or `system` with a visually distinct style (muted, italic, `system` tag) when `system === true`; body rendered as markdown. Comment form: textarea, submit `addComment(pid, number, body)` (201 → `Comment`), then invalidate `taskKeys.detail` and call `useTaskStore.getState().invalidate()`; empty body disabled; errors via `useFormSubmit`.
- [ ] Dependencies section grouped by kind with headings `Blocks on`, `Discovered from`, `Related`, each entry a link to `/projects/{pid}/tasks/{number}` with title resolved from the board snapshot (`selectTaskById`) or `#?` when absent; a `Blocked by this task` list from `task.blocks` the same way. (Add/remove controls come from the actions task; this task renders the lists only.)
- [ ] Children: each child card-like row (number, title, state, blocked) linking to its drawer route; parent shown as `part of #<n>` link.
- [ ] Sessions: rows of `session_id` (short id, link to `/sessions/{id}`), `first_touched_at`, `last_touched_at`, the current holder highlighted.
- [ ] `Copy link` in the drawer header copies `${window.location.origin}/projects/${task.project_id}/tasks/${task.number}` (built from the loaded task, without query, fragment or token) via `navigator.clipboard.writeText`; on success shows `Link copied` for ~2 s; on rejection or when `navigator.clipboard` is undefined shows a read-only, auto-selected `<input>` with the URL for manual copying. No request is made.
- [ ] `cd frontend && npm run lint && npx tsc -b && npm run build` pass.

## Implementation notes
- Files: `frontend/src/tasks/TaskDetail.tsx`, `frontend/src/tasks/CommentList.tsx`, `frontend/src/tasks/CommentForm.tsx`, `frontend/src/tasks/DependencyList.tsx`, `frontend/src/components/CopyLinkButton.tsx` (shared with the session header in the session epic; if that epic already added one, reuse it), edit `frontend/src/pages/TasksPage.tsx`, `frontend/src/tasks/index.ts`.
- Use the `/frontend-design` skill before shaping: a right-side drawer over the board, dense metadata grid, monospace for ids/numbers/commits, comments as a timeline.
- `CopyLinkButton({ path })` takes the canonical app path and prepends `window.location.origin`; export it from `components/` so the session epic can use it.
- Relative time helper in `frontend/src/utils/time.ts` if the foundation has none.

## Edge cases
- `:number` that is not an integer → not-found state without a request.
- The drawer must open from a direct URL before the board snapshot is loaded; dependency titles fall back to `#<n>` lookups from the detail's own `children` where possible and to `#?` otherwise, then fill in on the next render.
- A `deleted` event for the open task: the refetch returns 404 → not-found state with a `Back to board` action.
- Clipboard write can reject on non-secure origins; the fallback field is the documented behaviour.

## Testing
- Vitest: `buildTaskLink(origin, projectId, number)` pure helper; `CopyLinkButton` with a mocked `navigator.clipboard` (success shows `Link copied`; rejection shows the fallback field).
- Manual: open a task via URL directly after login; comment and see it appear via the event-driven refetch.
- `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:unit`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- Frontend foundation epic: `ProtectedRoute` preserving the return destination through login and the forced password change so a pasted task link lands on the drawer; `services/users.ts` `getUser`.
- Task tracker epic: `GET /projects/{pid}/tasks/{number}` → `TaskDetail`, `POST .../comments`.