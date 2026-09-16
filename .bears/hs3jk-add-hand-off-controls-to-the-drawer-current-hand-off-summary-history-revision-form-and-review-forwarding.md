---
id: hs3jk
title: "Add hand-off controls to the drawer: current hand-off summary, history, revision form and review forwarding"
status: open
priority: P1
created: "2026-09-16T20:44:42.412522137Z"
updated: "2026-09-16T20:44:42.412522137Z"
tags:
  - frontend
  - tracker
  - git
depends_on:
  - dpm8a
parent: gn4y2
---

## Summary
Show and drive commit-bound hand-offs from the drawer: a summary of the current hand-off (source session, branch, pinned commit, comment, review status labelled with the commit it covers), the full history oldest first with the current one marked, a revision form (source session, exact commit, required comment, target state) that publishes through `PUT` with `handoff.kind = "revision"`, and review actions that forward the current hand-off id with `approved` or `changes_requested` (or no decision) and a comment to a target state. Neither form can select the reviewer's own branch implicitly.

## Documents
- `SPEC.md` "Code hand-offs and review" (all paragraphs: `HandoffInput`, different target state required (400), non-empty comment, `source_session_id` required for REST, `commit` full object id, 409 tip mismatch, forwarding requires the current `handoff_id` (409), review decision semantics, `Handoff` shape, worked example)
- `SPEC.md` "Frontend", "Hand-off controls" (the whole paragraph)
- `SPEC.md` "Tasks" (`PUT` with `handoff`, `TaskDetail.handoffs` oldest first, `comment_id`)
- `ARCHITECTURE.md` "Task tracker", "Code hand-offs" and "Review approval"
- `docs/data-model.md` `task_handoffs`
- ADR 0018

## Acceptance criteria
- [ ] `frontend/src/tasks/HandoffPanel.tsx` in the drawer shows, when `task.handoff` is set: source session (link `/sessions/{source_session_id}`, or `deleted session` when null), `source_branch`, `commit` (short with full in `title` and a copy action), the comment body resolved from `comments` by `comment_id`, and a review badge: `Unreviewed`, `Approved · <short commit>` or `Changes requested · <short commit>`, with reviewer (user via `["users", id]` query or session link) and `reviewed_at`. Without a hand-off it shows `No code hand-off`.
- [ ] History list from `handoffs` (oldest first as delivered, rendered newest first is acceptable if labelled), each row: created time, actor (user/session), short commit, review status, comment excerpt, `current` marker when `id === task.handoff?.id`. Each row has a `View diff` action wired by the merge/diff task (render a disabled placeholder here if that task has not landed).
- [ ] `Publish revision` form: `source_session_id` select listing the project's sessions (`listSessions(pid)`; sessions that touched this task first, then the rest; each row shows title, state and short id), `commit` input validated as `^[0-9a-f]{40}$` (hint `Full 40-character commit id; the session branch tip must equal it`), `comment` textarea (required), `state` select excluding the current state (hint `A hand-off requires a different target state`). Submits `updateTask(pid, number, { state, handoff: { kind: "revision", source_session_id, commit, comment } })`; 400 and 409 (`tip mismatch`, stale) bodies shown verbatim; success invalidates `taskKeys.detail` and calls `taskStore.invalidate()`.
- [ ] `Review` actions, enabled only when `task.handoff` is set: buttons `Approve`, `Request changes`, `Forward without decision`; each opens the same form with a `state` select (current excluded) and required `comment`, and submits `{ state, handoff: { kind: "forward", handoff_id: task.handoff.id, comment, review?: "approved" | "changes_requested" } }`. The form states which commit the decision covers (`Approving commit <short>`). A 409 (`handoff_id` no longer current) refetches the task and shows `The hand-off changed; review the new revision`.
- [ ] Both forms clearly show that the source is the hand-off's branch/commit and never default `source_session_id` to any session the viewer happens to have open; the revision form's session select has no default when more than one candidate exists.
- [ ] A newly published revision appears with `Unreviewed` after the refresh; older approvals remain in the history list.
- [ ] `cd frontend && npm run lint && npx tsc -b && npm run build` pass.

## Implementation notes
- Files: `frontend/src/tasks/HandoffPanel.tsx`, `frontend/src/tasks/RevisionForm.tsx`, `frontend/src/tasks/ReviewForm.tsx`, edit `frontend/src/tasks/TaskDetail.tsx`; reuse `useTaskMutations` from the actions task for the `PUT`.
- `listSessions(pid)` from `services/sessions.ts` (session epic); sessions that touched the task come from `task.sessions`.
- Use the `/frontend-design` skill: commits and branches monospace, review status as the only coloured element (approved green-ish, changes requested amber), history as a compact timeline.
- Short commit = 10 characters.

## Edge cases
- The current hand-off's `comment_id` may reference a comment not yet in `comments` only during a partial refresh; show `comment unavailable` rather than crashing.
- `source_session_id` null on old records after session deletion: show `deleted session`; the branch and commit remain.
- A revision with the same commit as the previous one is allowed and still starts `Unreviewed`.
- Both forms disable submit while the `PUT` is in flight; a state move from the `Move to` control never sends `handoff` (planning-only).

## Testing
- Vitest: `reviewLabel(handoff)` and the commit-id validator; `orderSessionsForPicker(sessions, touched)`.
- Manual against a real orchestrator with the stub image: publish a revision at a synced tip (201/200), then with a stale commit (409), approve it, and see the label carry the commit.
- `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:unit`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- Code hand-offs and review epic: `PUT` with `HandoffInput` and the statuses cited.
- Frontend project and session views epic: `services/sessions.ts` `listSessions(pid, filters?)`.