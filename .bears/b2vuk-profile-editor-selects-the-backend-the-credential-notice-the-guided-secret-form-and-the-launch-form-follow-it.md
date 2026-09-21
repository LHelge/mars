---
id: b2vuk
title: Profile editor selects the backend; the credential notice, the guided secret form and the launch form follow it
status: open
priority: P2
created: "2026-09-21T12:22:42.596740Z"
updated: "2026-09-21T20:26:15.229474428Z"
tags:
  - frontend
  - profiles
  - secrets
depends_on:
  - tt9d9
  - njr2q
parent: ddb8s
---

## Summary
The backend is a profile-level choice (`agent_profiles.backend`). The API accepts it; the UI has had one value. Make it selectable and make everything that already varies per backend — `GET …/agent-credentials`, `credential_for`, `AgentCredentialNotice`, the guided credential form — show the selected backend's answer.

## Documents
- `SPEC.md`, "Frontend": profile editor, Secrets page guided form, launch form; "User-facing features" if backends are described there. `frontend/tests/README.md` coverage rows belong to the Playwright task.

## Acceptance criteria
- [ ] Invoke `/frontend-design` first (`CLAUDE.md`, "Frontend conventions"): dense operator console, quiet colour for state only.
- [ ] `ProfileEditorPage`: a backend select (`Claude Code`, `Curiosity`); changing it swaps the image placeholder to that backend's default, the model field's hint (`CLI default` vs `provider/model`, e.g. `openrouter/z-ai/glm-5.3-flash` — take the exact example from what Curiosity's README documents), and the credential notice. `partial_messages` help text says what it means for the backend. Editing an existing profile's backend warns that parked sessions of the profile will not resume their conversation (the state directories differ).
- [ ] Secrets page guided form: choose backend → choose credential kind (Claude: subscription token or API key; Curiosity: OpenRouter API key) → value; the name is never typed (ADR 0036). 409 for a second credential of the backend at the scope is shown as today.
- [ ] Launch form and `LaunchForTask`: the pre-launch credential warning uses the profile's backend.
- [ ] Session view: the `init`-less first seconds and an empty `tools` list render sensibly; the session header shows the backend.
- [ ] All API calls through `src/services/`; types in `src/types/` mirror `SPEC.md`; new `data-testid`s are constants in `src/utils/testIds.ts`.

## Implementation notes
- `frontend/src/pages/ProfileEditorPage.tsx`, `src/secrets/{AgentCredentialNotice,agentCredentials,useAgentCredential}.ts(x)`, `src/tasks/LaunchForTask.tsx`, `src/types/{profiles,secrets}.ts`.

## Testing
- Vitest beside the modules (`agentCredentials.test.ts`, `AgentCredentialNotice.test.tsx` extended per backend); `npm run lint && npx tsc -b && npm run build && npm run test:unit`.