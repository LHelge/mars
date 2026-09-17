---
id: xzahq
title: "Implement envelope encryption primitives: seal, open, reseal and rewrap with the row AAD"
status: done
priority: P0
created: "2026-09-16T20:29:42.780753979Z"
updated: "2026-09-17T12:37:33.951322473Z"
tags:
  - orchestrator
  - secrets
  - core
depends_on:
  - "5efvd"
parent: t36d2
assignee: claude-opus-subagent
attempts: 1
---

## Summary
Deliver the envelope-encryption primitives on top of the keyring: `seal` a plaintext value under a fresh per-row data key with the row AAD, `open` it back into a zeroizing buffer, `reseal` under a new AAD with the same data key (rename), and `rewrap` a row's data key under the newest master key without touching the ciphertext (rotation). Also the AAD builder every caller uses so the format `<scope>:<scope_id or empty>:<name>` is defined in exactly one place.

## Documents
- `ARCHITECTURE.md` "Secrets" (diagram: per-row data key, AES-256-GCM, AAD = scope:scope_id:name; "Rotation": ciphertexts untouched; "Credential handling and transcripts": zeroization)
- `docs/data-model.md` `secrets` (ciphertext includes the 16-byte tag; 12-byte nonces; AAD string; "Renaming a secret therefore re-encrypts the value under the new AAD (decrypt, re-encrypt, one transaction); it does not change the data key")
- ADR 0006

## Acceptance criteria
- [ ] `aad(scope: SecretScope, scope_id: Option<Uuid>, name: &str) -> String` yields `global::NAME`, `project:<uuid>:NAME`, `user:<uuid>:NAME` (lowercase hyphenated UUID; scope as the Postgres enum value).
- [ ] `seal(&keyring, aad: &str, plaintext: &[u8]) -> Result<EncryptedSecret>` generates a random 32-byte data key and a random 12-byte nonce, encrypts with AES-256-GCM using the AAD, wraps the data key under `keyring.newest_version()` and returns `EncryptedSecret { ciphertext: Vec<u8>, nonce: Vec<u8>, data_key_wrapped: Vec<u8>, data_key_nonce: Vec<u8>, key_version: i32 }`.
- [ ] `open(&keyring, aad: &str, &EncryptedSecret) -> Result<Zeroizing<Vec<u8>>>` unwraps then decrypts; any failure (wrong AAD, tampered ciphertext, wrong nonce, unknown version, bad lengths) returns `SecretsError::Decrypt` with no detail about which step failed.
- [ ] `reseal(&keyring, old_aad: &str, new_aad: &str, &EncryptedSecret) -> Result<EncryptedSecret>` decrypts under `old_aad` and re-encrypts under `new_aad` with the same data key and a fresh nonce; `data_key_wrapped`, `data_key_nonce`, `key_version` are unchanged.
- [ ] `rewrap(&keyring, &EncryptedSecret) -> Result<EncryptedSecret>` unwraps under the row's `key_version` and wraps under the newest; `ciphertext` and `nonce` are byte-identical; returns `Ok` with the input unchanged when already newest.
- [ ] Plaintext and data-key buffers are `Zeroizing`; `EncryptedSecret` has no derived `Debug` (or a custom one printing lengths and `key_version` only).

## Implementation notes
- File: `orchestrator/src/secrets/crypto.rs`; re-export from `orchestrator/src/secrets/mod.rs`.
- `EncryptedSecret` fields map 1:1 to the `BYTEA`/`INTEGER` columns so the repository (task 3) can bind it straight into `INSERT`/`UPDATE` and build it from a row.
- Use `aes_gcm::{Aes256Gcm, KeyInit, aead::{Aead, Payload}}` with `Payload { msg, aad }`; nonces from `rand::rngs::OsRng`. Never reuse a nonce with the same data key: every `seal`/`reseal` draws a fresh one.
- Empty plaintext is legal at this layer (the model/service rejects empty values on the API).

## Edge cases
- Ciphertext of 16 bytes or fewer (tag only or shorter) → `Decrypt`, no panic.
- Nonce or wrapped key of the wrong length → `Decrypt`, no panic (never `unwrap` the `GenericArray` conversion).
- Values up to 64 KiB must round-trip; no explicit size cap here.

## Testing
- Unit tests in `crypto.rs` with a two-version test keyring: round-trip; `open` with a different name, scope or scope_id in the AAD fails; flipping one ciphertext byte fails; flipping one wrapped-key byte fails; `reseal` under a new AAD opens under the new AAD, fails under the old one, and keeps `data_key_wrapped` equal; `rewrap` from version 1 to 2 keeps `ciphertext`/`nonce` equal, sets `key_version = 2`, and `open` still succeeds; `rewrap` of a newest-version row is a no-op; `open` with an unconfigured version → `Decrypt`; `aad` string form for all three scopes.
- `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Database schema, models, repositories and test harness": `SecretScope` enum in `models/` with a stable string form (`global`, `project`, `user`) matching the Postgres enum values.