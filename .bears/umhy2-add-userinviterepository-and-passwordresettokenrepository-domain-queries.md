---
id: umhy2
title: Add UserInviteRepository and PasswordResetTokenRepository domain queries
status: open
priority: P1
created: "2026-09-16T20:28:08.083469761Z"
updated: "2026-09-16T20:28:08.083469761Z"
tags:
  - orchestrator
  - auth
parent: qacxf
---

## Summary
Provide all SQL for invitations and password-reset tokens: creating an invite with a hashed token and 7-day expiry, listing open invites, revoking, rotating the token on resend, locking and accepting an invite, and the reset-token insert/lookup/consume set. Together with the previous repository task this completes the data-access layer for the auth epic; the route tasks only compose these helpers.

## Documents
- `docs/data-model.md` `user_invites` (partial unique index `user_invites_open_email_idx`, 7-day expiry, "Inviting an email that already belongs to a user is refused with a conflict"), `password_reset_tokens` (single use; "A token must be unexpired and have null `used_at` when revalidated under the user lock").
- `SPEC.md` "Users (`/api/users`)" invite rows and the `Invite` DTO; "Auth (`/api/auth`)" rows for `GET /auth/invite/{token}` and `POST /auth/accept-invite`.
- ADR 0013, ADR 0025.

## Acceptance criteria
- [ ] `UserInviteRepository`: `insert(email, token_hash, admin, invited_by, expires_at) -> UserInvite`; `list_open() -> Vec<UserInvite>` (`WHERE accepted_at IS NULL AND expires_at > NOW() ORDER BY created_at DESC`); `find_open_by_hash(hash) -> Option<UserInvite>` (unlocked; open and unexpired); `lock_open_by_hash(tx, hash) -> Option<UserInvite>` (`FOR UPDATE`, open and unexpired); `find_open_by_id(id)`; `rotate_token(id, new_hash, new_expires_at) -> Option<UserInvite>` (`WHERE id = $1 AND accepted_at IS NULL`); `delete_open(id) -> bool` (`WHERE id = $1 AND accepted_at IS NULL`); `delete_expired_open_for_email(tx, email) -> u64`; `mark_accepted(tx, id, user_id)` (`SET accepted_at = NOW(), accepted_user_id = $2 WHERE id = $1 AND accepted_at IS NULL`); `delete_expired()` for the cron epic.
- [ ] `PasswordResetTokenRepository`: `insert(tx, user_id, token_hash, expires_at)`; `find_by_hash(hash) -> Option<PasswordResetToken>` (unlocked, any state, used to locate the user); `find_valid_by_hash_for_user(tx, hash, user_id) -> Option<PasswordResetToken>` (`WHERE token_hash = $1 AND user_id = $2 AND used_at IS NULL AND expires_at > NOW()`, executed after the user row lock); `mark_all_used_for_user(tx, user_id)`; `delete_expired()`.
- [ ] A unique-violation on `user_invites_open_email_idx` or `users_email_key` surfaces as `Error::Conflict` with a message the route can pass through (`an open invite already exists for this email` / `email already belongs to a user`), detected via `sqlx::Error::Database` and the constraint name, not by a pre-read.
- [ ] `.sqlx/` refreshed with `cargo sqlx prepare`; `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Implementation notes
- Files: `orchestrator/src/repositories/user_invites.rs`, `orchestrator/src/repositories/password_reset_tokens.rs`, `orchestrator/src/repositories/mod.rs`, `orchestrator/.sqlx/`.
- Models (`orchestrator/src/models/user_invite.rs`, `models/password_reset_token.rs`) mirror the tables; `UserInvite` serialises as `Invite = { id, email, admin, invited_by, expires_at, created_at }` with `token_hash`, `accepted_at`, `accepted_user_id` skipped (`#[serde(skip_serializing)]`). Add `UserInviteError::InvalidEmail` if the schema epic did not define email validation (trim, lower-case, must contain exactly one `@` with non-empty local and domain parts, at most 254 chars).
- Invite acceptance locks the invite row (there is no user row yet); reset consumption locks the user row first and re-reads the token under it, matching the data-model rule.
- Constraint-name mapping helper (e.g. `repositories::conflict_on(err, "user_invites_open_email_idx", msg)`) can be shared with later repositories.

## Edge cases
- Expired-but-unaccepted invites are not "open": they are excluded from `list_open`, `find_open_by_hash` and `lock_open_by_hash`, and `delete_expired_open_for_email` clears one out of the partial unique index's way before a new invite for the same email is inserted (the index treats it as open until the reaper deletes it).
- `rotate_token` on an accepted invite returns `None` (route answers 404).
- `mark_accepted` affecting zero rows means a concurrent acceptance won the race: return `Error::BadRequest("invalid or expired invite")` from the caller.
- Two reset tokens may be outstanding for one user (two requests within the limit); consuming either invalidates both through `mark_all_used_for_user`.

## Testing
- Repository integration tests on `TestApp::spawn().pool`: insert invite, `list_open` contains it; after `mark_accepted` it is gone from `list_open` and `find_open_by_hash` returns `None`; an invite with `expires_at` in the past is excluded; inserting a second open invite for the same email yields `Error::Conflict`; `delete_expired_open_for_email` then insert succeeds; `rotate_token` changes hash and expiry.
- Reset tokens: `find_valid_by_hash_for_user` returns `None` when used, expired or belonging to another user; `mark_all_used_for_user` sets `used_at` on every outstanding token.
- Command: `cd orchestrator && cargo sqlx prepare && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Database schema, models, repositories and test harness": the `users` migration with `user_invites` and `password_reset_tokens`, their model structs, and `TestApp::spawn()`.