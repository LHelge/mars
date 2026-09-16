---
id: rb2xf
title: "Add auth primitives: JWT Claims, Argon2id password hashing, opaque token generation, refresh cookie and the User DTO shape"
status: open
priority: P0
created: "2026-09-16T20:27:16.329398231Z"
updated: "2026-09-16T20:51:51.630071023Z"
tags:
  - orchestrator
  - auth
depends_on:
  - p5tsd
parent: qacxf
---

## Summary
Provide the pure building blocks every auth route uses: the `Claims` struct with encode/decode against `JWT_SECRET`, Argon2id hashing and verification, the opaque token generator (`<uuid>.<uuid>` raw, SHA-256 hex stored) shared by refresh, invite and reset tokens, the `refresh_token` cookie builder/clearer whose `Secure` flag derives from `PUBLIC_URL`, and the guarantee that `User` serialises to exactly the `SPEC.md` shape. No database access and no routes in this task.

## Documents
- `SPEC.md` "Authentication" (claims list, 15-minute access token, cookie attributes, `Secure` when `PUBLIC_URL` is https).
- `SPEC.md` "Users (`/api/users`)" (`User = { id, username, email, admin, must_change_password, notify_email, created_at }`).
- `docs/data-model.md` `users` (`password_hash` is an Argon2id PHC string, never serialised; `auth_version` internal), `refresh_tokens` (raw token is two UUIDs joined by `.`, row stores SHA-256 hex; 30 days).
- `ARCHITECTURE.md` "Orchestrator internals" (`prelude/` holds `Claims`; crates `jsonwebtoken`, `argon2`, `sha2`; `ClaimsError` is an `Error` variant; `axum-extra` `cookie`).
- ADR 0025.

## Acceptance criteria
- [ ] `Claims { sub: Uuid, auth_version: i64, username: String, admin: bool, must_change_password: bool, iat: i64, exp: i64 }` in `orchestrator/src/prelude/claims.rs`, with `Claims::for_user(&User, now) -> Claims` setting `exp = iat + 900`, `Claims::encode(&self, &Config) -> Result<String, ClaimsError>` and `Claims::decode(token, &Config) -> Result<Claims, ClaimsError>` (HS256, signature and expiry validated, no leeway beyond `jsonwebtoken` default of 0).
- [ ] `ClaimsError` (`Invalid`, `Expired`) converts into `Error` and answers 401 with error `authentication required`.
- [ ] `hash_password(&str) -> Result<String, UserError>` produces an Argon2id PHC string with a fresh salt; `verify_password(hash, candidate) -> bool` returns false, never panics, on a malformed hash.
- [ ] `OpaqueToken::generate() -> OpaqueToken { raw: String, hash: String }` where `raw` is `<uuid v4>.<uuid v4>` and `hash` is lowercase SHA-256 hex of `raw`; `OpaqueToken::hash_of(raw) -> String` is the same function used on the way in.
- [ ] `refresh_cookie(&Config, raw: &str) -> Cookie<'static>` sets name `refresh_token`, `HttpOnly`, `SameSite=Lax`, `Path=/`, `Max-Age` 30 days, `Secure` iff `config.public_url` starts with `https://`; `clear_refresh_cookie(&Config)` has the same attributes with an empty value and `Max-Age=0`.
- [ ] `User` serialises to `{ id, username, email, admin, must_change_password, notify_email, created_at }` and nothing else (`password_hash`, `auth_version`, `updated_at` skipped); a unit test asserts the exact key set.
- [ ] `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Implementation notes
- Files: `orchestrator/src/prelude/claims.rs` (new), `orchestrator/src/prelude/mod.rs` (re-export `Claims`, `ClaimsError`), `orchestrator/src/models/user.rs` (hashing helpers, serde attributes), `orchestrator/src/models/token.rs` (new, `OpaqueToken`), `orchestrator/src/routes/cookies.rs` (new, cookie helpers; pure functions, used by `routes/auth.rs`, `routes/users.rs`, `routes/test.rs`).
- Constants, exported from `prelude`: `ACCESS_TOKEN_TTL = Duration::minutes(15)`, `REFRESH_TOKEN_TTL = Duration::days(30)`, `INVITE_TTL = Duration::days(7)`, `REFRESH_COOKIE = "refresh_token"`.
- Use `argon2::Argon2::default()` with `PasswordHasher`/`PasswordVerifier` and `SaltString::generate(&mut OsRng)`; hashing is CPU-bound, so route handlers call it inside `tokio::task::spawn_blocking` (note for later tasks; helper `hash_password_blocking` may live here).
- Use `sha2::Sha256` and hex encoding (`format!("{:x}")` or `base16` via `hex` if already present; do not add a crate just for hex).
- `Config.jwt_secret` as `jsonwebtoken::EncodingKey::from_secret` / `DecodingKey::from_secret`; validate `exp` and require `sub` to parse as `Uuid`.
- `User` model: `#[serde(skip_serializing)]` on `password_hash`, `auth_version`, `updated_at`; do not derive `Deserialize` for `User`.

## Edge cases
- `PUBLIC_URL` with uppercase scheme or trailing slash: compare the scheme case-insensitively; trim the trailing slash before building links elsewhere.
- Token decode with a wrong algorithm header (`alg: none`) must fail: restrict `Validation` to HS256.
- `iat` in the future is not rejected (clock skew is out of scope); `exp` is strict.
- `verify_password` on the seeded admin hash from the `users` migration must succeed with `changeme` (unit test uses a hash produced by `hash_password`, plus one integration test elsewhere logs in as the seeded admin before `TestApp` removes it if the harness allows; otherwise skip).

## Testing
- Unit tests in `claims.rs`: round trip; expired token → `ClaimsError::Expired`; tampered signature → `ClaimsError::Invalid`; `alg` other than HS256 rejected.
- Unit tests in `models/user.rs`: hash then verify true; wrong password false; malformed hash false; two hashes of the same password differ (salt).
- Unit tests in `models/token.rs`: raw has the `<uuid>.<uuid>` shape; `hash_of(raw) == hash`; hash is 64 lowercase hex chars.
- Unit tests in `routes/cookies.rs`: attributes for `http://` and `https://` `PUBLIC_URL`; clear cookie has `Max-Age=0`.
- Serialisation test: `serde_json::to_value(user)` key set equals the SPEC list.
- Command: `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Repository scaffolding, tooling and CI": `Config` exposes `jwt_secret: String` and `public_url: String`; prelude `Error` enum with `ClaimsError` and `UserError` variants and `IntoResponse`.
- "Database schema, models, repositories and test harness": `models::user::User` struct with the columns of `users` and `UserError` with the username (3–32) and password (10–128) validation rules; this task adds hashing and serde attributes to it.