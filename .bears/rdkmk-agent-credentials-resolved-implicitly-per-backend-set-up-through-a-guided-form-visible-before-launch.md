---
id: rdkmk
title: "Agent credentials: resolved implicitly per backend, set up through a guided form, visible before launch"
type: epic
status: done
priority: P1
created: "2026-09-20T22:21:26.309604049Z"
updated: "2026-09-21T07:36:38.694760200Z"
tags:
  - secrets
  - agent
  - orchestrator
  - frontend
---

## Scope
Make authenticating agents one step in one place (ADR 0036). Today the user must know the name `CLAUDE_CODE_OAUTH_TOKEN` / `ANTHROPIC_API_KEY`, store it as a secret and add it to every profile's `secrets`; a mistake shows up as a `launch_warning`, a 401 and a parked session. After this epic the backend declares its credential names, the launcher resolves them for every session (most specific scope wins, one credential per scope per backend), the Secrets page has a guided `Agent credentials` form, and the profile editor and launch form say which credential a launch would use before anything is launched.

The documents were written first and are already on `main`; every task implements a section of them and changes them only where implementation proves them wrong.

## Documents
- `docs/decisions/0036-agent-credentials-belong-to-the-backend.md`
- `ARCHITECTURE.md` "Agent process model" (`credential_names` on the trait), "Claude Code invocation" (Credentials), "Secrets" (Resolution at launch, Agent credentials)
- `SPEC.md` "Secrets" (agent-credential rules, `credential_for`, `GET /projects/{pid}/agent-credentials`), "Agent profiles" (`ProfileInput.secrets`), "AgentEvent" (`launch_warning`), "Frontend" (Agent credentials)
- `docs/data-model.md` `secrets` (`secrets_claude_credential_idx`), `agent_profiles.secrets`
- `README.md` "Prerequisites", "Start" (first-run walkthrough)

## Acceptance criteria
- [ ] A Claude session whose profile has an empty `secrets` list is launched with the launching user's credential, else the project's, else the global one, whichever of the two names it carries; exactly one is injected and `secret_uses` records it.
- [ ] The launch-time refusal of "both credentials" is gone; the conflict is a 409 where the second credential of a scope is written.
- [ ] A profile cannot list an agent credential name; existing lists are cleaned by migration.
- [ ] `GET /projects/{pid}/agent-credentials` tells the caller which credential a launch would use without decrypting anything.
- [ ] The Secrets page creates a credential without the name being typed; the profile editor and launch form show the resolved credential or a warning with a link.
- [ ] Both quality chains of `CLAUDE.md` pass, including Playwright.
