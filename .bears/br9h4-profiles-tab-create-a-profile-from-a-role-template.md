---
id: br9h4
title: "Profiles tab: create a profile from a role template"
status: done
priority: P2
created: "2026-09-20T22:41:21.018579583Z"
updated: "2026-09-21T09:45:36.949390886Z"
tags:
  - frontend
  - profiles
depends_on:
  - "7sq7f"
parent: pekcb
attempts: 1
---

## Summary
`New profile` offers `Start from: Blank / planner / implementer / reviewer / merger`. Choosing a template pre-fills the editor — name, served states, tool allow-list, system prompt — and the user saves it as an ordinary profile. Works in any project, so existing projects and deleted roles are covered.

## Documents
- `SPEC.md` "Frontend": a short "Role templates" paragraph (written by this task); "Agent profiles" (`GET /profile-templates`, `ProfileTemplate`)
- `CLAUDE.md` "Frontend conventions" — invoke `/frontend-design` before shaping the control

## Acceptance criteria
- [ ] `frontend/src/types/` gains `ProfileTemplate` mirroring `SPEC.md`; `services/profiles.ts` gains `listProfileTemplates()`; TanStack Query key `["profile-templates"]` with a long `staleTime` (the list is constant per build).
- [ ] The new-profile flow (`ProfilesTab` → `ProfileEditorPage`) has a `Start from` select. Picking a template fills the form through the existing `profileForm` helpers; picking `Blank` restores the defaults. Changing the selection after the user edited a field asks before overwriting.
- [ ] If the project already has a profile with the template's name, the name is pre-filled as `<name>-2` (next free suffix), since names are what the user sees in the launch form.
- [ ] Served states of the template that are not queue states of this project are dropped from the pre-fill, with an inline note naming them, instead of letting the save fail with a 400.
- [ ] Editing an existing profile shows no template control.
- [ ] The full frontend chain passes, including Playwright; nothing lazy-only is added to the components barrel.

## Implementation notes
- Files: `frontend/src/pages/ProfileEditorPage.tsx`, `frontend/src/pages/project/profileForm.ts` (+ its test), `frontend/src/pages/project/ProfilesTab.tsx`, `frontend/src/services/profiles.ts`, `frontend/src/types/`.
- The system prompt field should be comfortable for a prompt of this length: monospace, tall enough to read (operator-console tone; no colour).
- If epic `rdkmk` (agent credentials) has landed, the secrets field and `AgentCredentialNotice` are unaffected; templates carry no secrets.

## Edge cases
- Template fetch fails: the select shows only `Blank` and the page works as today.
- `is_default` from the template is never applied by this flow.

## Testing
- Vitest: `profileForm` pre-fill from a template, name suffixing, dropping unknown states.
- Playwright: in a fresh project delete nothing, create a profile from `reviewer`, see it saved as `reviewer-2` with the prompt; a fresh project's Profiles tab lists the four seeded roles with `implementer` marked default.
