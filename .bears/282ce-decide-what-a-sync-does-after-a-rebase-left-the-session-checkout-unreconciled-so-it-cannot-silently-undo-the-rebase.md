---
id: "282ce"
title: Decide what a sync does after a rebase left the session checkout unreconciled, so it cannot silently undo the rebase
status: open
priority: P2
created: "2026-09-20T19:29:07.440149143Z"
updated: "2026-09-20T19:29:07.440149143Z"
tags:
  - orchestrator
  - git
  - docs
---

## Summary
Observed by the sey9x git E2E specs (`frontend/tests/git.spec.ts`, "a rebase onto a dirty checkout asks the session to reconcile"). When a session branch is rebased while the session's work clone is dirty, the orchestrator writes the rebased commit to `refs/sessions/<sid>` in the mirror and reports `work_tree: reconciliation_required` in the `git` event, leaving the checkout on the pre-rebase commit (`ARCHITECTURE.md`, "Git model", "Merge, rebase, push"). The next sync of that session then writes the pre-rebase commit back over the rebased ref, because the checkout is authoritative for the session ref. The sync is not only the explicit one: `GET /projects/{pid}/git/diff?head=<sid>` syncs the head first, and the Changes panel refetches the diff on every `git` event, so with the panel open the rebase is undone within moments of succeeding.

Evidence from the E2E stack: log line `rebased a branch … commit=51fcd23c… git.work_tree=ReconciliationRequired`, and `git -C <mirror> rev-parse refs/sessions/<sid>` reading `4cbc1cc…` (the pre-rebase commit) a moment later.

Each rule is documented and the result follows from them, so this is a gap in the model, not a coding slip: the user is told the rebase succeeded, and nothing says it was then overwritten. The rebased commits are left unreferenced.

## Documents
- `ARCHITECTURE.md` "Git model" ("Session clone", "Merge, rebase, push"); `SPEC.md` "Git" (diff syncs a session head first; `rebase` outcome `work_tree`); `docs/open-questions.md` if the decision is deferred.

## Acceptance criteria
- [ ] A decision with the user, recorded as an ADR if a real alternative is rejected. Candidates: refuse to rebase a session branch whose checkout is dirty (409 naming the reason) instead of reporting reconciliation afterwards; or keep the rebase and make syncs of that session refuse or skip until the checkout is reconciled, with the state visible in the UI; or accept the behaviour and say so plainly in `ARCHITECTURE.md` and in the `git` event the user sees.
- [ ] The chosen behaviour is implemented with an integration test over real bare repositories (git is never mocked), and `frontend/tests/git.spec.ts` scenario 6 asserts the mirror ref as well as the checkout and the event.
- [ ] `ARCHITECTURE.md` and `SPEC.md` describe the interaction in the same commit.
