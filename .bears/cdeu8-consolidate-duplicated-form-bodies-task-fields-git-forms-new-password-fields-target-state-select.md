---
id: cdeu8
title: "Consolidate duplicated form bodies: task fields, git forms, new-password fields, target-state select"
status: open
priority: P3
created: "2026-09-21T10:55:43.142436051Z"
updated: "2026-09-21T10:56:48.992686022Z"
tags:
  - frontend
  - technical-review
  - refactor
  - react
depends_on:
  - s9rxc
  - "9c5rg"
  - tmd2v
parent: "579dz"
---

Problem: the same form is typed out per feature. With the field primitives (s9rxc) and the submission convention (9c5rg) in place, these collapse:
- CreateTaskForm and TaskEditForm are about 70% the same form: TITLE_MAX (CreateTaskForm.tsx:28, TaskEditForm.tsx:46), the title/labels validation block verbatim (CreateTaskForm.tsx:88-99, TaskEditForm.tsx:119-129), and the Title, Description, Priority, Labels and Parent JSX (CreateTaskForm.tsx:110-232, TaskEditForm.tsx:154-286). CreateTaskForm.tsx:31-36 also redefines PRIORITIES with labels beside PRIORITIES/PRIORITY_MEANING in taskChrome.ts, which TaskEditForm uses. Shape: `validateTaskFields(values)` in taskEdit.ts plus `<TaskFields values onChange errors idPrefix parents extra />`; Create adds State and Blocked-by, Edit adds Assignee.
- The "Move to" target-state select is triplicated: ReviewForm.tsx:93-120 and RevisionForm.tsx:189-216 are identical including the filter and STATE_HINT; MoveToState.tsx is a third variant. Shape: `<TargetStateSelect id task value onChange disabled />`.
- MergeForm, PushForm and RebaseForm repeat six blocks: the form shell (MergeForm.tsx:102-108, PushForm.tsx:90-96, RebaseForm.tsx:84-90), the read-only "label + mono value" block (MergeForm.tsx:139-144, PushForm.tsx:98-103, RebaseForm.tsx:92-97), the conflict trio — state, `try/catch isGitConflict`, <ConflictList> — line for line (MergeForm.tsx:70-71/:87-94/:194-196, RebaseForm.tsx:61-62/:71-78/:160-162, third copy in tasks/MergeTaskAction.tsx), the success span, the trailing form.error Alert, and the optgroup rendering (MergeForm has an Options helper at :206-228, RebaseForm inlines it twice at :119-136); each also carries the formId/disabled/onBusy triple and a useReportBusy call. Shape: `useGitAction<T>(run)` returning { submit, loading, error, conflicts, result }, a GitFormShell, `<ReadOnlyField>`, an exported `<RefOptions>`. GitActionsPanelProps and MergeFormProps have optionals that only apply to one mode (onSync/syncing/workTreeNote only with sessionId; source/sourceLabel vs defaultSource): make them discriminated unions while here.
- Password + confirm is implemented three times with near-identical state, validation and JSX: pages/AcceptInvitePage.tsx, pages/ResetPasswordPage.tsx, components/PasswordChangeForm.tsx. Shape: `useNewPassword()` returning { fields, validate(), reset() } plus `<NewPasswordFields disabled />`. AcceptInvite and ResetPassword also write `token ?? ""` because the submit closure sits above the guard; split into an inner component taking `token: string`, as ProjectPage/ProjectView already do for id.
- Seams in the large files: TaskStatesEditor.tsx (586 lines) splits at AddStateForm (:206-332) and StateRow (:334-573); TaskDetail.tsx (509) — Meta, Row, Stamp (:275-419) and Sessions (:451-504) are presentational; HandoffPanel.tsx (450) — History/HistoryRow (:270-358). pages/ProfileEditorPage.tsx (788 after the credentials work) is not a route: it is a panel of ProfilesTab whose logic lives in pages/project/profileForm.ts, so imports zig-zag; rename to pages/project/ProfileEditor.tsx and cut a SecretsFieldset (with useSecretNames and mergeSecretOptions), a ServedStatesFieldset owning the task-states query, and a generic CheckboxGroup replacing the three near-identical tools/states/secrets blocks with their "orphans" tails.

Acceptance: each duplicated body exists once, callers keep their domain-specific parts explicit, behaviour and DOM test ids are unchanged (the Playwright suite and existing unit tests pass untouched apart from import paths). No oversized generic form framework: every abstraction introduced has at least two callers on the day it lands. Invoke the frontend-design skill only if markup visibly changes.

References: files above. Contract: CLAUDE.md, "Frontend conventions" (shared UI list updated for anything promoted to components/).