---
id: g2dcy
title: "Let the drawer's other sub-forms claim Escape: hand-off, review, merge, move and launch forms"
status: open
priority: P3
created: "2026-09-21T19:04:39.103423743Z"
updated: "2026-09-21T19:04:39.103423743Z"
tags:
  - frontend
  - technical-review
  - accessibility
depends_on:
  - kzvp9
parent: "579dz"
---

Problem: kzvp9 made the task drawer a modal `<dialog>` and gave it one Escape rule (frontend/src/tasks/drawerEscape.ts): Escape closes the innermost registered sub-form before the drawer, and asks once over unsaved text. The seam is `useDrawerEscape(close, open)`, one line in the module that owns a sub-form's open state. Only TaskBody's edit form uses it, because the other forms belonged to a sibling task in that round. Until they opt in, Escape over an open review, revision, merge, launch or delete-confirmation form falls through to the dirty-text question and then closes the whole drawer instead of just that form.

Acceptance: every sub-form or confirmation under TaskBody that has an open/closed state registers with `useDrawerEscape`: HandoffPanel (review and revision forms), MergeTaskAction, MoveToState's confirmation, TaskActions' delete confirmation, LaunchForTask's panel, DependencyEditor if it has an open state. Escape closes that form first and the drawer on the next press; a draft still costs the extra press. One Playwright or component scenario per distinct seam is not needed: one scenario for a hand-off form and one unit test that the registry closes innermost-first are enough. SPEC.md, "Frontend", "Task board" already states the rule; correct it only if the behaviour differs.

References: frontend/src/tasks/drawerEscape.ts, TaskDetail.tsx (the one consumer today), HandoffPanel.tsx, MergeTaskAction.tsx, MoveToState.tsx, TaskActions.tsx, LaunchForTask.tsx. Discovered from: kzvp9.