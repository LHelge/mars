---
id: "7yh4k"
title: End confirmation names a session's commits that are not on the default branch
status: in_progress
priority: P2
created: "2026-09-25T21:00:48.811759234Z"
updated: "2026-09-25T21:01:12.851980899Z"
tags:
  - frontend
  - sessions
  - git
  - docs
parent: "4txx2"
attempts: 1
---

## Summary

The End confirmation in `src/session/SessionActions.tsx` currently says only "End this session? The container stops…". Add the warning the delete confirmation already gives (ADR 0049; `SessionDeleteConfirm`, `sessionDeleteWarning.ts`). When the session's branch has `ahead > 0` against the default branch, the panel adds: `session/<sid> has N commits not on <default>. Tasks it filed will not be dispatched until they are merged.` It also links to the branch panel so the commits can be merged. This is a warning and never a refusal.

## Implementation Notes

- Reuse `sessionDeleteWarning.ts`. Generalise it, for example to a `sessionUnmergedWarning(branch, defaultBranch, purpose)`, rather than copying it, and keep the delete wording unchanged.
- Read `SessionBranch.ahead` through the same cached `session-branches` query when the panel opens, exactly as the delete panel does. Until the list arrives, or if it cannot be read, say nothing about commits.
- The ref reflects the last sync. For a live session that has never synced there is no row and the panel says nothing, which is acceptable: the end's own fetch-back keeps the ref, and the board line (sibling task) will show it. Do not add a sync call to opening the panel.
- `ConfirmPanel` stays the owner of the confirm. `useMutation` state is read off the mutation (CLAUDE.md "Submitting a form").

## Docs (same commit)

- `SPEC.md`, "Frontend", Confirmations: extend the paragraph on deleting a session to cover ending one.
- Add an E2E coverage table row and scenario, if the existing delete-warning scenario can be extended cheaply. Otherwise add a Vitest for the warning text.

## Testing

- Vitest for the generalised warning helper (both purposes, singular and plural).
- Full frontend chain.