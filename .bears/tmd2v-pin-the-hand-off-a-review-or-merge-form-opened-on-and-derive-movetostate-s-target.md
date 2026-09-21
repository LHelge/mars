---
id: tmd2v
title: Pin the hand-off a review or merge form opened on, and derive MoveToState's target
status: open
priority: P2
created: "2026-09-21T10:52:16.163757837Z"
updated: "2026-09-21T10:52:16.163757837Z"
tags:
  - frontend
  - technical-review
  - bug
  - tracker
  - react
parent: "579dz"
---

Problem: three tracker forms follow a prop that keeps moving under a user who has already decided. Same family as wsckz, different forms and fix.
- ReviewForm: HandoffPanel passes `handoff={current}` (always task.handoff from the latest refetch) and keys the form only by decision (tasks/HandoffPanel.tsx:141-151); ReviewForm sends `handoff_id: handoff.id` (ReviewForm.tsx:71). A reviewer opens Approve for commit A and types a comment, an agent publishes revision B, the drawer refetches, the cover line quietly becomes "Approving commit B..." and Submit approves code the reviewer never looked at. The stale-hand-off 409 path written for exactly this (handoffRules.ts:137-154) can only fire in the sub-second window before the refetch lands.
- MergeTaskAction has the same shape (`handoff={task.handoff}`, MergeTaskAction.tsx:71-79); less harmful because a new revision is unreviewed and the server refuses it. Also in MergeHandoffForm: the Merge button stays armed beside the success text after `merged` is set, so a second click sends a second merge (:224), and chosenOr returns the value unchanged when there are no options (components/git/formState.ts:64), so for a project with no integration head the target is default_branch, the select shows "No integration head", and Merge is enabled into a 400 (:111-114). merged/conflict/refused are three useStates for mutually exclusive outcomes of one request.
- MoveToState holds `useState(task.state)` (MoveToState.tsx:36, :40). The drawer is open on a task in `ready`; an agent hands it to `review`; task.state changes but target is still "ready", so `unchanged` is false, the select shows `ready` and Move is armed although the user chose nothing; Move plus confirm sends the task back and clears its lease and attempts.

Acceptance: a review or merge form submits the hand-off id it was opened on, so the server's 409 does its job, and shows that a newer revision arrived instead of silently retargeting (pin with useState(handoff.id), or key the form by `${handoff.id}:${decision}` and explain the reset). MoveToState stores only the user's explicit choice (`useState<string | null>(null)`, `target = choice ?? task.state`) and clears it on success. Merge submit is disabled once merged and when branches have loaded with no heads; the merge outcome is one discriminated state. Component tests: rerender with a new current hand-off under an open ReviewForm and assert the submitted id and the notice; rerender MoveToState with an externally changed state and assert Move stays disabled.

References: frontend/src/tasks/HandoffPanel.tsx, ReviewForm.tsx, MergeTaskAction.tsx, MoveToState.tsx, handoffRules.ts; frontend/src/components/git/formState.ts. Contract: SPEC.md, "Tasks" (hand-off review, stale hand-off 409) and "Frontend".