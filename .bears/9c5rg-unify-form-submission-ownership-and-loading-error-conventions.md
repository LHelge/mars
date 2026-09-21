---
id: "9c5rg"
title: Unify form submission ownership and loading/error conventions
status: done
priority: P2
created: "2026-09-21T10:44:41.307126474Z"
updated: "2026-09-21T20:46:02.137142605Z"
tags:
  - frontend
  - technical-review
  - refactor
  - react
depends_on:
  - wsckz
  - c9tju
  - s9rxc
  - wast2
parent: "579dz"
attempts: 1
---

Problem: forms mix useFormSubmit with TanStack mutations and independently maintained loading/error/success state. This creates multiple submission lifecycles and makes double-submit protection, API error handling and cache invalidation harder to reason about.

Acceptance: inventory the existing patterns and establish a documented convention aligned with useFormSubmit and TanStack Query responsibilities. Consolidate duplicated lifecycle logic with one owner of pending/error state per submission, while keeping domain-specific validation and mutation/cache invalidation explicit. Migrate inconsistent forms, preserving API error text, preventing duplicate submission, and avoiding misleading success/error state after a resource change. Use focused tests for meaningful behavioral risks; do not add implementation-mirroring tests. Update CLAUDE.md if the agreed convention changes.

References: frontend/src/hooks/useFormSubmit.ts; frontend/src/pages/project/ProjectSettingsForm.tsx; frontend/src/tasks/TaskEditForm.tsx and useTaskMutations.ts; frontend/src/components/git/*.tsx. Contract: CLAUDE.md, "Frontend conventions" and "Testing expectations"; SPEC.md, "Frontend".

Merged from the second review (2026-09-21), same finding, the inventory it produced:
- useFormSubmit today: CreateTaskForm, LaunchForTask, MergeTaskAction, AddStateForm, ProfileEditor, LaunchSessionForm, ProjectCreateForm, the git forms and all auth pages. Raw useMutation: TaskEditForm, ReviewForm, RevisionForm, MoveToState, StateRow (TaskStatesEditor), ProjectSettingsForm, SharedDirForm. Error rendering differs accordingly (`add.error` string vs projectErrorMessage(update.error) vs reviewErrorMessage), which is where the copies consolidated in wast2 came from; this task now depends on it.
- ProjectSettingsForm.tsx:41-42/:62-74 and SharedDirForm.tsx:37-39/:52-60 hand-roll `error` and `saved` beside a mutation that already holds save.error and save.isSuccess, with save.reset() available on edit. Good contrast to keep: SharedDirRow reads `remove.error ?? clear.error` directly.
- useTaskMutations builds five useMutation observers per call and its six call sites use one or two each (about 30 observers per open drawer); its `pending` aggregate (useTaskMutations.ts:53, :112-117) has no reader and, with one instance per call site, could not mean "some write is in flight" anyway. Split into per-verb hooks over one shared settle helper and delete `pending`.
- The settle rule (invalidate the detail, then useTaskStore.getState().invalidate()) is copy-pasted in useTaskMutations.ts:62-71, CommentForm.tsx:37-40, MergeTaskAction.tsx:136-141, LaunchForTask, CreateTaskForm.tsx:81 and, as a variant, TaskStatesEditor.tsx:93-104. Export one useSettleTask(projectId, number). remove.onSuccess (useTaskMutations.ts:86-90) invalidates the detail of the task it just deleted, guaranteeing one wasted 404 before backToBoard.
- Lingering state to cover under "misleading success/error state": TaskStatesEditor's rename error row renders whenever draftError or rename.error is set, regardless of `editing`, so Cancel leaves a red "Name must be..." under a row no longer being edited (TaskStatesEditor.tsx:459-469, :554-562), and `failure = remove.error ?? move.error` (:428) survives a later successful action on the same row. ProfileEditor's header Cancel is not disabled during save although the footer one is, so a mid-save click unmounts the form and onClose runs twice.
- Forms with six to nine parallel useStates where one values object would do (TaskEditForm, CreateTaskForm, RevisionForm); a single values state is also what makes wsckz's snapshot trivial and the shared task-fields component possible.
- What is sound and should survive the convention: useFormSubmit's ref-based in-flight guard, the action held in a ref written post-commit so `submit` is stable, `finally` always clearing loading, and errors never escaping as unhandled rejections, which is what makes every `void x.submit()` safe.