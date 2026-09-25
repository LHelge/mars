---
id: ftpaq
title: "Board: show that a task is waiting for its author's branch, with the way to land it"
status: open
priority: P2
created: "2026-09-25T21:00:48.788085363Z"
updated: "2026-09-25T21:00:48.788085363Z"
tags:
  - frontend
  - tracker
  - git
  - docs
depends_on:
  - zpsh4
parent: "4txx2"
---

## Summary

A task the dispatcher holds back (task zpsh4) looks, on the board, exactly like one nobody has picked up. Make the reason visible. When a non-terminal, unheld task's `created_by_session_id` names a session whose branch in `GET /projects/{pid}/git/session-branches` has `ahead > 0`, the card and the task drawer show a quiet state line: `Waiting for session <title>'s branch to reach <default branch> (N commits)`. The line links to where the branch can be merged: the project's Branches tab (`?tab=branches`), or the session view's branch panel.

## Implementation Notes

- The data is already in the frontend: `Task.created_by_session_id` (`src/types/tasks.ts`) and the session-branch list with `ahead` (`services/git.ts`; the query key already used by `SessionBranchTable` and `SessionDeleteConfirm`). Join them in a small pure helper beside the board code, for example `tasks/authorBranch.ts` with a `*.test.ts` beside it. Do not add a new endpoint.
- This is the same rule as the dispatcher's: `ahead > 0` against the default branch is "tip not contained in the default branch". The one gap is a live author session that has never synced and so has no ref yet. The dispatcher's check syncs silently and creates that ref, so the board catches up after one dispatcher run. Say so in the SPEC text.
- The session-branch list must refresh when branches change. Reuse the invalidation the Branches tab already relies on (git events / merge success). The board must not poll git on its own schedule beyond what the list already does.
- Title of the author session comes from the cached session list, as `SessionBranchTable` does.
- Follow CLAUDE.md frontend conventions: an icon from `components/icons.ts` if one is used, a route helper rather than a template literal (add a `branchesPath` helper if none exists), and a `data-testid` constant in `src/utils/testIds.ts`. Invoke `/frontend-design` first; this is a quiet state colour, not an alarm.

## Docs (same commit)

- `SPEC.md`, "Frontend", Task board: describe the line, the rule, and the never-synced gap.
- Add a row to `frontend/tests/README.md` coverage table and a Playwright scenario. Arrange a session with a commit on the stub image (see existing session-branch E2E scenarios), a task created by that session, and assert the line. Then merge the branch through the API and assert the line goes away.

## Testing

- Vitest for the join helper: ahead > 0, ahead 0, no ref, no author, terminal task, held task.
- Full frontend chain.