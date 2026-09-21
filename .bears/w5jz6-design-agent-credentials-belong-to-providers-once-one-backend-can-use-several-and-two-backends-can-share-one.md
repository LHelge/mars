---
id: w5jz6
title: "Design: agent credentials belong to providers once one backend can use several and two backends can share one"
status: open
priority: P3
created: "2026-09-21T12:23:43.544277Z"
updated: "2026-09-21T12:23:43.544277Z"
tags:
  - orchestrator
  - secrets
  - design
  - backlog
  - agent-backends
depends_on:
  - tt9d9
---

## Summary
ADR 0036 makes a credential name the property of exactly one backend (`credential_backend_of` maps a name to a single backend; one partial unique index per backend; "the whole list is one slot, exactly one is injected"). Curiosity starts with OpenRouter only, precisely so that this holds. It stops holding the moment Curiosity enables rig's direct Anthropic or OpenAI providers — `ANTHROPIC_API_KEY` is already Claude's — or an `opencode` backend arrives, where the credential needed depends on the `provider/` prefix of the profile's model, not on the backend.

This is a design task: its output is an ADR that supersedes the relevant part of 0036 and the tasks to implement it. No code.

## Questions the ADR answers
- [ ] Is the unit a *provider* (`anthropic`, `openrouter`, `openai`, `github-copilot`) with backends declaring which providers they accept and how each is injected (env name per backend)? What then is "one per scope" — per provider?
- [ ] How the launcher picks: from the profile's model prefix for multi-provider backends; Claude stays single-provider with its two kinds (subscription token, API key) and its precedence rule.
- [ ] `GET /projects/{pid}/agent-credentials`, `credential_for` in `SecretMeta`, the guided form and the pre-launch warning under the new model.
- [ ] Migration of existing rows and indexes; `.down.sql` reversibility.
- [ ] Credentials that are not static values — a ChatGPT/Codex subscription is an `auth.json` with a single-use rotating refresh token the CLI writes back; parallel containers sharing one copy get the grant revoked. In scope only as a stated non-goal or a sketched direction (a per-user writable CLI home mounted into that user's sessions, filled by a device-code login run in a one-off container); not designed here.

## Acceptance criteria
- [ ] ADR written and indexed; `docs/open-questions.md` entry added now if the question should be visible before this task is picked up, and deleted by the ADR.
- [ ] Implementation tasks filed and linked.