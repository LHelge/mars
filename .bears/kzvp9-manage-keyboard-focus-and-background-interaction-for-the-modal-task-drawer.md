---
id: kzvp9
title: Manage keyboard focus and background interaction for the modal task drawer
status: done
priority: P2
created: "2026-09-21T10:44:49.596495260Z"
updated: "2026-09-21T19:25:56.033202496Z"
tags:
  - frontend
  - technical-review
  - accessibility
  - react
parent: "579dz"
attempts: 1
---

Problem: TaskDetail declares role=dialog and aria-modal=true, but does not move focus into the drawer, contain keyboard focus, restore the opener's focus, or make the underlying board inert. Keyboard users can reach controls behind the modal overlay.

Acceptance: focus a sensible element when opening the drawer; keep Tab/Shift+Tab inside; prevent interaction with the obscured background; preserve Escape/Close behavior; restore focus to the opener when possible and choose a sensible fallback for direct links or a deleted opener. Handle task-to-task navigation and nested controls without losing focus. Use a native dialog or a small established accessible composition as appropriate; invoke the frontend-design skill if implementation reshapes UI, per CLAUDE.md. Add browser-level keyboard tests and update the E2E coverage table.

References: frontend/src/tasks/TaskDetail.tsx and frontend/tests/tasks.spec.ts. Contract: SPEC.md, "Frontend", Routes/task board; CLAUDE.md, "Frontend conventions" and "Frontend E2E" testing expectations.

Merged from the second review (2026-09-21), same finding plus the Escape behaviour it should settle at the same time:
- The document-level keydown handler (TaskDetail.tsx:69-79) closes the drawer on any Escape: it does not check event.defaultPrevented or event.isComposing, nor whether a sub-form is open or dirty. A user eight lines into a description, revision comment or review comment who presses Escape (dismissing an IME/autocomplete popup, or out of habit) loses the text with no prompt. Acceptance addition: ignore the key when defaultPrevented or isComposing; while a sub-form is open Escape closes that form first, and a dirty draft is not discarded silently. A native <dialog> with showModal gives focus-in, trap and restore for free but its `cancel` event needs the same dirty check.
- Note for whoever keys TaskBody by task id (c9tju): task-to-task navigation remounts the body, so the focus target after an A -> B link click has to be chosen here.