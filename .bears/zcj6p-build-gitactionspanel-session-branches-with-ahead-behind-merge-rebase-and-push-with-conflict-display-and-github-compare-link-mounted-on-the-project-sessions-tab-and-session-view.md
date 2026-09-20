---
id: zcj6p
title: "Build GitActionsPanel: session branches with ahead/behind, merge, rebase and push with conflict display and GitHub compare link, mounted on the project sessions tab and session view"
status: done
priority: P1
created: "2026-09-16T20:48:27.553313318Z"
updated: "2026-09-20T08:49:48.823270470Z"
tags:
  - frontend
  - git
  - projects
  - sessions
depends_on:
  - zv5br
  - k97mz
parent: cgdc2
attempts: 1
---

## Summary
Build the git operations UI shared by the project page and the session view: the list of session branches in the mirror with ahead/behind against the default branch, and the merge, rebase and push actions with their forms, 422 conflict path lists, 409 non-fast-forward errors, success commits, and the client-built GitHub compare link after a push to a `github.com` remote. On the project page it lists every session branch and also offers a generic branch merge (for example `origin/main` into `main`); on the session view it is scoped to that session's branch.

## Documents
- `SPEC.md` "User-facing features", Git operations paragraph (list session branches, merge a session branch into a target, rebase onto a target, push upstream; conflicts reported with paths; after a push to a GitHub remote the UI links to the compare page).
- `SPEC.md` "Git" table and text: `GET /projects/{pid}/git/session-branches` → `SessionBranch[]` (`{session_id, ref: "refs/sessions/<id>", commit, ahead, behind, base, updated_at}`, ahead/behind against the integration head named by `default_branch`); `POST /git/merge` `MergeInput` → `{commit}` or 422 `{status, error, conflicts}` (409 for stale/unapproved task hand-off); `POST /git/rebase` `{branch, onto}` → `{commit}` or 422; `POST /git/push` `{ref, remote_branch?, force?: false}` → `{remote_branch, commit}`; name rules (`source`/`head` accept a session id, integration branch name or `origin/<name>`; `onto`/`base` accept integration or upstream names; `branch`/`ref` accept a session id or integration branch name; `target` accepts an integration branch name; upstream-tracking refs cannot be mutation targets or push sources: 400); non-fast-forward push → 409 preserving local work; force push requires explicit `force: true`; `MergeInput = {target, message?} & ({source} | {task_id, handoff_id})` (the task form is the board epic's).
- `SPEC.md` "Frontend", "Changes panel": after a successful push to a `github.com` remote show `https://github.com/<owner>/<repo>/compare/<target>...<remote_branch>?expand=1`, built client-side from `remote_url`, next to the push result and on the session-branch list.
- `SPEC.md` "Projects": `GET /projects/{id}/branches` (`kind: head | upstream | session`).
- `ARCHITECTURE.md` "Git model" (ref ownership; merge/rebase in a temp clone; session refs pushed as `refs/heads/session/<id>` by default; after a rebase of a session branch the work clone is reset only if clean, otherwise the outcome event reports reconciliation is required; merging `origin/main` into `main` integrates upstream changes).
- `README.md` "Operating notes" (fetch refreshes `origin/<branch>` only; merge `origin/main` into `main` explicitly; a rejected push leaves local work intact).

## Acceptance criteria
- [ ] `frontend/src/utils/github.ts` exports `githubCompareUrl(remote_url, target, remote_branch): string | null` returning the compare URL for `https://github.com/<owner>/<repo>` and `https://github.com/<owner>/<repo>.git` remotes (owner/repo taken verbatim, `.git` stripped) and `null` for any other host; unit-tested.
- [ ] `frontend/src/components/git/GitActionsPanel.tsx` props `{ project: Project, sessionId?: string }`; loads `listSessionBranches(pid)` (key `["projects", pid, "git", "session-branches"]`, refetch on window focus and after every mutation) and `listBranches(pid)` for target/onto options.
- [ ] Branch table: session (link to `/sessions/{session_id}`, title joined from the project sessions query when available), `ref`, `commit` short sha, `ahead`/`behind` as `+n / −m` against `base`, `updated_at` relative; per-row actions `Merge into…`, `Rebase onto…`, `Push…`. With `sessionId` the table is filtered to that session and shows `not synced yet` when its ref is absent (offer the `Sync` action from the session header via a callback prop).
- [ ] Merge form: `target` select over integration heads (default `default_branch`), optional `message`; source is the row's session id (or, in the project-wide generic form, any head/upstream/session name); submits `{target, message?, source}`; success shows `Merged at <short commit>`; 422 shows `Conflicts in:` with the `conflicts` paths as a monospace list; 400 (upstream as target) and other errors show the server message.
- [ ] Rebase form: `onto` select over heads and upstream refs (default `default_branch`); submits `{branch: <session id or head>, onto}`; success `Rebased to <short commit>`; 422 conflict list; the outcome note that a dirty work tree needs reconciliation is surfaced from the resulting `git` transcript event (session view) or the server message.
- [ ] Push form: `ref` fixed to the row (session id or head), `remote_branch` text default `session/<id>` for session refs or the head's own name, `force` checkbox default off with a red warning when checked; submits `{ref, remote_branch, force}`; success shows `Pushed <remote_branch> at <short commit>` and, when `githubCompareUrl(project.remote_url, target, remote_branch)` is non-null, an `Open compare on GitHub` external link where `target` is the project's `default_branch`; 409 shows `Push rejected: upstream has advanced. Fetch, merge origin/<branch> and retry.` plus the server message.
- [ ] After a successful push the branch row keeps a `compare` link (stored in component state keyed by session id until the next refetch replaces `commit`).
- [ ] Project-wide generic merge form (project page only): `source` select over heads, upstream refs and session refs, `target` over heads; preselects `origin/<default_branch>` → `<default_branch>` as the "integrate upstream" shortcut.
- [ ] Mounted below the session list in `SessionsTab` (project page) and in the session view header as a `Branch` popover/section with `sessionId` set.
- [ ] `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:unit` pass.

## Implementation notes
- Files: `frontend/src/components/git/{GitActionsPanel,SessionBranchTable,MergeForm,RebaseForm,PushForm,ConflictList}.tsx`, `frontend/src/utils/github.ts` (+ test), mounting edits in `frontend/src/pages/project/SessionsTab.tsx` and `frontend/src/session/SessionHeader.tsx`.
- All mutations through `services/git.ts`; detect 422 with the `isGitConflict` guard from the types/services task; invalidate `["projects", pid, "git", ...]`, the project (for `last_fetched_at`) and, in the session view, let the `git` event refresh the Changes panel.
- The task-form merge (`task_id` + `handoff_id`) is not built here; the board epic's drawer sends it. Keep `MergeForm` generic so the board can reuse it with a `mode: "branch" | "task"` prop later, but only implement `branch`.
- Compare link target: the SPEC's `<target>` is the integration branch the push should be compared against; use `default_branch` (heads pushed under their own name compare against themselves, so hide the link when `remote_branch === target`).

## Edge cases
- `remote_url` with uppercase owner/repo or trailing slash: keep case, strip the slash.
- A push of `main` itself (integration head) uses `remote_branch = main` by default.
- Rebase of the currently running session's branch is allowed by the API; warn in the form that the agent's checkout is reset only when clean.
- Concurrent operations: disable all forms for the panel while one mutation is pending (the server serialises per project anyway).
- 401/403 from git endpoints are handled by `apiClient` (refresh) and the shared error mapping.

## Testing
- Vitest: `github.test.ts` (`https://github.com/o/r.git`, `https://github.com/o/r`, `https://gitlab.com/o/r` → null, `https://github.com/o/r/` → stripped), `ConflictList` render test, `PushForm` test asserting `force` is sent only when checked and the compare link appears for a github remote with a mocked success.
- Command: `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:unit`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- Git operations epic (backend): the endpoints and status codes above.
- Frontend task board epic: reuses `MergeForm` in task mode and `ConflictList`; not required here.