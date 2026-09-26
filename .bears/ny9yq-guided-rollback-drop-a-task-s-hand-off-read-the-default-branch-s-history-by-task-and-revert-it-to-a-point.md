---
id: ny9yq
title: "Guided rollback: drop a task's hand-off, read the default branch's history by task, and revert it to a point"
type: epic
status: open
priority: P2
created: "2026-09-25T21:03:25.419857441Z"
updated: "2026-09-25T22:47:45.474337684Z"
tags:
  - orchestrator
  - frontend
  - git
  - tracker
---

## Scope

Recovering from bad work that has already been merged into the default branch, typically work built on the wrong base, currently takes git on the host and a lot of manual task clean-up. The recovery went like this:
1. find the last good commit;
2. roll the default branch back;
3. find every task merged after that commit;
4. move those tasks out of their terminal state;
5. make sure their next launch does not start from the old hand-off.

Mars already records everything needed for this: auto-merge comments, `Requested-By:` trailers, and `task_handoffs.commit`. This epic makes the recovery a guided flow, for users only.

## Decisions (ADR)

- **Revert, never reset.** "Revert to here" writes one new commit on top of the integration head, whose tree is the chosen commit's tree. It does not move the head backwards. ADR 0050's end-of-session judgement relies on integration heads only moving forward: a session ref deleted because the default branch contained its tip would be silently lost by a reset. A revert also needs no force push upstream.
- **Users only.** None of this is exposed over MCP. Rolling back is a human decision.
- **No general git editor in the UI.** History rewriting belongs in a conversational session's own checkout and goes through the ordinary merge path.

## Acceptance Criteria

- [ ] A user can drop a task's current hand-off, so its next launch starts from the default branch. Hand-off history is kept.
- [ ] The Branches tab shows the default branch's first-parent history, with each entry attributed to the task(s) and session(s) behind it.
- [ ] "Revert to here" on a history row creates the revert commit. In the same confirmed action, it can move the tasks merged after that point to a chosen state and drop their hand-offs.
- [ ] SPEC.md, ARCHITECTURE.md "Git model", an ADR, and the E2E coverage table are updated.