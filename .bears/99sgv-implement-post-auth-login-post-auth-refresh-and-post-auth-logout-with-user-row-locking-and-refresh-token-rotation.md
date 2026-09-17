---
id: "99sgv"
title: Implement POST /auth/login, POST /auth/refresh and POST /auth/logout with user-row locking and refresh-token rotation
status: done
priority: P0
created: "2026-09-16T20:29:38.038521864Z"
updated: "2026-09-17T09:51:39.944014667Z"
tags:
  - orchestrator
  - auth
depends_on:
  - rb2xf
  - qx67f
  - ruxrp
  - "78zk5"
parent: qacxf
assignee: claude-opus-subagent
attempts: 1
---

## Summary
Create `orchestrator/src/routes/auth.rs` with the three credential-issuing endpoints. Login verifies the password, then locks the user row, revalidates against the locked row, inserts a refresh token and returns `{ user, access_token }` plus the cookie. Refresh locates the token, locks its user, re-reads the token under the lock, revokes it, inserts a replacement and returns a new pair from current user values. Logout revokes the presented token and clears the cookie. This task also provides the shared `issue_pair` helper that accept-invite, password change and the test route reuse.

## Documents
- `SPEC.md` "Authentication", bullets 1, 4 and 5 (response shape, cookie, rotation, 401 clears the cookie, serialise on the user row and revalidate after locking).
- `SPEC.md` "Auth (`/api/auth`)" rows `/auth/login`, `/auth/refresh`, `/auth/logout`; "REST API" status table (401, 429).
- `SPEC.md` "User-facing features", "Login and invites" (15-minute access token, 30-day refresh cookie, throttling).
- `docs/data-model.md` `users` ("Login, refresh, reset-link issuance ... lock the user row ... Return credentials only after commit"), `refresh_tokens`.
- `ARCHITECTURE.md` "User authentication and revocation"; ADR 0025.

## Acceptance criteria
- [ ] `POST /api/auth/login` `{ username, password }`: 400 on missing fields; 429 `too many login attempts` when the throttle blocks the username or client address (before any hashing); 401 `invalid username or password` on unknown user or wrong password (recorded as a throttle failure; unknown user runs a dummy verification so timing does not reveal existence); on success 200 `{ user, access_token }` with `Set-Cookie: refresh_token=...; HttpOnly; SameSite=Lax; Path=/; Max-Age=2592000[; Secure]`. Users with `must_change_password = true` can log in.
- [ ] Login transaction: verify password outside the transaction (`spawn_blocking`), then `BEGIN`, `lock_for_update(user.id)`, verify again against the locked row's `password_hash` (401 if it changed), `RefreshTokenRepository::insert`, `COMMIT`, then mint the access token from the committed row (`Claims::for_user`).
- [ ] `POST /api/auth/refresh` (cookie only): missing cookie, unknown hash, revoked, expired or missing user → 401 `authentication required` and a `Set-Cookie` that clears `refresh_token`. Success: `BEGIN`, unlocked `find_by_hash` to learn `user_id`, `lock_for_update(user_id)` (missing → 401), `find_by_hash_for_user` under the lock re-checking `revoked_at IS NULL AND expires_at > NOW()` (else 401), `revoke(old.id)`, `insert` new token, `COMMIT`, then 200 `{ user, access_token }` with the new cookie, claims built from the locked (current) row.
- [ ] `POST /api/auth/logout`: revokes the cookie's token if present (`revoke_by_hash`), always clears the cookie and answers 204, even without a cookie.
- [ ] `routes()` is nested under `/api/auth`; `issue_pair(&AppState, tx, &User) -> Result<(String /*raw refresh*/, RefreshToken)>` and `TokenPairResponse { user: User, access_token: String }` are `pub(crate)` for reuse.
- [ ] Integration tests cover every path listed under Testing; `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Implementation notes
- Files: `orchestrator/src/routes/auth.rs` (new), `orchestrator/src/routes/mod.rs` (nest), reuse `routes/cookies.rs`, `routes/throttle.rs`, `routes/extractors.rs`.
- DTOs private to the module: `LoginRequest { username: String, password: String }`; response `TokenPairResponse` (shared, `pub(crate)`).
- Cookie handling through `axum_extra::extract::CookieJar` (`jar.add(refresh_cookie(..))`, `jar.add(clear_refresh_cookie(..))`); return `(StatusCode, CookieJar, Json<..>)`.
- Throttle: call `login_throttle.check(&username, client_addr)` first; `record_failure` on 401; `record_success` after commit.
- Trace failed logins at `info` with `username = %username` and `addr = %addr` only; never the password.
- Locking order: user row lock, then the user's `refresh_tokens` rows. No project or session locks in these handlers.

## Edge cases
- Refresh presented after a password change: the unlocked lookup finds the row, the locked re-read sees `revoked_at` set → 401 + cleared cookie (this is the revocation race the epic tests).
- Two concurrent refreshes with the same cookie: the first commits and revokes; the second re-reads under the lock, sees `revoked_at`, answers 401 (no token-family reuse detection in v1).
- A cookie for a deleted user: `lock_for_update` returns `None` → 401 + cleared cookie.
- Login body with whitespace around the username: trim before lookup; password used verbatim.
- Throttle failure counting uses the submitted username even when the user does not exist.

## Testing
- `tests/auth_login.rs` (one `#[tokio::test]` per scenario): happy path returns user without `password_hash`/`auth_version` keys, an access token that decodes to the user's claims with `exp - iat == 900`, and a `Set-Cookie` with the exact attributes for the harness `PUBLIC_URL`; wrong password 401; unknown user 401; missing field 400; 10 failures then 429; 429 for a different username from the same address; `must_change_password` user can log in and the returned `must_change_password` is true.
- `tests/auth_refresh.rs`: refresh returns a new access token and a new cookie value; the old cookie value then answers 401 with a clearing `Set-Cookie`; no cookie 401; expired token (set `expires_at` in the past through the pool) 401; deleted user 401; refresh after `apply_password_change` (through the repository) 401; claims in the refreshed token carry the current `admin` flag after a direct DB demotion.
- `tests/auth_logout.rs`: 204 and the token is revoked in the table; 204 without cookie.
- `TestApp` helpers: `login(username, password) -> (TokenPairResponse, refresh_cookie: String)`, `refresh(cookie)`.
- Command: `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- none: implements the documented contract as written. (If a `TestApp` helper insertion of users is needed before the test route exists, insert through `UserRepository`.)

## Assumes from other epics
- "Repository scaffolding, tooling and CI": `main.rs`/`lib.rs` build the `/api` router from each module's `routes()` and `tower-http` tracing is configured to not log request bodies.
- "Database schema, models, repositories and test harness": `TestApp::spawn()` with an `axum_test::TestServer`, and `UserRepository::insert` to seed users for tests.