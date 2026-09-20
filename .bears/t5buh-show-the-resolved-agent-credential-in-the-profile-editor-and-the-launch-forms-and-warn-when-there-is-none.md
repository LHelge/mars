---
id: t5buh
title: Show the resolved agent credential in the profile editor and the launch forms, and warn when there is none
status: open
priority: P1
created: "2026-09-20T22:23:20.477123027Z"
updated: "2026-09-20T22:23:20.477123027Z"
tags:
  - frontend
  - secrets
  - sessions
depends_on:
  - khe7q
  - vgp23
  - hy6c8
parent: rdkmk
---

## Summary
Close the loop: wherever a user is about to launch or is configuring a profile, the UI says which credential the session will authenticate with — and whose — or that there is none, with a link to fix it. This replaces finding out from a parked session.

## Documents
- `SPEC.md` "Frontend", "Agent credentials" (second paragraph: `AgentCredentialNotice`, query key, wording, `Add credential` / `Launch anyway`)
- `SPEC.md` "Secrets" (`GET /projects/{pid}/agent-credentials`, `AgentCredentialStatus`); "Agent profiles" (the 400 for a credential name in `secrets`)
- ADR 0036 (why a missing credential warns rather than blocks)
- `CLAUDE.md` "Frontend conventions" — invoke `/frontend-design` first

## Acceptance criteria
- [ ] `frontend/src/types/` gains `AgentCredentialStatus`; `services/secrets.ts` gains `getAgentCredentials(pid)`; a `useAgentCredential(pid, backend)` hook with query key `["projects", pid, "agent-credentials"]`, invalidated by every secrets mutation (create, replace, rename, delete).
- [ ] `frontend/src/secrets/AgentCredentialNotice.tsx`: for a status entry renders `Authenticates with your <label>` (scope `user`), `… the project's <label>` (`project`), `… the shared <label>` (`global`), using `labelForCredential`; for `null` renders, in the warning state colour, `No agent credential: sessions of this profile will fail to authenticate` with a link to `/secrets`. Loading and error states are quiet (no layout jump, no blocking).
- [ ] `ProfileEditorPage`: the notice sits read-only beside the secrets field for the profile's `backend`; the secrets picker (`mergeSecretOptions`) leaves out names whose `credential_for` is set, and a 400 from the server for a typed credential name is shown on the field.
- [ ] `LaunchSessionForm` and `tasks/LaunchForTask`: the notice for the selected profile's backend. With no credential the primary button becomes `Add credential` (navigates to `/secrets`) and a secondary `Launch anyway` performs the launch; with a credential the form behaves as today.
- [ ] The full frontend chain passes, including Playwright; lazily used components are imported by path, not through the components barrel.

## Implementation notes
- Files: `frontend/src/secrets/AgentCredentialNotice.tsx`, `frontend/src/secrets/useAgentCredential.ts`, `frontend/src/pages/ProfileEditorPage.tsx`, `frontend/src/pages/project/LaunchSessionForm.tsx`, `frontend/src/tasks/LaunchForTask.tsx`, `frontend/src/services/secrets.ts`, `frontend/src/types/`.
- The E2E suite launches sessions on the stub image with no credential. Those tests must keep working: either click `Launch anyway` in the shared helper in `tests/utils/test-helpers.ts`, or have the helper create a fake user-scoped credential first. Prefer the helper creating a fake credential (`fake-oauth-token-for-tests`) so the common path is what is exercised, and keep one test for `Launch anyway`.
- Colour is reserved for state: only the missing-credential notice uses the warning colour.

## Edge cases
- The answer is per caller: a teammate opening the same profile sees their own credential. Nothing about it is stored on the profile.
- The credential is deleted between rendering and launching: the server launches with a `launch_warning`; the session view already shows it. No extra handling.
- Several backends in the response: pick the entry whose `backend` equals the profile's; an unknown backend renders nothing.

## Testing
- Vitest for the notice's wording per scope and for the picker filter.
- Playwright: with no credential the launch form shows the warning and `Add credential`; after adding one through the Secrets page the form reads `Authenticates with your Claude subscription token`; `Launch anyway` still launches a stub session.
