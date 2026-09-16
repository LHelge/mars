---
id: a86fj
title: "Implement the secret rotation job: hourly rewrap_outdated sweep with the report logged and unknown key versions warned"
status: open
priority: P2
created: "2026-09-16T20:44:51.980879695Z"
updated: "2026-09-16T20:44:51.980879695Z"
tags:
  - orchestrator
  - cron
  - secrets
depends_on:
  - yb2ny
parent: cxmar
---

## Summary
Fill in `CronService::secret_rotation`: once an hour call the secrets epic's `rewrap_outdated` so rows wrapped under an older master key move to the newest key without operator action. The job adds nothing to the sweep itself; it schedules it, maps the `RotationReport` into the common `JobReport`, and warns when rows remain under a version the keyring does not hold.

## Documents
- `ARCHITECTURE.md` "Background jobs" (secret rotation row: `1 h`, "Re-wrap rows whose `key_version` is behind the newest key, if any")
- `ARCHITECTURE.md` "Secrets" -> "Rotation" ("run `mars-orchestrator rotate-secrets` ... or wait for the cron job: it selects rows with `key_version < newest` in batches of 100 ...")
- `README.md` "Operating notes" (rotation bullet: add a new entry, restart, let the rotation job re-wrap; remove the old entry once no row is on the old version)
- `docs/data-model.md` `secrets` (`key_version`, `secrets_key_version_idx`)
- `CLAUDE.md` rule 3 (no key material in logs)

## Acceptance criteria
- [ ] `cron/secret_rotation.rs`: `impl CronService { pub async fn secret_rotation(&self, now: DateTime<Utc>) -> Result<JobReport> }` calls `secrets::rotation::rewrap_outdated(&self.state.pool, &self.state.keyring).await?` and returns `JobReport { items: report.rewrapped, skipped: report.skipped, failures: 0 }`.
- [ ] When `report.remaining > 0` after the sweep, the job logs `warn!(remaining = report.remaining, newest_version = keyring.newest_version(), "secrets remain under a master key version the keyring cannot unwrap")` once per run; nothing else about keys is logged (the sweep already logs unknown versions at `error`).
- [ ] When the keyring holds a single version the call returns immediately with zeros and the scheduler logs at `debug`; the job adds no pre-check of its own.
- [ ] An error from the sweep propagates as `Err` (the scheduler logs and retries next hour); partial progress committed by the sweep is kept.
- [ ] `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Implementation notes
- Files: `orchestrator/src/cron/secret_rotation.rs`, `orchestrator/src/cron/mod.rs`.
- The job runs concurrently with the API and with the `rotate-secrets` subcommand by design: `rewrap_outdated` guards every row update with `WHERE key_version = <expected>`.
- `now` is unused beyond the signature; do not thread it into the sweep.

## Edge cases
- Keyring newest version lower than some rows' `key_version` (operator removed the newest key): startup verification refuses to boot, so the job never sees it; if it did, those rows are not selected and `remaining` stays 0.
- The sweep and a `PUT /api/secrets/{id}` replace interleaving: the replace seals under the newest key; the sweep's guarded update skips the row (`skipped += 1`), which is not a failure.

## Testing
- Integration test `orchestrator/tests/cron_secret_rotation.rs` via `TestApp`: build an `AppState` clone whose `keyring` holds versions 1 and 2 (the `TestApp` fixed key as version 1 plus a second fake 32-byte key), seal 5 rows under version 1 through `SecretRepository`, run `CronService::new(state).secret_rotation(Utc::now())`: `items = 5`, every row has `key_version = 2`, ciphertexts byte-identical; second run `items = 0`; seed one row with `key_version = 7` and rerun: `skipped = 1` and the `warn` line is emitted (assert with a `tracing` test subscriber or a captured log layer if the harness has one; otherwise assert the report only).
- Command: `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Secrets manager": `secrets::rotation::rewrap_outdated(pool, keyring) -> Result<RotationReport { rewrapped, skipped, remaining }>`, `SecretsKeyring::newest_version()`, a way to construct a keyring with several versions in tests.