---
id: "597h9"
title: "Implement resolve_for_launch: scope precedence, orchestrator_only suppression, launch warnings and secret_uses rows"
status: in_progress
priority: P1
created: "2026-09-16T20:31:08.252726664Z"
updated: "2026-09-17T13:07:27.474377908Z"
tags:
  - orchestrator
  - secrets
  - sessions
depends_on:
  - qafug
parent: t36d2
assignee: claude-opus-subagent
attempts: 1
---

## Summary
Deliver `secrets::resolve_for_launch`, the function the session launcher calls to turn a profile's `secrets` name list into the container environment: precedence `global` → `project` → `user` with the last found winning, `orchestrator_only` winners suppressing the name entirely, missing names reported as `launch_warning` text, and one `secret_uses` row per injected secret written in one transaction.

## Documents
- `ARCHITECTURE.md` "Secrets" → "Resolution at launch" (whole paragraph) and "Launch sequence" (`SO->>SEC: resolve profile.secrets for (global, project, user)` / `SEC-->>SO: env map (orchestrator-only excluded); secret_uses rows written`)
- `docs/data-model.md` `secret_uses` ("A row with `purpose = 'launch'` and `session_id` set is written for every injected secret on every launch, including relaunches of parked sessions"), `agent_profiles.secrets` ("Orchestrator-only secrets are never injected even if listed"), `sessions.created_by` ("determines the `user` secret scope"), enum `secret_scope` note
- `SPEC.md` "AgentEvent" `{ kind: "launch_warning"; message: string } // e.g. undeclared secret`
- `ARCHITECTURE.md` "Secrets" → "Credential handling and transcripts" (names only in spans, zeroize)

## Acceptance criteria
- [ ] `pub async fn resolve_for_launch(pool: &PgPool, keyring: &SecretsKeyring, scope: LaunchScope { session_id: Uuid, project_id: Uuid, created_by: Option<Uuid> }, names: &[String]) -> Result<ResolvedSecrets>` with `ResolvedSecrets { env: Vec<(String, Zeroizing<String>)>, warnings: Vec<String>, skipped: Vec<String> }`.
- [ ] For each name the candidates are the rows at `(global, NULL)`, `(project, project_id)` and `(user, created_by)`; the winner is the highest-precedence row present (`user` over `project` over `global`).
- [ ] Winner with `orchestrator_only = true` → the name is absent from `env`, present in `skipped`, and `tracing::info!(secret_name = %name, scope = %winner.scope, "orchestrator-only secret skipped at launch")` is emitted; a lower-precedence non-orchestrator-only row is never used.
- [ ] No row at any scope → `warnings` gets the exact message `secret NAME is not defined at any scope` (name substituted) and resolution continues.
- [ ] Every injected name gets one `secret_uses` row `{ secret_id, session_id: Some(session_id), user_id: created_by, purpose: "launch" }`; all rows are written in one transaction that commits before `Ok` is returned; skipped and missing names write no row.
- [ ] Decryption failure of a winning row → `Error::Internal` after `tracing::error!(secret_name = %name, key_version)`; the transaction is rolled back so there is no partial audit trail and the launcher fails the launch.
- [ ] Duplicate names in the list resolve once; `env` keys are unique; `created_by = None` skips the user scope entirely.

## Implementation notes
- File: `orchestrator/src/secrets/resolve.rs`; re-export from `orchestrator/src/secrets/mod.rs`.
- One `find_for_resolution` query (task 3), then in-memory grouping by name and picking the winner by scope rank (`user = 2`, `project = 1`, `global = 0`); open each winner with `crypto::open` under `aad(row.scope, row.scope_id, &row.name)`; `String::from_utf8` failure → `Internal`.
- The both-`ANTHROPIC_API_KEY`-and-`CLAUDE_CODE_OAUTH_TOKEN` refusal is the launcher's rule (Session lifecycle epic) applied to `env` after this function returns; do not implement it here.
- The `launch_warning` events are appended by the launcher through the event-append primitive; this function only returns the messages, so it never takes the session row lock. Secrets transactions and event transactions stay separate; no lock ordering to observe.

## Edge cases
- A name in the list that fails the name regex (profile validation should prevent it): treat as missing and warn.
- Same name `orchestrator_only` at `global` and plain at `user`: user wins and is injected (precedence first, then the winner's flag).
- Same name plain at `global` and `orchestrator_only` at `project`, no user row: project wins → skipped, the global value does not leak.
- Session whose creating user was deleted (`created_by = NULL`): only global and project scopes participate.
- Empty name list: returns empty `env`, no warnings, writes nothing.

## Testing
- Integration tests in `orchestrator/tests/secrets_resolve.rs` via `TestApp` with user, project, profile and session rows seeded through the repositories and values sealed with `crypto::seal`: precedence (user > project > global) asserting the decrypted value equals the one stored at the winning scope; the two shadowing cases above; missing name → exact warning text and no `secret_uses` row; one `secret_uses` row per injected secret with `purpose = launch`, the session id and `user_id = created_by`; running twice writes rows twice (relaunch); deleted-user case; duplicate names in the list; empty list.
- `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Database schema, models, repositories and test harness": repositories to insert `users`, `projects`, `agent_profiles` and `sessions` rows in tests.
- "Session lifecycle: launcher, owner, recovery and sessions API": calls this function during launch, appends the `launch_warning` events and applies the both-credentials refusal.