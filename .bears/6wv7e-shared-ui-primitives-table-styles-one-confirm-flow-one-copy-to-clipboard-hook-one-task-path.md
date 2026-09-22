---
id: "6wv7e"
title: "Shared UI primitives: table styles, one confirm flow, one copy-to-clipboard hook, one task path"
status: done
priority: P3
created: "2026-09-21T10:55:57.503816726Z"
updated: "2026-09-22T00:13:28.734892095Z"
tags:
  - frontend
  - technical-review
  - refactor
  - react
  - accessibility
depends_on:
  - s9rxc
parent: "579dz"
attempts: 1
---

Problem:
- Table class constants are redefined per file and drifting (py-2 vs py-1.5, align-top vs align-middle): HEAD in 9 files, CELL in 7, ROW in 13 occurrences — components/admin/tableStyles.ts (the right constants, scoped to one folder), components/git/SessionBranchTable.tsx:38-39, components/secrets/SecretsManager.tsx:31 and SecretRow.tsx:34/:159/:284, pages/DashboardPage.tsx:38-41, ProjectsPage.tsx:39-41, project/SessionsTab.tsx:60-62, ProfilesTab.tsx:25-26, SharedDirsTab.tsx:38-39. Magic `colSpan={8}` (SecretRow.tsx:285) and `colSpan={6}` (SessionBranchTable.tsx:209) are tied to headers defined elsewhere, in another file for secrets.
- Confirm-delete has five idioms: window.confirm (tasks/TaskStatesEditor.tsx:417-426, pages/project/ProfilesTab.tsx, SharedDirsTab.tsx twice), an in-row two-click button (SessionsTab.tsx, whose button has no accessible name tying it to its session), a warning-Alert two-step (ProjectHeader.tsx), and two inline panels (tasks/TaskActions.tsx, MoveToState.tsx). window.confirm is unstyled, blocking, and needs a dialog handler in Playwright.
- tasks/HandoffPanel.tsx CopyCommit (:387-450) duplicates components/CopyLinkButton.tsx: the same copied/manual/timer/select-fallback state machine down to the `navigator.clipboard as Clipboard | undefined` line.
- The task route is hand-built in tasks/TaskCard.tsx:56, session/TasksPanel.tsx:120 and pages/DashboardPage.tsx:247 although tasks/taskLink.ts taskPath exists and DependencyList uses it; `close()` in TaskDetail.tsx:63-67 is the same code as backToBoard() in TaskActions.tsx.
- Smaller accessibility items found on the way: board column and card are both h3 (TaskBoard.tsx:253, TaskCard.tsx:69), so outline navigation cannot tell them apart; CommentList.tsx:33-35 re-sorts with localeCompare on every render (slow ICU collation, and RFC 3339 strings with variable fraction width mis-order within a millisecond) where Date.parse or the API's order would do; COST_DECIMALS is defined in SessionHeader.tsx:43 and messages/ResultMessage.tsx:7.

Acceptance: table styles live in one module under components/ (or a small DataTable/Th), with column counts derived from the header definition. One `<ConfirmPanel tone message confirmLabel pending error onConfirm onCancel />` (inline, console style) replaces every idiom above, each confirm button carries an accessible name naming its target, and window.confirm is gone from src/. One `useCopyToClipboard(text)` hook returning { copied, manual, copy, fieldRef } behind both copy buttons. taskPath and a single board-path helper are the only spellings of those routes. Cards are h4. CLAUDE.md's shared-UI list names the new pieces. Invoke the frontend-design skill for the confirm panel. Playwright scenarios that relied on the native dialog are updated with their coverage rows.

References: files above; frontend/tests/ (dialog handlers). Contract: CLAUDE.md, "Frontend conventions"; SPEC.md, "Frontend".