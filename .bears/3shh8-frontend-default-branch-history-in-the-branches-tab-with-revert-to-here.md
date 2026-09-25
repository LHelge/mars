---
id: "3shh8"
title: "Frontend: default-branch history in the Branches tab, with \"Revert to here\""
status: done
priority: P2
created: "2026-09-25T21:04:24.804786266Z"
updated: "2026-09-25T22:47:45.473919219Z"
tags:
  - frontend
  - git
  - docs
depends_on:
  - zhj6q
  - zhcgs
parent: ny9yq
attempts: 1
---

## Summary

Add a "History" section to the project's Branches tab, below the integration heads. It shows the first-parent history of the default branch (a head selector if cheap) from `GET /projects/{pid}/git/history` (task zhj6q), in a dense monospace table:
- short commit, subject, time;
- `Requested-By` shown as user, session or system;
- the attributed tasks as links (`taskPath`) and sessions as links;
- "Load older" pagination through `before`.

Each row except the newest has a **Revert to here** action, using `POST /projects/{pid}/git/revert` (task zhcgs). It opens an inline `ConfirmPanel` (a form, so `useFormSubmit`) that:
- says `This adds one commit to <branch> restoring it to <short>; N commits and these tasks are undone:` followed by the list of the rows above it, with their tasks;
- offers an optional "Reopen these tasks" section, with a state select (queue and human states of the project) and a required comment when it is ticked;
- sends `expected_head` = the head the table was read at. A 409 `branch has moved` is shown as advice to reload the history, not as a failure;
- on success, invalidates the history, the branches, the session-branches list and the board, awaited inside the action, and shows the new commit with a pointer to the existing `Push…` action.

## Notes

- Follow CLAUDE.md frontend conventions: `TableHead` with `tableStyles`, `FieldShell` for the select and the comment, icons through `icons.ts`, types in `src/types/` mirroring SPEC exactly, `data-testid` constants. Invoke `/frontend-design` first.
- The panel's list is read from the history already loaded. If the rows between `to` and the head are not all loaded, load them before enabling the confirm.

## Docs (same commit)

- SPEC.md "Frontend", Project page (Branches tab) and Confirmations.
- Add coverage table rows. A Playwright scenario arranges two task merges through the API, reverts to before them with reopen, and asserts the tasks are back in the chosen state without a current hand-off and that the history shows the revert commit.

## Testing

Vitest for any pure helper (for example the rows-between computation). Run the full frontend chain.