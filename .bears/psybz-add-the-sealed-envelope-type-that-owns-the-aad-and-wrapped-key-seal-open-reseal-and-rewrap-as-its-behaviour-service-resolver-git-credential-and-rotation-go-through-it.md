---
id: psybz
title: "Add the sealed-envelope type that owns the AAD and wrapped key: seal, open, reseal and rewrap as its behaviour; service, resolver, git credential and rotation go through it"
status: in_progress
priority: P2
created: "2026-09-17T20:01:32.567263006Z"
updated: "2026-09-19T11:42:58.671215769Z"
tags:
  - orchestrator
  - secrets
  - architecture
  - docs
depends_on:
  - "48ke5"
  - trxaf
parent: zeccj
attempts: 1
---

## Summary
Introduce one type in `secrets/` that is a row's encrypted identity: the AAD inputs (`scope`, `scope_id`, `name`) and the envelope (`ciphertext`, `nonce`, `data_key_wrapped`, `data_key_nonce`, `key_version`). Sealing a value, opening it, resealing under a new identity (rename) and rewrapping under the newest master key are methods on it. The service, the launch resolver, the git credential and rotation stop assembling AAD strings and passing envelope columns around loose, and the four envelope shapes collapse to this one plus the row.

## Documents
- `ARCHITECTURE.md` "Secrets" (envelope diagram: `AAD = scope:scope_id:name`; "Rotation": re-wrap the data key, never the ciphertext).
- `docs/data-model.md` `secrets` (columns, `key_version`, the AAD binding).
- ADRs 0002, 0006.

## Acceptance criteria
- [ ] `orchestrator/src/secrets/envelope.rs` defines `SecretIdentity { scope: SecretScope, scope_id: Option<Uuid>, name: SecretName }` (or reuses `ScopeRef` + `SecretName`) and `SealedSecret { identity, ciphertext, nonce, wrapped: WrappedKey }` with `SealedSecret::seal(&SecretsKeyring, identity, &[u8]) -> Result<SealedSecret>`, `open(&self, &SecretsKeyring) -> Result<Zeroizing<Vec<u8>>>`, `reseal(self, &SecretsKeyring, new_identity) -> Result<SealedSecret>`, `rewrap(&self, &SecretsKeyring) -> Result<Option<SealedSecret>>` (`None` when already at the newest version; `UnknownKeyVersion` preserved as an error, not flattened).
- [ ] The AAD is computed only inside `envelope.rs`; `crypto::aad`, `crypto::aad_for`, `crypto::seal/open/reseal/rewrap` are removed or made private helpers of the envelope; `grep -rn "aad" orchestrator/src` matches only `envelope.rs` and its tests.
- [ ] `Secret::encrypted_value`, `EncryptedValue` and `KeyVersionSample` in `models/secret.rs` are replaced by `Secret::sealed(&self) -> SealedSecret` (or `From<&Secret>`), and `SecretRepository::rewrap` takes `&SealedSecret` plus the expected version instead of six loose arguments; `insert` takes a `SealedSecret` and the metadata.
- [ ] `rotation.rs::rewrap_data_key` is deleted; the sweep calls `SealedSecret::rewrap`. `ROTATION_BATCH` moves to `rotation.rs` (the policy owner) and the repository takes the batch size as a parameter.
- [ ] `service.rs` create and rename, `resolve.rs`, `git_credential.rs` no longer mention `data_key_*` or build AAD strings.
- [ ] The empty-plaintext rule is stated once: `validate_secret_value` rejects it at the model; the envelope does not care.
- [ ] `cargo sqlx prepare` run and `.sqlx/` committed.

## Implementation notes
- Files: `orchestrator/src/secrets/{envelope,crypto,keyring,service,resolve,rotation,git_credential,mod}.rs`, `orchestrator/src/models/secret.rs`, `orchestrator/src/repositories/secrets.rs`.
- `WrappedKey` (keyring) stays as the wrapped-key triple; the envelope composes it. The keyring keeps `wrap_data_key`/`unwrap_data_key`; only the envelope calls them.
- Rename is the only operation that opens and reseals; `reseal` consumes `self` so the old identity cannot be reused by mistake.
- Keep the `FOR UPDATE` interlock between rename and rotation in the service and the optimistic `expected_key_version` in the repository; this task changes what is passed, not the locking.

## Edge cases
- A row whose `key_version` is unknown to the keyring: `open` fails with `UnknownKeyVersion` (today `open` flattens everything to `Decrypt`); the resolver reports it as a launch failure for that name, not as "wrong AAD".
- `rewrap` on a row already at the newest version returns `None` so the sweep counts it as skipped, matching the current `Ok(false)` protocol.

## Testing
- Move the `crypto.rs` unit tests onto `SealedSecret`: seal/open round trip, wrong identity fails, reseal changes the AAD and the ciphertext, rewrap changes only the wrapped triple, unknown version surfaces.
- `tests/secrets_service.rs`, `tests/secrets_resolve.rs`, `tests/secrets_keyring.rs` and `tests/secrets_rotation.rs` stop importing `aad`, `aad_for`, `open`, `seal` and stop hand-assembling `EncryptedValue`; they arrange rows through `SecretsService` or `SealedSecret::seal` + `SecretRepository::insert`.
- `cd orchestrator && cargo fmt && cargo clippy --all-targets -- -D warnings && cargo clippy --all-targets --features integration-tests -- -D warnings && cargo test --features integration-tests` passes.

## Documentation
- `ARCHITECTURE.md` "Secrets": under the envelope diagram, one paragraph naming the sealed-envelope type as the only place the AAD is assembled and the wrapped key is handled, and that rotation is its `rewrap`.
- `docs/data-model.md` `secrets`: no column change; if the prose names `EncryptedValue`, rename it.

## Assumes from other epics
- Nothing new; secrets epic (`t36d2`) is done.