---
id: g3qdk
title: Delete a session's branch ref when the session is deleted, and sweep orphaned session refs
status: open
priority: P2
created: "2026-09-23T21:16:35.726548249Z"
updated: "2026-09-24T08:19:36.512340Z"
tags:
  - orchestrator
  - frontend
  - git
  - docs
depends_on:
  - ebk7u
parent: kc8k3
---

Today `DELETE /sessions/{id}` removes the session directory and transcript (ARCHITECTURE.md, "Session directories"; SPEC.md, "Sessions" endpoint table) but leaves `refs/sessions/<sid>` in the project mirror (ARCHITECTURE.md, "Git", Ref ownership). No endpoint removes it, so a session that was started, ended and deleted leaves a branch in `GET /projects/{pid}/git/session-branches` whose `session_id` names nothing, and the UI offers no way to get rid of it.

Decision (in discussion with the user, 2026-09-23): the session ref belongs to the session and goes with it. We rejected the alternative of keeping the ref and adding an explicit "delete branch" action. The ref is named by a session that no longer exists; deleting a session is already the explicit, final action (only on `done`/`failed`); and work worth keeping has other homes by then: an integration head, an upstream push, or a hand-off, whose `refs/handoffs/<id>` holds its commit independently (ADR 0018). Record this in a new ADR in `docs/decisions/`.

## Scope

1. **Delete removes the ref.** Session deletion takes the project git lock *before* any database lock (lock order, ARCHITECTURE.md, "Git", Serialization, and "Task tracker"). It deletes `refs/sessions/<sid>` from the mirror through `src/git/` (`git update-ref -d`), then deletes the row. A missing ref is not an error: the session may never have synced, or the project may not be ready.
2. **Orphan sweep.** Extend the hourly orphan cleanup job (ARCHITECTURE.md, "Background jobs", orphan cleanup row). Under each project's git lock it already removes `refs/handoffs/*` with no matching hand-off row. It should also remove `refs/sessions/*` with no matching session row. This cleans up the refs of sessions deleted before this change, and covers a ref delete that succeeded where the row delete then failed.
3. **Warn before losing work.** The session delete `ConfirmPanel` states when the session branch has commits not on the default branch (`SessionBranch.ahead > 0`), for example "`session/<sid>` has 3 commits not on `main`; they will be lost." This is a warning, not a refusal. Decide whether the frontend reads this from the existing session-branches list or the delete endpoint exposes it, and document the choice.
4. **Out of scope:** branches pushed upstream. Mars only pushes, and never deletes a remote branch as a side effect.

## Documentation (rule 1)

- SPEC.md: the `DELETE /sessions/{id}` row ("removes the session directory and its branch ref"), and the Frontend confirmation wording if it changes.
- ARCHITECTURE.md: the session directories paragraph on deletion, "Ref ownership" (`refs/sessions/<sid>` lives as long as its session), and the orphan cleanup row.
- New ADR: delete the session ref with the session rather than a separate branch-delete action.

## Tests

- Integration: deleting a synced session removes `refs/sessions/<sid>` from the mirror (real bare repository, git is never mocked). Deleting a session that never synced still returns 204.
- Orphan cleanup: a `refs/sessions/<uuid>` with no row is removed, and the ref of a live session is kept.
- E2E: the delete confirmation shows the unmerged-commits warning for a session with commits ahead, and after deletion the branch is gone from the project's branch list.