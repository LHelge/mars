---
id: "9jv8t"
title: "Implement password change and reset: POST /users/{id}/password, POST /auth/request-password-reset, POST /auth/reset-password"
status: done
priority: P1
created: "2026-09-16T20:30:48.390164927Z"
updated: "2026-09-17T10:20:47.827835436Z"
tags:
  - orchestrator
  - auth
depends_on:
  - nhtrn
  - qx67f
  - ruxrp
  - umhy2
  - "99sgv"
parent: qacxf
assignee: claude-opus-subagent
attempts: 1
---

## Summary
Implement the three password mutation flows on top of `apply_password_change`: self-service change (requires `current_password`, returns a replacement token pair so the browser stays signed in), admin change of another user's password (204, acting admin's credentials untouched), reset-link request (always 204, rate limited, email through `EmailClient`) and reset by link (204, no login). Every successful mutation increments `auth_version`, revokes all refresh tokens and invalidates all reset links in one transaction under the user-row lock, and clears `must_change_password`, which is how the seeded administrator completes first login.

## Documents
- `SPEC.md` "Authentication" bullets 3 and 5 (gate cleared by change, self-service pair, admin change 204, reset 204 without login, revalidate after locking).
- `SPEC.md` "Auth (`/api/auth`)" rows `/auth/request-password-reset`, `/auth/reset-password`; "Users (`/api/users`)" row `POST /users/{id}/password`; "User-facing features" (passwords 10–128, reset limit 3 per identifier per hour still 204).
- `docs/data-model.md` `users` (atomic mutation paragraph), `password_reset_tokens` (single use, revalidated under the user lock, invalidated by any change).
- `README.md` "Start" (admin/changeme, first-login change), "Configuration" (`PUBLIC_URL` for email links).
- ADR 0013, ADR 0024, ADR 0025, ADR 0026.

## Acceptance criteria
- [ ] `POST /api/users/{id}/password` takes `UngatedUser`. Self (`id == caller.id`): body `{ current_password, password }`; missing `current_password` → 400 `current password required`; wrong `current_password` → 400 `current password is incorrect` (revalidated against the locked row); invalid new password → 400 from `UserError`; success → 200 `{ user, access_token }` where the access token carries the new `auth_version` and `must_change_password: false`, plus a new `refresh_token` cookie; every previous refresh token is revoked and every reset token used.
- [ ] Same route, another user's id: caller must be an administrator by database row (else 403 `admin required`), `current_password` is ignored, unknown id → 404, success → 204 with no `Set-Cookie`; the acting admin's own tokens still work afterwards; the target's tokens are revoked and the target's `must_change_password` is cleared.
- [ ] `POST /api/auth/request-password-reset` `{ identifier }`: always 204 with an empty body; when the identifier matches a username or email (email match lower-cased/trimmed) and the rate limiter allows, the handler locks the user row, inserts a reset token with `expires_at = NOW() + 1 hour`, commits, then sends `EmailMessage::password_reset(user.email, "<PUBLIC_URL>/reset-password/<raw token>", expires_at)`; unknown identifiers and limited requests send nothing and take a comparable amount of time (no early return before the limiter records the call). Email failures are logged at `error` and still answer 204.
- [ ] `POST /api/auth/reset-password` `{ token, password }`: unlocked `find_by_hash` to locate the user (unknown → 400 `invalid or expired token`); `BEGIN`; `lock_for_update(user_id)` (None → 400); `find_valid_by_hash_for_user` under the lock (None → 400); validate password (400); `apply_password_change(.., replacement: None)`; `COMMIT`; 204, no cookie, no body.
- [ ] Password hashing runs in `spawn_blocking`; the transaction is opened only after the new hash exists.
- [ ] `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Implementation notes
- Files: `orchestrator/src/routes/users.rs` (password handler), `orchestrator/src/routes/auth.rs` (reset handlers), reuse `issue_pair`, `refresh_cookie`, `ResetRateLimit`, `UserRepository::apply_password_change`, `PasswordResetTokenRepository`, `EmailClient`.
- DTOs: `ChangePasswordRequest { current_password: Option<String>, password: String }`, `RequestPasswordResetRequest { identifier: String }`, `ResetPasswordRequest { token: String, password: String }`.
- Self-service sequence: hash new password (blocking) → `BEGIN` → `lock_for_update(caller.id)` (None → 401) → verify `current_password` against the locked hash (blocking verify is acceptable inside the transaction here, or verify before and again after locking) → generate `OpaqueToken` → `apply_password_change(tx, id, hash, Some(&token.hash))` → `COMMIT` → mint access token from the returned user → respond with cookie `refresh_cookie(config, token.raw)`.
- Admin sequence: authorise (`caller.admin` from the DB row) → hash → `BEGIN` → `lock_for_update(id)` (None → 404) → `apply_password_change(.., None)` → `COMMIT` → 204.
- Reset request: normalise identifier; `reset_rate_limit.allow(identifier)`; lookup; if both pass: `BEGIN` → `lock_for_update` → `PasswordResetTokenRepository::insert` → `COMMIT` → `email.send(..)` outside the transaction. Log `user_id = %id` at `info` for a sent reset, never the token (the `LogEmailClient` is the only logger of the link).
- Reset TTL constant `PASSWORD_RESET_TTL = Duration::hours(1)` next to the other TTLs in the prelude.

## Edge cases
- A gated user (`must_change_password`) changing their own password is the primary first-login path; the route is exempt from the gate through `UngatedUser`. A gated administrator changing another user's password is also allowed (the route is listed as exempt without qualification).
- Reusing a reset link after a successful reset → 400 (`used_at` set); a reset link issued before an unrelated password change → 400 (invalidated by that change).
- `current_password` supplied on an admin-for-other change is ignored, not validated.
- Same new password as old is allowed (no history rule in v1).
- `identifier` that matches by username for one user and by email for another cannot happen (`users.email` and `users.username` are separate unique columns, but a username equal to another user's email is possible): prefer the username match, then the email match, and send at most one email.

## Testing
- `tests/users_password.rs`: self change happy path (new pair works, old refresh cookie → 401, old access token → 401 on `GET /users/me`, `must_change_password` false); wrong current password 400 and nothing revoked; missing current password 400; short password 400; non-admin on another user 403; admin on another user 204, target's old cookie 401, admin's own cookie still refreshes; admin on unknown id 404; gated seeded-style user completes first login through this route and can then call a gated route.
- `tests/auth_reset.rs`: request for a known email sends exactly one message through `MockEmailClient` containing `/reset-password/<token>`; request for unknown identifier 204 and no message; 4th request within the hour 204 and still only 3 messages; reset with the token from the captured message → 204, login with the new password works, old refresh cookie 401; reusing the token 400; token after a self-service change 400; expired token (set `expires_at` in the past through the pool) 400; identifier by username also works.
- Command: `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- `SPEC.md` "User-facing features", "Login and invites" paragraph and `docs/data-model.md` `password_reset_tokens`: state the reset-link validity (1 hour) in the same commit; the documents currently give the invite TTL (7 days) but not the reset TTL.

## Assumes from other epics
- "Database schema, models, repositories and test harness": `UserError::InvalidPassword` (10–128) exists; `TestApp` exposes the mock email client.