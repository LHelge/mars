---
id: a8bga
title: Declare credential names on AgentBackend and resolve the agent credential implicitly at launch
status: open
priority: P1
created: "2026-09-20T22:21:50.092272783Z"
updated: "2026-09-20T22:21:50.092272783Z"
tags:
  - orchestrator
  - agent
  - secrets
parent: rdkmk
---

## Summary
The core of ADR 0036. `AgentBackend` gains `credential_names()`, the launch resolver takes the backend's names beside the profile's and treats them as one slot (the row at the most specific scope wins whatever its name), and the launcher stops refusing "both credentials" because it can no longer resolve both. Also adds the lookup the later tasks share: which backend, if any, a secret name is a credential of.

## Documents
- `ARCHITECTURE.md` "Agent process model" (trait with `credential_names`), "Claude Code invocation" (Credentials), "Secrets" (Resolution at launch, Agent credentials)
- `SPEC.md` "AgentEvent" (`launch_warning`: no agent credential)
- ADR 0036

## Acceptance criteria
- [ ] `AgentBackend::credential_names(&self) -> &'static [CredentialName]` in `orchestrator/src/agent/mod.rs`; the Claude adapter returns `ClaudeCodeOauthToken` and `AnthropicApiKey`; `MockAgentBackend` returns the same two so launcher tests keep their shape.
- [ ] `agent::credential_backend_of(name: &str) -> Option<Backend>` (iterates the backends through `backend_for`), plus `agent::all_credential_names()` if the callers want the flat list. Unit-tested. This is the only place outside the adapters that enumerates credential names.
- [ ] `secrets::resolve::resolve_for_launch` takes `credentials: &[SecretName]` beside `names`. Among rows at `global`, `project(project_id)`, `user(created_by)` carrying any credential name, the most specific scope wins and only that row is decrypted and injected; it appears in `env`, `scopes` and gets its `secret_uses` row like any injected secret. The resolver still does not know which backend the names belong to — update its doc comment, which currently explains the opposite arrangement.
- [ ] A name present in both lists is resolved once, as a credential (defence in depth; the profile rule is a later task).
- [ ] If the winning credential row has `orchestrator_only = true` (only possible for rows older than the write rule) it is skipped and logged exactly like the existing orchestrator-only rule, and no lower-precedence credential leaks through.
- [ ] No credential row at any scope: one `launch_warning` with the message `no agent credential for backend <backend>; add one on the Secrets page`, launch proceeds. The warning text contains no secret name that was not asked for and no value.
- [ ] `session/launcher.rs`: `BOTH_CREDENTIALS_ERROR` and the refusal in `injected_credential` are removed; `injected_credential` still yields the `InjectedCredential { name, scope }` the translator's authentication-failure event needs, now taken from the resolver's credential result rather than by scanning `env` for two hardcoded names.
- [ ] Both clippy invocations and `cargo test --features integration-tests` pass; `cargo sqlx prepare` run and `.sqlx/` committed if any query changed.

## Implementation notes
- Files: `orchestrator/src/agent/mod.rs`, `orchestrator/src/agent/claude/` (adapter), `orchestrator/src/agent/state.rs` (`CredentialName`), the mock backend, `orchestrator/src/secrets/resolve.rs`, `orchestrator/src/session/launcher.rs`.
- `CredentialName::as_str` is already the exact spelling; converting to `SecretName` should go through the one name rule in `models/secret.rs`, not a second regex.
- `rank(scope)` in `resolve.rs` already orders the scopes; the credential slot is "max rank over rows whose name is in the credential list". One query for all wanted names at the three scopes is preferable to one per name.
- `ResolvedSecrets` should expose the winning credential (name + scope) explicitly so the launcher does not re-derive it.

## Edge cases
- The same scope holding both names (rows older than the write rule, before the migration of the sibling task): pick deterministically — `CLAUDE_CODE_OAUTH_TOKEN` loses to nothing at a *more specific* scope, and within one scope prefer the order of `credential_names()` — and log at `warn` with the names only.
- Relaunch of a parked session resolves again, so a credential added after the first failure is picked up (existing behaviour; keep a test for it).
- A session whose `created_by` user was deleted has no user scope; project and global still apply.

## Testing
- `tests/secrets_resolve.rs`: user beats project beats global across different names; empty profile list still injects; none → warning text; orchestrator-only winner is skipped without leak-through; `secret_uses` row written for the credential.
- `tests/session_launcher.rs`: the "both refused" test is replaced by "global API key + user OAuth token launches with the token"; the authentication-failure event still names the injected variable and scope.
- Unit tests for `credential_backend_of`.
