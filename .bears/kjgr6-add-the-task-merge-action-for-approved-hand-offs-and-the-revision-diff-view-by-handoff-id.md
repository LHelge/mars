---
id: kjgr6
title: Add the task merge action for approved hand-offs and the revision diff view by handoff_id
status: open
priority: P2
created: "2026-09-16T20:45:10.847523110Z"
updated: "2026-09-16T20:45:10.847523110Z"
tags:
  - frontend
  - tracker
  - git
depends_on:
  - hs3jk
parent: gn4y2
---

## Summary
Complete the hand-off surface with git: a `Merge` action in the drawer that sends the task form of `POST /projects/{pid}/git/merge` (`task_id` + `handoff_id`) and is enabled only for an approved current hand-off, and a `View diff` for any hand-off in the history that calls `GET /projects/{pid}/git/diff?handoff_id=` to show the retained commit's files and patch without syncing a live branch. Extends `services/git.ts` with the two parameter forms.

## Documents
- `SPEC.md` "Git" (`GET .../git/diff` with `?handoff_id=&base=`; `POST .../git/merge` `MergeInput` task form → `{commit}`, 422 `{status, error, conflicts}`, 409 stale or unapproved; `Diff` shape with `files`, `patch`, `truncated`; exactly one of `head`/`handoff_id`)
- `SPEC.md` "Frontend", "Hand-off controls" (merge action sends `task_id` and `handoff_id`, enabled only for an approved current hand-off; diff by `handoff_id`)
- `SPEC.md` "Frontend", "Changes panel" (diff renderer reuse, file list with counts)
- `SPEC.md` "Projects" (`GET /projects/{id}/branches` → `Branch[]`, `kind: "head"` for integration heads)
- `ARCHITECTURE.md` "Task tracker", "Review approval" (UI merge requires approval and merges only the pinned commit)
- ADR 0018

## Acceptance criteria
- [ ] `frontend/src/services/git.ts` gains (or already has, then extend) `getDiff(pid, params: { head: string; base?: string } | { handoff_id: string; base?: string })` → `Diff` and `mergeBranch(pid, { target, message?, source })` / `mergeTaskHandoff(pid, { target, message?, task_id, handoff_id })` → `{ commit }`; conflict responses (422) are surfaced as a typed error carrying `conflicts: string[]`.
- [ ] `frontend/src/tasks/MergeTaskAction.tsx` in the drawer action bar: button `Merge approved hand-off` enabled only when `task.handoff && task.handoff.review_status === "approved"`; otherwise disabled with reason `Requires an approved current hand-off`. It opens a form with `target` select of integration heads (`listBranches(pid)` filtered to `kind === "head"`, default `default_branch`), optional `message`, and the sentence `Merges commit <short> exactly; later commits on <source_branch> are not included`. Submit calls `mergeTaskHandoff(pid, { target, message, task_id: task.id, handoff_id: task.handoff.id })`.
- [ ] Success shows `Merged as <short commit>` and invalidates `taskKeys.detail`, `["projects", pid, "branches"]` and calls `taskStore.invalidate()`. 422 shows `Conflicts` with the `conflicts` paths listed in monospace; 409 shows the body's `error` (stale or unapproved) and refetches the task.
- [ ] `View diff` on any history row (and on the current summary) opens `HandoffDiff` which fetches `getDiff(pid, { handoff_id })` with `useQuery(["projects", pid, "git", "diff", { handoff_id }])`, shows `base`, `head`, `merge_base`, the file list with `status`, `+additions/-deletions`, a `truncated` notice when set, and renders `patch` with the shared diff renderer.
- [ ] `cd frontend && npm run lint && npx tsc -b && npm run build` pass.

## Implementation notes
- Files: edit `frontend/src/services/git.ts`, `frontend/src/tasks/MergeTaskAction.tsx`, `frontend/src/tasks/HandoffDiff.tsx`, edit `frontend/src/tasks/HandoffPanel.tsx` (wire `View diff`).
- Diff renderer: reuse the component the session epic builds for edit tools and the Changes panel (expected under `frontend/src/session/` as a named export such as `DiffView`); if it is not yet available, render the patch in a monospace `<pre>` with added/removed line colouring and leave a TODO comment referencing this task to swap it.
- `listBranches(pid)` from `services/projects.ts` (session/project epic); add it there if missing.
- `apiClient` error type: if the foundation's `ApiError` carries the parsed body, read `conflicts` from it; otherwise extend `ApiError` minimally with an optional `body` field.

## Edge cases
- The current hand-off can change between rendering and clicking (a new revision): the server answers 409; refetch and re-disable the button (new revision is unreviewed).
- `Diff.patch` above 1 MiB is truncated server-side; show the notice and do not attempt to fetch more.
- The diff endpoint requires exactly one of `head` or `handoff_id`; the service signature makes that a type-level guarantee.
- Merging is not a task move; the task stays in its state (e.g. `merge`) until someone moves it; say so in the success message (`Task state unchanged`).

## Testing
- Vitest: `canMerge(task)` predicate (null hand-off, unreviewed, changes_requested, approved) and the conflict-error parsing.
- Manual with the stub image and a real bare repository: approve a hand-off, merge, see `{commit}`; advance the source branch and confirm the merged commit is the pinned one.
- `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:unit`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- Git operations epic and Code hand-offs epic: diff by `handoff_id`, task merge form with 409/422 semantics.
- Frontend project and session views epic: `services/git.ts` base functions, `services/projects.ts` `listBranches`, the shared diff renderer.