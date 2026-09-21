---
id: c9tju
title: Reset task drafts and editor state when the displayed resource changes
status: open
priority: P1
created: "2026-09-21T10:44:29.449408078Z"
updated: "2026-09-21T10:49:50.108933306Z"
tags:
  - frontend
  - technical-review
  - bug
  - tracker
  - react
parent: "579dz"
---

Problem: TaskDetail renders TaskBody without a resource-specific key. Navigating from task A in edit mode to already-cached task B can preserve A's draft while mutation callbacks now target B. Cached results avoid the loading-state unmount that can otherwise hide this bug.

Acceptance: establish an explicit task identity boundary, for example key TaskBody by task.id, so edit mode, drafts, comments, and other resource-owned state reset appropriately. Audit project/profile/settings editors for the same prop-to-state lifetime problem and fix analogous cases found. Do not reset an in-progress draft on a refresh of the same resource. Add a cached A-to-B navigation regression proving A's content cannot be saved into B, plus same-resource refresh coverage.

References: frontend/src/tasks/TaskDetail.tsx:154; frontend/src/tasks/TaskEditForm.tsx; frontend/src/pages/ProjectPage.tsx and project/ProjectSettingsForm.tsx. SessionPage already keys its route by session ID. Contract: SPEC.md, "Frontend", Routes and task board; React guidance: https://react.dev/learn/preserving-and-resetting-state .

Merged from the second review (2026-09-21), same finding, the audit list it produced:
- Reachable today without typing a URL: child links and DependencyList links stay rendered while the edit form is open (TaskDetail.tsx:226-240, DependencyList.tsx:172), and browser Back returns to a cached task.
- State under the unkeyed TaskBody that survives A -> B: TaskEditForm fields (form is relabelled "Edit task #B" and PUTs A's fields); ReviewForm comment (keyed only by decision, HandoffPanel.tsx:145, so a comment written for A approves B's hand-off); RevisionForm source/commit/comment; CommentForm body (CommentForm.tsx:31); MoveToState target and `confirming` (MoveToState.tsx:36-37: an open "Move #A to done" confirm re-labels itself for B); TaskActions confirmingDelete (TaskActions.tsx:44); DependencyEditor (DependencyEditor.tsx:54-56); HandoffPanel open/viewing; LaunchForTask kind, message and base override; MergeTaskAction open; every useMutation error from useTaskMutations.
- The comment at the DOM-id block of TaskEditForm ("the drawer can be replaced by another task's without unmounting") shows the hazard was known for ids but not for state.
- Analogous latent case: ProjectView is not keyed by id (pages/ProjectPage.tsx:59). ProjectHeader confirmingDelete/error (ProjectHeader.tsx:42-43), ProjectSettingsForm fields (:38-40) and settingsOpen would survive A -> B when B is cached, so a "Delete A?" confirmation would act on B. No direct A -> B link exists today (grep), so it is latent; `key={id}` closes it.
- Already right and worth keeping: key={selected} on the profile editor, key={kind} on the launch panel, key={shown.id} on HandoffDiff.