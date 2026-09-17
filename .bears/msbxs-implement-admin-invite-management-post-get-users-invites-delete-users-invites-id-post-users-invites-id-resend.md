---
id: msbxs
title: "Implement admin invite management: POST/GET /users/invites, DELETE /users/invites/{id}, POST /users/invites/{id}/resend"
status: done
priority: P1
created: "2026-09-16T20:31:13.777280918Z"
updated: "2026-09-17T10:39:37.165543846Z"
tags:
  - orchestrator
  - auth
depends_on:
  - nhtrn
  - qx67f
  - umhy2
  - "8pnnv"
parent: qacxf
assignee: claude-opus-subagent
attempts: 1
---

## Summary
Add the four administrator invite endpoints to `routes/users.rs`: create an invite for an email (hashed single-use token, 7-day expiry, email delivered through `EmailClient`), list open invites, revoke one, and resend one with a fresh token and expiry. The raw token exists only in the email (or the `LogEmailClient` record) and is never part of an `Invite` response. Invite acceptance is the next task.

## Documents
- `SPEC.md` "Users (`/api/users`)" rows `GET /users/invites`, `POST /users/invites`, `DELETE /users/invites/{id}`, `POST /users/invites/{id}/resend`; the `Invite` DTO paragraph; the `LogEmailClient` paragraph.
- `SPEC.md` "User-facing features", "Login and invites" (7-day expiry, revocable, no self-registration).
- `docs/data-model.md` `user_invites` (partial unique index, conflict on an email that has a user).
- ADR 0013, ADR 0014, ADR 0026.

## Acceptance criteria
- [ ] `POST /api/users/invites` (`AdminUser`) `{ email, admin?: bool }` (default `false`): email trimmed and lower-cased; invalid email → 400; email of an existing user → 409 `email already belongs to a user`; an open (unexpired, unaccepted) invite for the email → 409 `an open invite already exists for this email`; otherwise insert with `token_hash = OpaqueToken::generate().hash`, `invited_by = caller.id`, `expires_at = NOW() + 7 days`, commit, then send `EmailMessage::invitation(email, "<PUBLIC_URL>/invite/<raw>", expires_at)`; 201 `Invite`.
- [ ] `GET /api/users/invites` (`AdminUser`) → 200 `Invite[]` of open, unexpired invites, newest first.
- [ ] `DELETE /api/users/invites/{id}` (`AdminUser`) → 204; 404 when unknown or already accepted. A revoked invite's link answers 400 on lookup.
- [ ] `POST /api/users/invites/{id}/resend` (`AdminUser`) → 200 `Invite` with a new `expires_at` (`NOW() + 7 days`) and a new token; the old link no longer resolves; the email is sent again; 404 when unknown or accepted.
- [ ] `Invite` JSON is exactly `{ id, email, admin, invited_by, expires_at, created_at }`; a test asserts the key set and that no field contains the token or its hash.
- [ ] Email is sent after the database commit; a send failure is logged at `error` and returned as a 500 generic error while the invite row remains, so `resend` recovers; `ResendClient` never logs the link.
- [ ] `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Implementation notes
- Files: `orchestrator/src/routes/users.rs` (register `/invites` routes before `/{id}`), `orchestrator/src/repositories/user_invites.rs` (already has the queries), reuse `OpaqueToken`, `INVITE_TTL`, `EmailClient`.
- DTO: `CreateInviteRequest { email: String, admin: Option<bool> }`.
- Creation sequence: validate/normalise email → `BEGIN` → `UserRepository::find_by_email` (Some → 409) → `UserInviteRepository::delete_expired_open_for_email(tx, email)` → `insert` (unique violation on `user_invites_open_email_idx` → 409 via the repository mapping) → `COMMIT` → `email.send`. The user-existence check and the insert are in one transaction so a concurrent accept-invite for the same email is caught by the `users.email` unique constraint at accept time, not here.
- Resend: `rotate_token(id, new.hash, NOW() + 7 days)` (None → 404) then send with `new.raw`.
- Log invite creation at `info` with `invite_id = %id` and `invited_by = %caller.id`; never the email body or token.

## Edge cases
- Inviting the caller's own email → 409 (they are a user).
- Inviting an email whose previous invite expired: the expired row is deleted and a new one created (201), not 409.
- `admin` omitted → `false`; explicit `admin: true` creates an administrator on acceptance.
- `MockEmailClient::fail_next()` path: 500, invite still listed, resend then delivers.
- Email longer than 254 characters or without a domain part → 400.

## Testing
- `tests/users_invites.rs`, one test per scenario: create → 201 with exact key set, one captured message whose text contains `/invite/` followed by a token whose SHA-256 equals the stored `token_hash` (read via pool); duplicate open invite 409; existing user email 409; expired open invite replaced 201; invalid email 400; non-admin 403; unauthenticated 401; list shows open only (accepted and expired excluded); delete 204 then list empty and the token no longer resolves via `find_open_by_hash`; delete unknown 404; resend 200 with later `expires_at`, second captured message with a different token, old token unresolvable; resend accepted invite 404; email failure → 500 and invite persists.
- `LogEmailClient` end-to-end check: a unit/integration test building `AppState` with `LogEmailClient` and the in-memory tracing writer asserts the logged record contains the full `/invite/<token>` link (can live in this task or in the email task; ensure it exists once).
- Command: `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Database schema, models, repositories and test harness": `UserInvite` model; `TestApp` exposes `MockEmailClient` captured messages.