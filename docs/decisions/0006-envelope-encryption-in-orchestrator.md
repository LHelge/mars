# 0006. Envelope encryption in the orchestrator

Status: accepted

## Context

Secrets (API keys, git credentials, OAuth tokens) must be stored encrypted at rest. Options:

1. `pgcrypto` in Postgres: simple SQL, but the key is sent to the database on every call and appears in query logs and on the wire.
2. A single master key that directly encrypts every value in the orchestrator: fine until the key must be rotated, which then means re-encrypting every row under load.
3. Envelope encryption in the orchestrator: a per-row data key encrypts the value; the master key only wraps data keys.
4. An external KMS or Vault: best key hygiene, one more service to run for a self-hosted single-team deployment.

## Decision

Option 3, with RustCrypto `aes-gcm` (AES-256-GCM). The master key comes from `SECRETS_MASTER_KEY` (32 bytes, base64) or a mounted file `SECRETS_MASTER_KEY_FILE`; several may be configured as `<version>=<key>` so that `key_version` selects which one wrapped a row. The AAD for the value is `<scope>:<scope_id>:<name>` so a ciphertext cannot be moved between rows. Rotation re-wraps data keys under the newest master key in batches without touching ciphertexts.

The API never returns a value. Decryption happens only at session launch (for injection) and inside the git credential provider.

## Consequences

- Postgres never sees plaintext or keys; a database dump is useless without the environment.
- Losing every configured master key loses every secret; the README documents backing up the key separately from the database.
- Adding a KMS later means replacing the "wrap/unwrap data key" function only.
- Values transit the orchestrator's memory in plaintext at launch; `zeroize` is used on buffers and values are never logged or included in events.
