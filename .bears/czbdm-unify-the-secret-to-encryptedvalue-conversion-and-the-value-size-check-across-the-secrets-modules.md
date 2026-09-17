---
id: czbdm
title: Unify the Secret-to-EncryptedValue conversion and the value-size check across the secrets modules
status: in_progress
priority: P2
created: "2026-09-17T13:46:23.578765519Z"
updated: "2026-09-17T13:57:08.346830555Z"
tags:
  - orchestrator
  - secrets
  - refactor
depends_on:
  - a2cku
  - cad3v
  - "597h9"
parent: t36d2
assignee: claude-opus-subagent
attempts: 1
---

## Summary
Wave 4 of the secrets manager epic ran four tasks in parallel and each needed the same two helpers, so they were written three times. Move them to their one natural home and delete the copies. Pure refactor: no behaviour, endpoint, schema or document change.

## Documents
- `CLAUDE.md` "Backend conventions" (models hold validation; one place per rule)
- `ARCHITECTURE.md` "Secrets" (no change expected; read for context)

## Acceptance criteria
- [ ] `models/secret.rs` gains one conversion from a stored row to its crypto columns: either `impl From<&Secret> for EncryptedValue` or `impl Secret { pub fn encrypted_value(&self) -> EncryptedValue }` (pick one, document why in the doc comment).
- [ ] The private copies are removed: `value_of` in `src/secrets/resolve.rs`, `encrypted_value_of` in `src/secrets/git_credential.rs`, `encrypted_of` in `src/secrets/service.rs`; each call site uses the model conversion.
- [ ] `src/secrets/git_credential.rs` drops its private `MAX_VALUE_BYTES` and validates with `models::validate_secret_value` (added by the service task); the two `BadRequest` messages become the `SecretError::InvalidValue` mapping, so a git credential and an API value are rejected with the same 400 text. Update `tests/secrets_git_credential.rs` assertions if they match on the old message text.
- [ ] No other file changes; `cargo fmt`, both clippy invocations and `cargo test --features integration-tests` stay clean with the same test counts.

## Implementation notes
- Three source files plus `models/secret.rs`; at most one test file for the message assertion.
- Do not touch `.sqlx/` or introduce queries.

## Testing
- Existing suites: `secrets_resolve`, `secrets_git_credential`, `secrets_service`, `secrets_repository`, the lib unit tests. Add one unit test for the conversion in `models/secret.rs`.

## Documentation
- none: internal refactor, no contract changes.