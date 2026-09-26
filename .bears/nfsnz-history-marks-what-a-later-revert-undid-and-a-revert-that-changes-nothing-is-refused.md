---
id: nfsnz
title: History marks what a later revert undid, and a revert that changes nothing is refused
status: open
priority: P1
created: "2026-09-26T08:41:05.210645999Z"
updated: "2026-09-26T08:41:05.210645999Z"
tags:
  - orchestrator
  - frontend
  - git
  - docs
parent: ny9yq
---

## Summary

This was found in the first real use of guided rollback, on the airgap project on 2026-09-26. A user reverted `main` to `83e2ac5`. The revert worked: it wrote `a692eea` `Revert main to 83e2ac5b77af`. But the History section still looked unchanged. The revert adds one row on top and leaves every older row as it was. The rows it undid looked exactly like live ones, and "Revert to here" was still offered on `83e2ac5` itself, where it would write an empty commit. The user concluded the revert had not run.

## Acceptance Criteria

### Backend

`POST /projects/{pid}/git/revert` refuses a revert that would change nothing.

- When the head's tree is already `to`'s tree, answer 409 `nothing to revert: <branch> already matches <short to>`. Nothing is written: no commit, no ref move, no reopen.
- The check is done under the git lock, after the `expected_head` check. Compare with `git rev-parse <head>^{tree}` against `<to>^{tree}`, using `--end-of-options`; it must work on git 2.39.

### History marks what a later revert undid

- `HistoryEntry` gains `tree: string` (the commit's tree id) and `reverted_by: string | null`: the full id of the newest revert commit, above this entry on the first-parent line, that undid it. The endpoint derives `reverted_by` from the listing itself, so no stored state is involved.
- A revert commit is recognised by its message: the subject `Revert <branch> to <short>`, followed by the `Requested-By` trailer. Parse the short `to` from the subject and match it against the full ids on the first-parent line. Keep this parsing in `git/history.rs`, next to the message format in `git/revert.rs`, as one pair.
- A revert R to `to` undoes every entry strictly between `to` and R. Nested reverts must be handled: a later revert to an older point undoes an earlier revert, and everything that earlier revert undid.
- Pagination: an entry undone by a revert on an earlier page must still be marked. Either walk from the head to the page's end for revert commits, which is cheap because it only needs subjects, or compute the answer across pages. Choose one and document it.

### Frontend

- An entry with `reverted_by` is drawn muted, with "undone by <short>" linking to that row, and offers no "Revert to here".
- "Revert to here" is not offered on any row whose `tree` equals the head row's tree. That row is labelled as the branch's current content.
- After a revert succeeds, scroll to or highlight the new top row, so the change is visible.
- A 409 `nothing to revert` gets its own message, like `branch has moved`.

### Docs, in the same commit

- `SPEC.md` "Git": the `HistoryEntry` fields and the new 409.
- `SPEC.md` "Frontend": the Branches tab History and Confirmations text.
- `ARCHITECTURE.md` "Git model": the History and Revert paragraphs.
- The in-app help page `frontend/src/help/branches.md`, "Rolling back": say that a revert adds a row and marks the ones it undid.

## Testing

- Git tests on real bare repositories: a no-op revert is refused and writes nothing; `reverted_by` is set for a simple revert, for a nested revert, and across a page boundary; a merge whose subject merely looks like a revert is not taken for one unless its trailer matches.
- HTTP tests: the 409.
- Vitest for the row-marking helper.
- Extend the existing Playwright revert scenario: after the revert, the undone rows show "undone by", and the row reverted to offers no "Revert to here".