---
id: a48hj
title: "Touch controls: 16 px text inputs and 44 px hit areas on a coarse pointer"
status: open
priority: P1
created: "2026-09-23T15:58:17.983116687Z"
updated: "2026-09-23T15:58:17.983116687Z"
tags:
  - frontend
  - mobile
depends_on:
  - cdkxp
parent: yymt7
---

## Summary

Every text control in the app is 12–14 px, so iOS zooms the page on every focus, and the chip buttons, icon-only buttons, unpadded `text-xs` links and 14 px checkboxes are 14–24 px targets. This task applies the pointer rule of `SPEC.md`, "Frontend", "Mobile layout" — 16 px text controls and 44 px hit areas on `pointer-coarse:`, unchanged density on a fine pointer — through the shared style constants first and the stragglers second.

Implements `SPEC.md`, "Frontend", "Mobile layout" (the pointer rule).

## Acceptance Criteria

- [ ] `CONTROL` in `src/components/fieldStyles.ts:15` carries `pointer-coarse:text-base` (16 px), so every `FieldShell`/`FormField` control, select and textarea follows; the same for `COMMENT_CONTROL` (`src/tasks/CommentForm.tsx:21`), the composer textarea (`src/session/Composer.tsx:185`), the title input (`src/session/SessionHeader.tsx:363`) and the `text-xs` controls: `SessionActions.tsx:280`, `StateRow.tsx:163`, `SELECT_CLASS` in `SecretsPage.tsx:48` and `AddAgentCredentialForm.tsx:29`, `ChangesPanel.tsx:80`, `CopyLinkButton.tsx:47`, `HandoffPanel.tsx:318`. Where one of those has its own class string, it appends to `CONTROL` instead (the file's own rule).
- [ ] One `TAP` constant in `fieldStyles.ts` (`pointer-coarse:min-h-11 pointer-coarse:min-w-11`, or the equivalent that keeps inline chips from breaking their line — a negative-margin pseudo-element hit area is acceptable if documented in the constant's comment) applied to: `SubmitButton` (`:57`), `CHIP` buttons (`src/tasks/taskChrome.ts:35`; used by `MergeTaskAction.tsx:80`, `ViewDiffButton.tsx:21`, `HandoffPanel.tsx:300`, `TaskCard.tsx:139`), the icon-only `p-1` buttons (`SidePanel.tsx:91,145`, `Alert.tsx:35`), the unpadded `text-xs` buttons and links (`SessionHeader.tsx:102,112`, `SessionBranchTable.tsx:186`, `DiffView.tsx:161`, `HandoffDiff.tsx:61`, `CollapsibleLines.tsx:76`, `ChangesPanel.tsx:134`, `UserMessage.tsx:46`, `JsonTree.tsx:46,110,131`), `CodeBlock.tsx:132`, `CopyLinkButton.tsx:33`, `Transcript.tsx:230,275`, and the nav links of `PageLayout.tsx:39` and tabs of `ProjectTabs.tsx:16` and `SidePanel.tsx:135`.
- [ ] Checkboxes and radios (`SecretsPage.tsx:186`, `SecretRow.tsx:163`, `CreateSecretForm.tsx:120`, `ProjectSettingsForm.tsx:222`, `AddAgentCredentialForm.tsx:112,149`, `CHECK_CLASS` in `profileForm.ts:55`) get one shared class in `fieldStyles.ts` with `pointer-coarse:size-5` and a label that is the hit area (the `<label>` wraps the control or has `min-h-11` on coarse pointers).
- [ ] The desktop rendering is pixel-identical: no class change outside a `pointer-coarse:` variant except where a copied class string is replaced by the constant it should have been.
- [ ] `SPEC.md`, "Frontend", "Mobile layout" names `CONTROL`, `TAP` and the checkbox constant as where the rule lives; `CLAUDE.md`, "Frontend conventions" adds `TAP` to the `CONTROL`/`FIELD` sentence.

## Implementation Notes

- Tailwind 4.3 ships `pointer-coarse:` (and `any-pointer-coarse:`); use `pointer-coarse:` so a laptop with a touch screen and a trackpad keeps its density.
- `min-h-11` on an inline chip changes the line height of the card it sits in; prefer `relative` + `after:absolute after:-inset-2` style extension for chips, with the class string in the constant so nothing copies it.
- Start from `grep -rn 'p-1"\|py-px\|py-0.5\|text-\[0\.' src --include=*.tsx | grep -v test` to find what the list above missed.

## Edge Cases

- A 16 px control in a `FieldShell` with a hint below: the hint stays `text-xs`; only the control changes.
- xterm's own font (`TerminalView.tsx:39`) is not a text input and stays at 12.

## Testing

- Vitest: a test on `fieldStyles.ts` that `CONTROL` and `TAP` contain the `pointer-coarse:` classes (guards a future copy-paste that drops them).
- Playwright `@mobile`: `settings.spec.ts` › `the password form is usable on a phone @mobile` — focus a field and assert its computed font size is 16 px, and every button's bounding box is at least 44 px tall. Add the row to `frontend/tests/README.md`.
- Frontend chain: `npm run lint && npx tsc -b && npm run build && npm run test:unit && npm run test:e2e:up && npm run test:e2e; npm run test:e2e:down`.
- Invoke the `/frontend-design` skill first.