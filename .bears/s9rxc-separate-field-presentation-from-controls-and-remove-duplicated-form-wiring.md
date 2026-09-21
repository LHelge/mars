---
id: s9rxc
title: Separate field presentation from controls and remove duplicated form wiring
status: done
priority: P2
created: "2026-09-21T10:44:39.152212355Z"
updated: "2026-09-21T18:14:06.389848014Z"
tags:
  - frontend
  - technical-review
  - refactor
  - accessibility
  - react
parent: "579dz"
attempts: 1
---

Problem: FormField requires value/onChange even when children provide the real select/textarea/output. Call sites supply dummy callbacks or duplicate handlers, and custom children bypass automatic aria-describedby, aria-invalid and other control attributes. Input styles are repeated across editors.

Acceptance: separate the label/hint/error wrapper from concrete input/select/textarea controls, or provide an equivalently clear typed composition API. Remove required dummy props and duplicated state handlers from callers. Centralize genuinely shared control styling without introducing an oversized generic form framework. Preserve unique label/control IDs and correctly connect hints/errors for native and custom controls. Cover accessible labels and error/hint associations with focused component tests and migrate affected callers.

References: frontend/src/components/FormField.tsx; frontend/src/pages/ProfileEditorPage.tsx; frontend/src/tasks/TaskEditForm.tsx; frontend/src/components/git/MergeForm.tsx. Contract: CLAUDE.md, "Frontend conventions"; SPEC.md, "Frontend".

Merged from the second review (2026-09-21), same finding with the inventory, plus the sibling problem in SubmitButton:
- Scale: 26 of 58 FormField call sites pass `children`; in that mode value, onChange, type, autoComplete, autoFocus, disabled and the computed aria wiring are accepted and ignored. Setters that look live but never fire: pages/project/ProjectSettingsForm.tsx (two sites, which also omit the setSaved(false) the real handler does), LaunchSessionForm.tsx, ProfileEditorPage.tsx (several), TaskEditForm.tsx:189-212, components/git/MergeForm.tsx:111-117 and :147-153, RebaseForm.tsx:99-105, secrets/CreateSecretForm.tsx:112-119. Explicit no-op handlers: CreateTaskForm.tsx:234-241, AcceptInvitePage.tsx.
- Hand-written `aria-describedby={`${name}-hint`}` against FormField's private id convention, none of which include the `-error` id, so an error on a child-rendered field is never announced: TaskEditForm.tsx:183/232/276, CreateTaskForm.tsx:139/222/254, TaskStatesEditor.tsx:293, AcceptInvitePage.tsx. CreateSecretForm's textarea hint is not associated at all.
- Forms that bypass FormField entirely because it adds nothing for non-input controls: tasks/ReviewForm, RevisionForm, LaunchForTask, MergeTaskAction (same hand-rolled label/control/hint block, static DOM ids "review-state", "handoff-commit", "merge-target", "launch-profile"), and the rename/replace forms in secrets/SecretRow.tsx (rename error <p> has no id or aria-describedby).
- Suggested shape: a discriminated union (`{ children: (control: { id; "aria-describedby"; "aria-invalid" }) => ReactNode } | { value; onChange; type?: HTMLInputTypeAttribute; ... }`) or FieldShell + TextField/SelectField/TextAreaField. `type?: string` should be HTMLInputTypeAttribute.
- Control class string: 18 occurrences across 15 files, defined as a constant at least four times and drifting (components/FormField.tsx:73, components/git/formState.ts CONTROL, tasks/taskChrome.ts CONTROL, a local CONTROL in tasks/LaunchForTask.tsx and another in tasks/CommentForm.tsx:21 that silently differs in font and disabled style, plus inline copies in CreateSecretForm, SecretRow, ProfileEditorPage, ProjectSettingsForm, LaunchSessionForm, BaseRefSelect, SecretsPage, TaskStatesEditor). The `FIELD` constant is defined identically in ReviewForm.tsx:44, RevisionForm.tsx:47, MergeTaskAction.tsx:47. Export one from the field module.
- SubmitButton (components/SubmitButton.tsx:9-16) is the app's generic button but `loading` is required (45 of 109 usages pass `loading={false}`) and it does not extend the native button props, so it cannot take aria-expanded, aria-label, title or data-testid. Consequences: the Replace/Rename/Uses disclosure toggles in SecretRow.tsx:228-269 have no aria-expanded; UsersTable.tsx:218 and ProfilesTab.tsx wrap a disabled button in `<span title>`, which is not keyboard reachable. Acceptance addition: `interface ButtonProps extends Omit<ComponentProps<"button">, "className"> { loading?: boolean; variant?: ... }` spreading the rest, SubmitButton kept as the name or an alias, and the dead `loading={false}` props removed.
- Inconsistent optional-prop idiom for `error`: `{...(x === null ? {} : { error: x })}` (ProfileEditorPage, ProjectSettingsForm, LaunchSessionForm) versus `error={x ?? undefined}` elsewhere; exactOptionalPropertyTypes is off, so pick the second.