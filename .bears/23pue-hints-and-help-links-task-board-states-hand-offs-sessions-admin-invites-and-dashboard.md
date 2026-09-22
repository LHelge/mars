---
id: "23pue"
title: "Hints and help links: task board, states, hand-offs, sessions, admin invites and dashboard"
status: in_progress
priority: P3
created: "2026-09-22T19:08:55.783831647Z"
updated: "2026-09-22T21:22:54.828105025Z"
tags:
  - frontend
depends_on:
  - ujccg
  - snn6t
  - v9rjg
parent: gtbp5
attempts: 1
---

Tighten hints and add `help` links. Invoke `/frontend-design` first. Update tests that assert old strings.

- `tasks/TaskBoard.tsx` `BOARD_HELP`: help → `task-flow`. `TaskStatesEditor.tsx` / `AddStateForm.tsx`: kind cannot change after creation, one human state per project; Name format hint.
- `tasks/HandoffPanel.tsx`: a one-line explanation of what a hand-off is, help → `task-flow`. RevisionForm: sync the session first; uncommitted work is not included. MergeTaskAction: "Merges the approved hand-off commit, not the branch's latest tip."
- `tasks/TaskMeta.tsx` Attempts: show against the project's max attempts ("escalates at N") rather than a bare count with a tooltip.
- Session retry/relaunch (`session/SessionActions.tsx`): say an ephemeral session cannot be retried; launch a new one. (Coordinate with the git-panel hints task, which touches Sync in the same file.)
- `components/admin/InvitesPanel.tsx` / `InviteRow.tsx`: without email configured the invitation link is only in the orchestrator log — the frontend cannot know `RESEND_API_KEY`, so word it conditionally ("Sent by email when the instance has email configured; otherwise an operator finds the link in the orchestrator log").
- `pages/DashboardPage.tsx` "Needs a human": "Resolve them on the task, then move them back to a queue state." `SettingsPage.tsx` notifications: when an escalation email is sent (assignee, else admins).