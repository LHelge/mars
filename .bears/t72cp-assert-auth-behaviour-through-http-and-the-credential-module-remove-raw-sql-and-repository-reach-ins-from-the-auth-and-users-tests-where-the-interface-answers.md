---
id: t72cp
title: "Assert auth behaviour through HTTP and the credential module: remove raw SQL and repository reach-ins from the auth and users tests where the interface answers"
status: done
priority: P3
created: "2026-09-17T20:04:43.884217910Z"
updated: "2026-09-19T19:33:36.171568597Z"
tags:
  - orchestrator
  - auth
  - architecture
  - tests
  - docs
depends_on:
  - jrw35
  - kc58a
parent: wju32
attempts: 1
---

## Summary
Nine of the twelve auth test files run raw `sqlx::query` or call `UserRepository`/`UserInviteRepository` directly to arrange or assert state, and three re-assert the cookie attribute string as a literal. With the credential module owning issuance and the repositories owning the administrator invariant, most of those reach-ins have an interface to go through. Rewrite them so that a test proves a rule at the seam callers use, and keep direct SQL only where the rule is about the row itself (a revoked token row exists; `auth_version` incremented).

## Documents
- `CLAUDE.md` "Testing expectations" (backend integration tests use `TestApp`, assert with `assert_status` and `json`).
- `SPEC.md` "Authentication".

## Acceptance criteria
- [ ] `tests/auth_login.rs`, `auth_logout.rs`, `auth_revocation_race.rs` no longer assert the `Set-Cookie` attribute string; the one assertion lives in `tests/auth_credentials.rs` from the credential-module task.
- [ ] `tests/auth_reset.rs`, `auth_refresh.rs`, `users_invites.rs`, `users_password.rs`, `users_last_admin_race.rs` arrange state through `TestApp` helpers (`create_user`, `login`, `refresh`, the invite and reset flows over HTTP, `email` captures) and assert through responses; raw `sqlx::query` remains only for row-level facts, each with a one-line comment saying which fact.
- [ ] `tests/common/app.rs` gains the helpers the rewrite needs (for example `invite_and_accept`, `request_reset_link`) rather than each file re-implementing them; `TestApp`'s public fields stay.
- [ ] `tests/common/races.rs` keeps its SQL count of unrevoked tokens (that is a row-level fact).

## Implementation notes
- Files: `orchestrator/tests/auth_*.rs`, `orchestrator/tests/users*.rs`, `orchestrator/tests/common/app.rs`.
- Do not lower coverage: every scenario that exists keeps a test; only the way it arranges and asserts changes.

## Testing
- `cd orchestrator && cargo fmt && cargo clippy --all-targets -- -D warnings && cargo clippy --all-targets --features integration-tests -- -D warnings && cargo test --features integration-tests` passes.

## Documentation
- `CLAUDE.md` "Testing expectations": one sentence that auth tests arrange and assert through HTTP or the credential module, and reach into SQL only for row-level facts.