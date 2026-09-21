---
id: tt9d9
title: "agent_backend gains `curiosity`: migration, credential OPENROUTER_API_KEY with its one-per-scope index, and the backend's launch command"
status: open
priority: P2
created: "2026-09-21T12:20:45.814998Z"
updated: "2026-09-21T20:26:15.033767949Z"
tags:
  - orchestrator
  - agent
  - acp
  - migration
  - secrets
depends_on:
  - x8zqq
  - yvb82
  - my6bz
  - yc2ah
  - qwve6
parent: eydgf
---

## Summary
The enum value and everything the compiler and the documents demand when it appears (`agent/mod.rs`: `backend_for` and `exhaustive_backend` stop compiling until an adapter exists), with the parts of the backend that are not protocol: launch command, credential, state directory, policies. The protocol (preamble, encoding, translation) is the next tasks; here they are the minimum that compiles and refuses.

## Documents
- `docs/data-model.md`: `agent_backend` enum row, the new index `secrets_curiosity_credential_idx`, the migration list. `SPEC.md`: `backend` wherever its values are enumerated (profiles, profile templates, `credential_for`, `GET …/agent-credentials`). `ARCHITECTURE.md`: "Agent process model" gains a "Curiosity invocation" section (command line, environment, state directory, policies); "Secrets", Agent credentials. `README.md` if backends are listed.

## Acceptance criteria
- [ ] Migration (`sqlx migrate add -r`): `ALTER TYPE agent_backend ADD VALUE 'curiosity'`; partial unique index `UNIQUE NULLS NOT DISTINCT (scope, scope_id) WHERE name IN ('OPENROUTER_API_KEY')`. The `.down.sql` drops the index and documents that an enum value cannot be removed (the repository's rule: values are only added); follow how `CLAUDE.md`, "Migrations" wants that stated. `cargo sqlx prepare`, `.sqlx/` committed.
- [ ] `models::AgentBackend::Curiosity` (`"curiosity"`), `BACKENDS`, `CredentialName::OpenrouterApiKey`; the test that asserts index predicate and `credential_names` agree covers the new index.
- [ ] `agent/acp/` module skeleton and `CuriosityBackend`: `launch_command` = `curiosity acp [--model <profile.model>] [--system-prompt <profile.system_prompt>]`; `state_dir` = `curiosity` / `CURIOSITY_HOME`; `mid_turn_input` = `Hold`; `cost_accounting` = per turn; MCP delivery `InProtocol`; `credential_names` = `[OPENROUTER_API_KEY]`.
- [ ] Ephemeral launches: ACP has no prompt-in-argv. Decide with the spike's findings and document in "Curiosity invocation": either the adapter's preamble carries the prompt after the handshake and the launcher attaches a stdin writer for an ephemeral ACP session (ADR 0034 says today it attaches none — that ADR's text is then amended), or `curiosity acp --prompt` runs handshake-free. The first keeps one protocol for every ACP agent and is preferred.
- [ ] Secrets write path answers 409 for a second Curiosity credential at a scope and reports `credential_for: "curiosity"`; `ProfileInput` refuses `OPENROUTER_API_KEY` in `secrets`; `GET /projects/{pid}/agent-credentials` lists the backend. Tests for each, per `CLAUDE.md` "Testing expectations".
- [ ] Profile templates stay Claude (`backend: "claude"`); nothing seeds a Curiosity profile.
- [ ] `frontend/src/types/profiles.ts` and `types/secrets.ts` mirror the new value (type only; UI is the end-to-end epic).

## Implementation notes
- `orchestrator/src/models/agent_profile.rs` (~l.121), `agent/{mod,state}.rs`, `secrets/service.rs`, `routes/{secrets,profiles}.rs`, `models/secret.rs`.
- `partial_messages` has no ACP equivalent (updates always stream): document that the flag is ignored for this backend, or that the adapter coalesces chunks into whole `text` events when it is false — prefer the latter, it is what the flag means.

## Testing
- `tests/secrets*.rs`, `tests/profiles.rs`, agent unit tests; both clippy invocations and the full nextest suite.