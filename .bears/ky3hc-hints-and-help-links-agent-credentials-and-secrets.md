---
id: ky3hc
title: "Hints and help links: agent credentials and secrets"
status: done
priority: P2
created: "2026-09-22T19:08:55.750584553Z"
updated: "2026-09-22T20:56:33.671438086Z"
tags:
  - frontend
depends_on:
  - ujccg
parent: gtbp5
attempts: 1
---

Tighten hints and add `help` links. Invoke `/frontend-design` first. Update tests that assert old strings.

- `secrets/agentCredentials.ts`: the API key kind has no hint — add one (from the Anthropic Console; billed per use).
- `secrets/AddAgentCredentialForm.tsx` "Applies to": hint that your own launches use the most specific match and automatic/scheduled runs need a project or Everyone credential; help → `agent-credentials`. `AgentCredentialsSection.tsx`: help → `agent-credentials`.
- `components/secrets/messages.ts` `PRECEDENCE_HELP` is misleading by omission: a secret reaches a session only when a profile lists its name; then user overrides project overrides global; orchestrator-only never injected. Help → `secrets`.
- `pages/SecretsPage.tsx` scope radio: a hint on what global / project / user scope reach.
- `components/secrets/CreateSecretForm.tsx`: Name — "Environment variable name: A–Z, 0–9 and _, starting with a letter." Orchestrator only — has no hint: "Kept for Mars itself (e.g. git); never injected into a session, even if a profile declares it."
- `components/secrets/SecretRow.tsx` GIT_CREDENTIAL row: "Replace when the token expires; keep it orchestrator-only", help → `git-credential`. Promote the orchestrator-only tooltip-only explanation to visible text where it matters.