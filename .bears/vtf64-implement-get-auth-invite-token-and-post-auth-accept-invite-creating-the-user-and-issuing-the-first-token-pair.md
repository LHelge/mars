---
id: vtf64
title: Implement GET /auth/invite/{token} and POST /auth/accept-invite creating the user and issuing the first token pair
status: done
priority: P1
created: "2026-09-16T20:31:58.359453069Z"
updated: "2026-09-17T10:59:37.471008561Z"
tags:
  - orchestrator
  - auth
depends_on:
  - rb2xf
  - umhy2
  - "99sgv"
  - msbxs
parent: qacxf
assignee: claude-opus-subagent
attempts: 1
---

## Summary
Complete the invite flow on the invitee side: the unauthenticated lookup that lets the accept page show the invited email and role, and acceptance, which validates the chosen username and password, creates the user row with the invite's email and admin flag, marks the invite accepted and issues the first token pair, all in one transaction that locks the invite row. This is the only way a non-seeded user comes into existence (ADR 0013).

## Documents
- `SPEC.md` "Auth (`/api/auth`)" rows `GET /auth/invite/{token}` (`{email, admin, expires_at}`, 400 if expired, used or unknown) and `POST /auth/accept-invite` (`{token, username, password}` → `{user, access_token}`, 201).
- `SPEC.md` "Authentication" bullet 1 (accept-invite sets the refresh cookie like login).
- `SPEC.md` "User-facing features", "Login and invites"; "Non-goals for v1" (no self-registration).
- `docs/data-model.md` `users` (email lower-cased, comes from the invite), `user_invites` (`accepted_at`, `accepted_user_id`, single use).
- ADR 0013.

## Acceptance criteria
- [ ] `GET /api/auth/invite/{token}`: hashes the path token, `find_open_by_hash`; unknown, accepted or expired → 400 `invalid or expired invite`; success → 200 `{ email, admin, expires_at }` and nothing else (no id, no inviter).
- [ ] `POST /api/auth/accept-invite` `{ token, username, password }`: validate username (3–32) and password (10–128) through `UserError` → 400; `BEGIN`; `lock_open_by_hash(tx, hash)` (None → 400 `invalid or expired invite`); insert user `{ username, email: invite.email, password_hash, admin: invite.admin, must_change_password: false, notify_email: true }`; unique violation on `users_username_key` → 409 `username already taken`, on `users_email_key` → 409 `email already belongs to a user`; `mark_accepted(tx, invite.id, user.id)`; `issue_pair(tx, &user)`; `COMMIT`; 201 `{ user, access_token }` with the `refresh_token` cookie.
- [ ] Accepting the same link twice → 400 on the second call; the first user remains.
- [ ] `must_change_password` is false for accepted users; an invite with `admin = true` produces an administrator who can call `GET /users` immediately.
- [ ] `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Implementation notes
- Files: `orchestrator/src/routes/auth.rs` (two handlers, DTOs `InviteLookupResponse { email, admin, expires_at }`, `AcceptInviteRequest { token, username, password }`), reuse `OpaqueToken::hash_of`, `hash_password` (in `spawn_blocking`, before the transaction), `UserRepository::insert(tx, ..)` (add a transaction-taking variant if the schema epic's insert borrows the pool), `UserInviteRepository`, `issue_pair`, `refresh_cookie`.
- Lock order: invite row (`FOR UPDATE`) only; there is no user row yet. `lock_open_by_hash` must include `accepted_at IS NULL AND expires_at > NOW()` in its `WHERE` so a revoked (deleted) or expired invite is simply absent.
- No throttle on these routes in v1 (tokens are 256-bit-equivalent random; not listed in `SPEC.md`).
- Trace acceptance at `info` with `invite_id = %invite.id` and `user_id = %user.id`; never the token.

## Edge cases
- Two concurrent accepts with the same token: the second blocks on the row lock, then finds the invite accepted (`lock_open_by_hash` returns `None` after the first commit) → 400.
- Invite email now belongs to a user created by another path (e.g. the test route) → 409 at insert; the invite stays open for the admin to revoke.
- Username equal to another user's email is allowed (different columns).
- Token with surrounding whitespace in the JSON body: trim before hashing; path parameter used verbatim.
- `expires_at` in the lookup response is RFC 3339 like every other timestamp.

## Testing
- `tests/auth_invite.rs`, one test per scenario: create invite as admin, extract the token from the `MockEmailClient` message, lookup 200 with exact key set; lookup unknown/expired/revoked/accepted → 400 each; accept happy path 201, cookie set, access token valid on `GET /users/me`, `GET /users/invites` no longer lists it, `accepted_user_id` equals the new user (via pool); accept twice 400; short username 400; short password 400; duplicate username 409 and the invite still open; admin invite yields an admin; non-admin invite yields a non-admin (403 on `GET /users`).
- Command: `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Database schema, models, repositories and test harness": `UserRepository::insert` and `UserError` validation; `TestApp` exposes the mock email client.