---
id: p7emz
title: "Keep the keyring pure: move boot verification against the secrets table out of SecretsKeyring into a startup step that uses the repository"
status: done
priority: P2
created: "2026-09-17T20:03:30.565493879Z"
updated: "2026-09-19T19:33:34.432626182Z"
tags:
  - orchestrator
  - secrets
  - architecture
depends_on:
  - psybz
parent: zeccj
attempts: 1
---

## Summary
`SecretsKeyring::verify_against_db` makes the crypto layer depend on the repository layer, and `KeyVersionSample` exists only to serve that call. Move the verification ("for each `key_version` present in `secrets`, unwrap one row's data key or refuse to start") into `secrets/` as a startup function that takes the pool and the keyring, using the sealed-envelope type from the previous task. The keyring keeps parsing, version selection and wrap/unwrap only.

## Documents
- `ARCHITECTURE.md` "Secrets" → "Keyring" (at start the keyring verifies it can unwrap one row per `key_version` present and refuses to start otherwise).
- `README.md` "Configuration" (`SECRETS_MASTER_KEYS`, `SECRETS_MASTER_KEY_FILE`).

## Acceptance criteria
- [ ] `secrets::verify_keyring_at_startup(pool, &keyring) -> Result<(), SecretsError>` (name free) lives in `secrets/mod.rs` or `secrets/startup.rs`; `main.rs` calls it where it called `verify_against_db`; the error text and log line are unchanged.
- [ ] `keyring.rs` has no `use crate::repositories` and no `sqlx` import; `KeyVersionSample` is gone (the repository returns one `SealedSecret` per distinct version, or the sealed type plus id).
- [ ] `SecretRepository::distinct_key_versions`, `list_meta` and `list_orphans`, which have no `src/` caller, are either used by this step and the reaper task in the background-jobs epic or deleted with a note in that epic's task; do not keep dead queries.

## Implementation notes
- Files: `orchestrator/src/secrets/{keyring,mod}.rs`, `orchestrator/src/main.rs`, `orchestrator/src/repositories/secrets.rs`, `orchestrator/src/models/secret.rs`, `orchestrator/tests/secrets_keyring.rs`.
- The check must still run before any session is adopted (`ARCHITECTURE.md` "Restart procedure").

## Testing
- `tests/secrets_keyring.rs` verification cases move to the new function; they arrange rows through `SealedSecret::seal` + `SecretRepository::insert`.
- `cd orchestrator && cargo fmt && cargo clippy --all-targets -- -D warnings && cargo clippy --all-targets --features integration-tests -- -D warnings && cargo test --features integration-tests` passes.

## Documentation
- None: `ARCHITECTURE.md` describes the behaviour, which is unchanged; check the "Keyring" paragraph does not name the method.