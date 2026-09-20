---
id: hy6c8
title: "Secrets page: Agent credentials section with a guided form that never asks for the name"
status: open
priority: P1
created: "2026-09-20T22:22:58.800817242Z"
updated: "2026-09-20T22:22:58.800817242Z"
tags:
  - frontend
  - secrets
depends_on:
  - sk5n8
parent: rdkmk
---

## Summary
The one place a user sets up how agents authenticate. `SecretsPage` opens with an `Agent credentials` section listing credentials under human labels, and an `Add agent credential` form of three fields — kind, value, applies to — that writes an ordinary secret through `POST /secrets`.

## Documents
- `SPEC.md` "Frontend", "Agent credentials" (first paragraph); "Secrets" (`SecretMeta.credential_for`, the 409/400 strings)
- `SPEC.md` product overview, "Agent credentials"
- ADR 0036
- `CLAUDE.md` "Frontend conventions" — invoke the `/frontend-design` skill before shaping the section

## Acceptance criteria
- [ ] `frontend/src/types/` `SecretMeta` gains `credential_for: "claude" | null`, mirroring `SPEC.md`.
- [ ] `frontend/src/secrets/agentCredentials.ts`: the constant table per backend — `{ name, label, hint }` for `CLAUDE_CODE_OAUTH_TOKEN` (`Claude subscription token`; hint: printed by `claude setup-token`, needs a Pro or Max subscription) and `ANTHROPIC_API_KEY` (`Anthropic API key`) — and `labelForCredential(name)`. The only file in the frontend that spells the names; unit-tested.
- [ ] `Agent credentials` section at the top of `SecretsPage`: rows where `credential_for !== null`, showing label, `Applies to` (`You`, the project's name, `Everyone`), last use, and the existing replace-value and delete actions. The general list below filters those rows out. `EmptyState` when there are none, with the sentence that sessions cannot authenticate without one.
- [ ] `Add agent credential` form (`useFormSubmit`, `FormField`, `SubmitButton`, `Alert`): kind (radio, subscription token first), value (password-type input, never echoed back), `Applies to`: `Me` (default) / `A project` (select of the user's projects) / `Everyone`. Submits `POST /secrets` with the name from the table, `scope` and `scope_id`. No `orchestrator_only` control.
- [ ] A 409 shows the body's `error` verbatim (it names the credential already at that scope); success clears the value field and invalidates the secrets queries and `["projects", *, "agent-credentials"]`.
- [ ] All calls through `src/services/secrets.ts`; no `fetch` in components.
- [ ] `npm run lint && npx tsc -b && npm run build && npm run test:unit && npm run test:e2e` pass; the `/secrets` chunk stays lazy and nothing new enters the components barrel that is only used lazily.

## Implementation notes
- Files: `frontend/src/pages/SecretsPage.tsx`, new `frontend/src/secrets/` (`agentCredentials.ts`, `AgentCredentialsSection.tsx`, `AddAgentCredentialForm.tsx`), `frontend/src/types/`, `frontend/src/services/secrets.ts`.
- Tone: dense operator console; the warning colour is reserved for the empty state, the rest is quiet.
- Import the section by path from `SecretsPage` rather than through a barrel, as with `DiffBody`.

## Edge cases
- The general create form can still create a credential by typing its name (same API, same rules); after creation it simply appears in the top section.
- A user with no projects: the `A project` option is disabled with a reason.
- An admin viewing other users' user-scoped credentials: `Applies to` shows the username, not `You`.

## Testing
- Vitest: the table/label helper; the form's mapping from `Applies to` to `scope`/`scope_id`.
- `SecretsPage.test.tsx`: credentials render in the top section and not in the general list.
- Playwright: add a subscription token for `Me` with an obviously fake value (`fake-oauth-token-for-tests`), see it listed as `Claude subscription token`; adding an API key for `Me` afterwards shows the 409 message.
