---
id: qacxf
title: Authentication, users, invites and email
type: epic
status: done
priority: P1
created: "2026-09-16T20:11:57.012437137Z"
updated: "2026-09-17T11:22:24.765567342Z"
tags:
  - orchestrator
  - auth
depends_on:
  - p5tsd
---

## Scope

Everything under `SPEC.md`, "Authentication", "Auth (`/api/auth`)" and "Users (`/api/users`)", plus the `EmailClient` trait those flows need.

- JWT access tokens (15 min, claims `sub`, `auth_version`, `username`, `admin`, `must_change_password`), refresh tokens (30 days, hashed, rotated, HTTP-only cookie with `Secure` derived from `PUBLIC_URL`), Argon2id password hashing.
- The auth extractor: signature, expiry, current-user load, `auth_version` match, `must_change_password` gate (403 `password change required` except the allowed routes), admin extractor using current database values.
- Login throttling (10 failures per username or client address in 15 min -> 429 for 15 min), password-reset rate limit (3 per identifier per hour, still 204).
- Password change/reset transaction: hash update, `auth_version` increment, refresh-token revocation, reset-token invalidation, replacement pair for self-service; user-row locking for login, refresh and token mutations (ADR 0025).
- Invites: create, list, revoke, resend, `GET /auth/invite/{token}`, accept-invite creating the user; last-administrator protection with the advisory lock; self-deletion prohibited; `PATCH /users/me` for `notify_email`.
- `email/`: `EmailClient` trait, `ResendClient` over `reqwest`, `LogEmailClient` logging full links at `info` when `RESEND_API_KEY` is unset (ADR 0026), mock capturing messages behind `integration-tests`. Messages: invitation, password reset, and the escalation message template used later by the tracker.
- Test-only `POST /api/test/users` behind the `integration-tests` feature.

## Documents

`SPEC.md` "Authentication", "Auth", "Users", "Test-only routes"; `ARCHITECTURE.md` "User authentication and revocation"; `docs/data-model.md` "Users and authentication"; ADRs 0013, 0014, 0024, 0025, 0026.

## Acceptance criteria

- [ ] Every endpoint in the Auth and Users tables exists with the documented statuses and has integration tests for the happy path and each error path.
- [ ] Revocation tests: a refresh racing a password change either commits before it and is revoked, or fails.
- [ ] Last-admin tests cover concurrent demotion and deletion.
- [ ] Invite and reset flows are asserted through the mock email client; `LogEmailClient` output is asserted to contain the link, `ResendClient` never logs it.

## Out of scope

WebSocket/SSE re-authorization at heartbeat ticks (Real-time delivery epic), the escalation email trigger itself (Task tracker epic).