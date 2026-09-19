---
id: "98qnn"
title: "Implement the token cleanup job: expired refresh tokens, reset tokens, unaccepted invites and secrets whose scope row is gone"
status: open
priority: P2
created: "2026-09-16T20:44:35.622437638Z"
updated: "2026-09-16T20:44:35.622437638Z"
tags:
  - orchestrator
  - cron
  - auth
  - secrets
depends_on:
  - yb2ny
parent: cxmar
---

## Summary
Fill in `CronService::token_cleanup`: once an hour delete expired refresh tokens, expired password-reset tokens, expired unaccepted invites and secrets whose `user` or `project` scope row no longer exists. Each of the four deletions is its own statement with the cut-off passed as a bound `now`, so tests control time and one failing statement does not prevent the others.

## Documents
- `ARCHITECTURE.md` "Background jobs" (token cleanup row: `1 h`, "Delete expired refresh tokens, reset tokens, unaccepted invites, and secrets whose scope row no longer exists")
- `docs/data-model.md` `refresh_tokens` (`expires_at`, index `refresh_tokens_expires_at_idx`), `user_invites` ("A background reaper deletes expired, unaccepted invites"; partial unique index `user_invites_open_email_idx` treats an expired unaccepted invite as open until deleted), `password_reset_tokens` ("A background reaper deletes expired rows"), `secrets` (`scope_id` has no FK "because the scope decides the target table; the repository validates existence and a reaper deletes orphans"), `secret_uses` (`ON DELETE CASCADE` from `secrets`)
- `CLAUDE.md` "Backend conventions" (repositories hold all SQL; `cargo sqlx prepare`)

## Acceptance criteria
- [ ] Repository methods, each taking `now: DateTime<Utc>` and returning the number of deleted rows: `RefreshTokenRepository::delete_expired(now)` (`DELETE FROM refresh_tokens WHERE expires_at < $1`; revoked-but-unexpired rows are kept), `PasswordResetTokenRepository::delete_expired(now)` (`DELETE FROM password_reset_tokens WHERE expires_at < $1`), `UserInviteRepository::delete_expired(now)` (`DELETE FROM user_invites WHERE accepted_at IS NULL AND expires_at < $1`; accepted invites are kept as history), `SecretRepository::delete_orphans()` (`DELETE FROM secrets WHERE (scope = 'user' AND NOT EXISTS (SELECT 1 FROM users u WHERE u.id = secrets.scope_id)) OR (scope = 'project' AND NOT EXISTS (SELECT 1 FROM projects p WHERE p.id = secrets.scope_id))`; `global` rows are never touched). If the auth epic already added `delete_expired()` without a `now` parameter, change its signature rather than adding a second method.
- [ ] `cron/token_cleanup.rs`: `impl CronService { pub async fn token_cleanup(&self, now: DateTime<Utc>) -> Result<JobReport> }` runs the four statements in that order, each as its own autocommit statement; `items` is the sum of deleted rows; a statement error is logged `error!(step = "refresh_tokens" | "password_reset_tokens" | "user_invites" | "orphan_secrets", error = %e)`, counts `failures += 1`, and the remaining steps still run; the job returns `Ok` when at least one step succeeded and `Err` only when all four failed.
- [ ] Per-step counts are logged at `info` with structured fields (`refresh_tokens = n, reset_tokens = n, invites = n, orphan_secrets = n`) when any is non-zero; secret names or values never appear in logs.
- [ ] `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes; `.sqlx/` refreshed.

## Implementation notes
- Files: `orchestrator/src/cron/token_cleanup.rs`, `orchestrator/src/cron/mod.rs`, `orchestrator/src/repositories/refresh_tokens.rs`, `orchestrator/src/repositories/password_reset_tokens.rs`, `orchestrator/src/repositories/user_invites.rs`, `orchestrator/src/repositories/secrets.rs`, `orchestrator/.sqlx/`.
- No row locks: these deletions touch rows nobody else updates once expired; the user-row locking rule in `docs/data-model.md` applies to issuing and consuming tokens, not to deleting expired ones. A refresh racing the deletion of its expired token fails the same way it would have failed on expiry.
- Deleting a secret cascades its `secret_uses` rows; that is intended (the audit belongs to a scope that no longer exists).

## Edge cases
- `now` earlier than some `created_at` (clock skew in tests): nothing is deleted; no special handling.
- An orphan `project` secret whose project is mid-deletion: project deletion already removes its secrets in its own transaction; the sweep only catches rows left by an interrupted deletion.
- Large backlogs: single `DELETE` statements are fine at v1 scale; no batching.

## Testing
- Integration test `orchestrator/tests/cron_token_cleanup.rs` via `TestApp`: seed for one user a refresh token expiring at `now - 1 s` (also one revoked but unexpired, one valid), a reset token expired and one valid, an unaccepted invite expired, an unaccepted invite valid, an accepted invite whose `expires_at` is past; create a `user`-scoped secret for a second user and a `project`-scoped secret for a project, then delete that user and (through the repository, bypassing the project service) the project row; also a `global` secret. Run `app.cron().token_cleanup(now)`: report `items = 5` (1 refresh + 1 reset + 1 invite + 2 secrets), the revoked-unexpired and valid rows remain, the accepted invite remains, the global secret remains, `secret_uses` of the deleted secrets are gone; after the sweep a new invite for the expired invite's email succeeds (the partial unique index is clear); a second run returns `JobReport::default()`.
- Command: `cd orchestrator && cargo sqlx prepare && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Authentication, users, invites and email": `RefreshTokenRepository`, `PasswordResetTokenRepository`, `UserInviteRepository` with their `delete_expired` placeholders and `TestApp` user helpers.
- "Secrets manager": `SecretRepository` with insert and `insert_use` for seeding test rows.
- Note (p7emz, 2026-09-19): `SecretRepository::list_orphans` and its test `orphans_are_the_scoped_rows_whose_target_is_gone` were deleted as dead code, along with `distinct_key_versions` and `list_meta`. This task adds `delete_orphans()` with its own test; do not expect a selecting query to build on.
