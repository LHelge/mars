---
id: xjaah
title: Code hand-offs and review
type: epic
status: done
priority: P1
created: "2026-09-16T20:13:57.990418377Z"
updated: "2026-09-19T23:50:40.181013760Z"
tags:
  - orchestrator
  - tracker
  - git
depends_on:
  - "5h3y4"
  - s52qg
---

## Scope

Commit-bound hand-offs between tasks and git (ADR 0018).

- `HandoffInput` on `PUT /projects/{pid}/tasks/{id}`: `revision` (source session, exact commit, comment) and `forward` (current hand-off id, comment, optional `review` decision); a different target `state` required.
- Publication protocol: under the project git lock validate task state, holder and current hand-off, sync the source session branch, require the fetched tip to equal the commit, pin `refs/handoffs/<id>`; then the tracker transaction rechecks and writes the `task_handoffs` row, its comment, `current_handoff_id`, lease release, state change, session links and events together. Sync failure or stale state leaves the task unchanged.
- Forwarding: reuse source session, branch and commit; carry or record review attribution; new revisions always `unreviewed`.
- Launch-for-task defaulting: `base_ref` omitted selects the current hand-off's commit atomically with the claim, records `handoff_id`; explicit base overrides and the generated message discloses hand-off id, branch, commit, review status and comment.
- Task merge form of `POST .../git/merge` and the MCP `merge` tool's task form: verify current and `approved` under the git lock, merge the pinned commit, 409 / `conflict` otherwise.
- Diff by `handoff_id`; hand-off history in `TaskDetail`; ref removal on task/project deletion under the git lock (orphan cleanup scheduled by the Background jobs epic).

## Documents

`SPEC.md` "Code hand-offs and review", "Sessions" (launch defaults), "Git" (task merge form, diff); `ARCHITECTURE.md` "Task tracker" -> "Code hand-offs", "Review approval", "Launching a session for a task"; "Git model" (task merge, `refs/handoffs`); `docs/data-model.md` `task_handoffs`; ADR 0018.

## Acceptance criteria

- [ ] Tests with real bare repositories: revision publication pins the commit, a tip mismatch returns 409 with the task unchanged, forwarding with and without a decision, a new revision resets review, a stale `handoff_id` is rejected.
- [ ] Task merge uses the pinned commit even after the source branch advanced; unapproved or stale hand-offs return 409.
- [ ] Launch-for-task records `handoff_id` and the commit as `base_ref`; an explicit base overrides it.

## Out of scope

The MCP transport itself (MCP epic); UI controls (Frontend task board epic).