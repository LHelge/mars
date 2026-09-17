---
id: wju32
title: "Architecture: login credentials and administrator membership issued from one place"
type: epic
status: open
priority: P2
created: "2026-09-17T19:59:36.274053474Z"
updated: "2026-09-17T19:59:36.274053474Z"
tags:
  - orchestrator
  - auth
  - architecture
depends_on:
  - naqhy
---

## Why

The architecture survey (2026-09-17) found that credential issuance is a discipline, not a type. Five handlers (`routes/auth.rs` login, refresh, request reset, reset, accept invite; `routes/users.rs` the two password changes) each re-implement locate → `lock_user` → revalidate under lock → mutate → commit → mint → cookie, and the rule is only enforced by five module doc blocks. `issue_pair` takes a `&User` and a connection and trusts the caller. The refresh lifetime is written three times (`prelude::REFRESH_TOKEN_TTL`, `auth.rs`, and a hard-coded `INTERVAL '30 days'` in `repositories/users.rs`); `INVITE_TTL` in the prelude is dead beside a live `INVITE_TTL_DAYS`; the `Secure` cookie rule exists twice, one copy dead. The last-administrator invariant that `docs/data-model.md` calls "a repository invariant" is a four-statement, order-dependent sequence duplicated in two route handlers, and the repository explicitly disclaims it. `UserInvite::is_usable` and `PasswordResetToken::is_usable` duplicate SQL `WHERE` clauses and have no production caller. `ARCHITECTURE.md` promises that extractor rejections answer in the documented error shape, but only `routes/secrets.rs` has the `Path`/`Query` wrappers that make it true.

## Scope

- One module owns issuing, rotating and revoking login credentials, including lifetimes, the cookie and the lock-then-revalidate rule; routes translate HTTP only.
- The administrator-membership invariant lives in the users repository as one operation.
- Each validity predicate, message constant and extractor wrapper has one home.
- Auth tests assert through HTTP and the credential module's interface rather than raw SQL.

## Documents

`ARCHITECTURE.md` "Orchestrator internals" (module tree, Errors) and "User authentication and revocation"; `SPEC.md` "Authentication" and "Users"; `docs/data-model.md` `users`, `refresh_tokens`, `user_invites`, `password_reset_tokens`; ADRs 0013, 0025.

## Acceptance criteria

- [ ] `Claims::encode` and `refresh_cookie` have exactly one production caller each, inside the credential module.
- [ ] Each lifetime (access, refresh, invite, reset) is a single constant with no SQL literal copy.
- [ ] `lock_admin_membership` and `count_admins` are called only inside the users repository.
- [ ] Every route module answers path and query rejections as `{ "status", "error" }`.

## Out of scope

The MCP session bearer (a different credential; MCP epic). WebSocket and SSE heartbeat rechecks (real-time epic) reuse `authenticate_access_token` unchanged.