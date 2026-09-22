---
id: uz3wt
title: Correct frontend lifecycle comments and remove stale implementation narratives
status: in_progress
priority: P3
created: "2026-09-21T10:44:44.930812371Z"
updated: "2026-09-22T00:04:07.518944278Z"
tags:
  - frontend
  - technical-review
  - docs
  - refactor
depends_on:
  - wsckz
  - c9tju
  - addgw
  - "5453d"
  - ntepg
  - "9c5rg"
  - xreap
  - h2uej
parent: "579dz"
attempts: 1
---

Problem: several comments promise lifecycle behavior the code does not implement, including session-store reset on ID changes and automatic login navigation after repeated socket auth rejection. Extensive implementation narration makes these inaccuracies difficult to spot.

Acceptance: after the lifecycle fixes, audit comments around auth, streams, stores and form state; correct inaccurate guarantees and remove obsolete task/epic/future-work narration. Retain concise explanations of ownership, ordering constraints, invariants and non-obvious decisions. Ensure behavioral guarantees point to actual implementation and relevant regression coverage. Keep authoritative rules in SPEC.md/ARCHITECTURE.md rather than duplicating lengthy prose in many modules. No tests are required solely for wording changes.

References: frontend/src/session/sessionStore.ts:883; frontend/src/session/useSessionSocket.ts:257; frontend/src/AuthBootstrap.tsx; frontend/src/tasks/useTaskMutations.ts. Contract: CLAUDE.md documentation rule; ARCHITECTURE.md, "Frontend architecture"; SPEC.md, "Frontend".

Merged from the second review (2026-09-21): further comments found to state something the code does not do. Each is either corrected here or made true by the task named.
- session/Transcript.tsx:46-48 "the React Compiler skips ..." and the header of session/messages/MessageRow.tsx "re-renders one row and leaves the rest alone": there is no React Compiler (vite.config.ts uses plain react(), no compiler dependency) and MessageRow is not memoised. Made true by the transcript render-cost task.
- session/useSessionSocket.ts:306 "A self-service password change installed a new pair": the handler fires on every refresh rotation (services/auth.ts installSession). Made true by the stream-teardown task.
- tasks/TaskEditForm.tsx:4-7 and the diffTaskInput doc comment: "two people editing different fields ... do not overwrite each other". Made true by wsckz.
- tasks/useTaskMutations.ts header "Every write the task drawer makes, in one place" (CommentForm, MergeTaskAction, LaunchForTask, CreateTaskForm and TaskStatesEditor each settle on their own) and :6-8 "a mutation never produces two reads" (every local write is two refreshes: invalidate() and the mutation's own SSE event each bump eventGeneration).
- components/admin/UsersTable.tsx:4 "Every row action is its own useMutation": there is one per action for the whole table. Made true by the per-row mutation task.
- session/SessionSocketContext.ts:3 says SessionPage publishes the context; SessionView does.
- pages/SessionPage.tsx:108-111 "a store that already holds a session is ahead of this REST read": false on a return visit (addgw).
- tasks/taskLink.ts:5-6 "the board, the drawer and their tests all spell the link the one way": TaskCard.tsx:56, session/TasksPanel.tsx:120 and pages/DashboardPage.tsx:247 hand-build the path.
- components/secrets/SecretRow.tsx header "lives in this component's state until the mutation settles and nowhere else": the replacement value survives closing the panel (secrets task).
- pages/DashboardPage.tsx and ProjectsPage.tsx credit `placeholderData: keepPreviousData` for "an error never blanks the table"; on a constant query key that option is a no-op and the retention is TanStack's default.
- types/index.ts header says everything is `import type` while it exports the runtime const PROFILE_GATED_TOOLS.