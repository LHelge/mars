---
id: vgp23
title: Refuse agent credential names in a profile's secrets list and strip them from existing profiles
status: open
priority: P2
created: "2026-09-20T22:22:25.543786325Z"
updated: "2026-09-20T22:22:25.543786325Z"
tags:
  - orchestrator
  - profiles
  - migration
depends_on:
  - a8bga
parent: rdkmk
---

## Summary
With the credential resolved implicitly, a profile's `secrets` list means "extra things this agent's job needs". Listing a credential there is now redundant and misleading, so `ProfileInput` refuses it and a migration removes the names from existing rows.

## Documents
- `SPEC.md` "Agent profiles" (`ProfileInput`: `secrets` entries must not be an agent credential name; 400 text)
- `docs/data-model.md` `agent_profiles.secrets`
- `ARCHITECTURE.md` "Secrets", Agent credentials
- ADR 0036

## Acceptance criteria
- [ ] `POST`/`PUT /projects/{pid}/profiles` with a `secrets` entry that is an agent credential name of any backend → 400 `<NAME> is an agent credential and is injected automatically`.
- [ ] Migration (`sqlx migrate add -r strip_agent_credentials_from_profiles`) removes `ANTHROPIC_API_KEY` and `CLAUDE_CODE_OAUTH_TOKEN` from every `agent_profiles.secrets` array, leaving order and other entries intact. The `.down.sql` is a documented no-op (`SELECT 1;` with a comment): the removed entries carried no information the new resolver does not already act on, so there is nothing to restore — say so in the file.
- [ ] `AgentProfile::secret_names()` also drops credential names (defence in depth for a column written by something else), next to the existing "drops what cannot be a name" rule and with a test beside that one.
- [ ] `.sqlx/` regenerated if queries changed; both clippy invocations and the test suite pass.

## Implementation notes
- Files: `orchestrator/src/models/agent_profile.rs` (`ProfileError` gains a variant carrying the name; `ProfileInput::resolve` and `validate`), `orchestrator/src/routes/profiles.rs` (error mapping), `orchestrator/migrations/`.
- Use `agent::credential_backend_of` from the first task; do not repeat the names in the model. The migration is the one place they are literal, as in the index migration.
- The check is against *every* backend's names, not just the profile's own backend: a Claude profile has no use for another backend's credential either, and the rule stays simple.

## Edge cases
- The message names the offending entry only; with several, the first in input order.
- Repeated entries are still stored once (existing rule); the refusal comes first.

## Testing
- `tests/profiles.rs`: 400 on create and on replace, message text; a list without credential names still accepted.
- `tests/migrations.rs`: a profile seeded with `{NPM_TOKEN, CLAUDE_CODE_OAUTH_TOKEN, X}` comes out as `{NPM_TOKEN, X}`.
- Unit test for `secret_names()` dropping a credential name.
