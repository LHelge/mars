# 0006. Envelope encryption in the orchestrator

Status: accepted; the environment variable is `SECRETS_MASTER_KEYS` (plural, versioned entries) as documented in `README.md`, not the singular name used below.

Superseded by [ADR 0027](0027-defer-transcript-secret-redaction.md) for the blanket claims that plaintext secrets never reach Postgres or event payloads: encryption covers managed credential storage, while arbitrary agent/user content is not automatically redacted.

## Context

Secrets (API keys, git credentials, OAuth tokens) must be encrypted at rest. Options considered:

1. `pgcrypto` in Postgres. Rejected: the key travels to the database on every call and appears in query logs and on the wire.
2. One master key encrypting every value directly. Rejected: rotation means re-encrypting every row under load.
3. Envelope encryption in the orchestrator: a per-row data key encrypts the value, the master key only wraps data keys. Chosen.
4. An external KMS or Vault. Best hygiene, but one more service for a self-hosted single-team deployment.

## Decision

Option 3 with RustCrypto `aes-gcm` (AES-256-GCM). Master keys are configured as `<version>=<key>` entries (32 bytes, base64, or a mounted file) so `key_version` selects which one wrapped a row. The AAD is `<scope>:<scope_id>:<name>` so a ciphertext cannot be moved between rows. Rotation re-wraps data keys under the newest master key in batches without touching ciphertexts.

The API never returns a value. Decryption happens only at session launch (for injection) and inside the git credential provider.

## Consequences

- Postgres never sees managed secret plaintext or keys; a database dump is useless without the environment.
- Losing every configured master key loses every secret; `README.md` documents backing up the key separately from the database.
- Adding a KMS later replaces only the wrap/unwrap function.
- Values transit orchestrator memory in plaintext at launch; buffers are zeroized and values are never logged or included in orchestrator-generated events.
