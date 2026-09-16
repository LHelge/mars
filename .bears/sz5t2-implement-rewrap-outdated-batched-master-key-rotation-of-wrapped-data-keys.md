---
id: sz5t2
title: "Implement rewrap_outdated: batched master-key rotation of wrapped data keys"
status: open
priority: P1
created: "2026-09-16T20:31:43.123490352Z"
updated: "2026-09-16T20:31:43.123490352Z"
tags:
  - orchestrator
  - secrets
  - cron
depends_on:
  - qafug
parent: t36d2
---

## Summary
Deliver `secrets::rotation::rewrap_outdated`, the batch re-wrap routine that moves every row whose `key_version` is behind the newest configured key onto the newest key without touching ciphertexts. It is the single implementation invoked by both the `rotate-secrets` subcommand (next task) and the hourly cron job (Background jobs epic).

## Documents
- `ARCHITECTURE.md` "Secrets" → "Rotation" ("selects rows with `key_version < newest` in batches of 100, unwraps and re-wraps each data key under the newest master key, and updates `data_key_wrapped`, `data_key_nonce`, `key_version` in one statement per row. Ciphertexts are untouched. Once no row references the old version, it can be removed from the environment.")
- `ARCHITECTURE.md` "Background jobs" row "secret rotation | 1 h | Re-wrap rows whose `key_version` is behind the newest key, if any."
- `README.md` "Operating notes" rotation bullet ("remove the old entry once `GET /api/secrets` shows no row on the old version")
- `docs/data-model.md` `secrets` index `secrets_key_version_idx`

## Acceptance criteria
- [ ] `pub async fn rewrap_outdated(pool: &PgPool, keyring: &SecretsKeyring) -> Result<RotationReport>` with `RotationReport { rewrapped: u64, skipped: u64, remaining: u64 }`.
- [ ] Loops over `select_rotation_batch(newest, 100)` until the batch is empty; per row: `keyring.unwrap_data_key(wrapped, nonce, old_version)` → `keyring.wrap_data_key(...)` under newest → `update_wrap(id, old_version, ...)`; a `false` result (row changed concurrently) counts as `skipped`.
- [ ] A row whose `key_version` has no configured key counts as `skipped`, is logged once per version at `error` (`key_version = v`), and does not abort the sweep; `remaining` is `count_below_version(newest)` at the end.
- [ ] `ciphertext` and `nonce` are never read or written by the sweep.
- [ ] Outcome logged at `info` with the three counters; no key material or values in logs.
- [ ] Idempotent: a second run reports `rewrapped = 0`; a keyring with one version returns immediately with zeros.

## Implementation notes
- File: `orchestrator/src/secrets/rotation.rs`; re-export from `orchestrator/src/secrets/mod.rs`.
- No long transaction: each row is one autocommit `UPDATE ... WHERE id = $1 AND key_version = $2`, so the sweep never blocks the API and the API blocks the sweep for at most one row. Unwrapped data keys are `Zeroizing` and dropped per row.
- Loop termination guard: stop when a batch yields zero `rewrapped` rows (every row skipped), otherwise rows under an unknown version would be re-selected forever.
- Rename (service task) keeps the data key, so it commutes with rotation; replace seals under newest, so its row leaves the sweep's set; the version guard handles either interleaving.

## Edge cases
- Rows under a version higher than `newest` (operator removed the newest key): not selected, but startup verification already refused to boot in that case.
- Batch boundaries: exactly 100, 101 and 250 rows must all end on the newest version.
- Pool exhaustion or a transient database error mid-sweep: return `Err`; the partial progress is committed and the next run continues.

## Testing
- Integration test in `orchestrator/tests/secrets_rotation.rs` via `TestApp`: build a keyring with version 1 only, seal 250 rows through the repository, build a keyring with versions 1 and 2, run `rewrap_outdated`, assert `rewrapped = 250`, every row has `key_version = 2`, every `ciphertext` / `nonce` is byte-identical to before, and every value still opens with `crypto::open`; second run `rewrapped = 0`, `remaining = 0`; a row moved to version 2 between select and update (simulate with `update_wrap` at a stale expected version) is `skipped`; a row under version 7 with no configured key is `skipped`, `remaining = 1`, and the others are rewrapped.
- Unit test on the loop-termination guard with a keyring lacking the rows' version.
- `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Background jobs": `CronService::secret_rotation` calls `rewrap_outdated` hourly and logs the report.