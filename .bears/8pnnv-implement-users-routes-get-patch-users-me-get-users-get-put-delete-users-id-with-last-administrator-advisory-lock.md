---
id: "8pnnv"
title: "Implement users routes: GET/PATCH /users/me, GET /users, GET/PUT/DELETE /users/{id} with last-administrator advisory lock"
status: done
priority: P1
created: "2026-09-16T20:30:07.800849511Z"
updated: "2026-09-17T09:44:26.417882700Z"
tags:
  - orchestrator
  - auth
depends_on:
  - qx67f
  - ruxrp
parent: qacxf
assignee: claude-opus-subagent
attempts: 1
---

## Summary
Create `orchestrator/src/routes/users.rs` with the user-management endpoints and the repository operations they need: self profile read and `notify_email` update, admin listing, single user read, admin edit of `username`/`admin`, and deletion. Demotion and deletion run inside one transaction that takes the transaction-scoped advisory lock for administrator membership, re-reads the administrator count after acquiring it, and rejects any change that would leave zero administrators with 409; self-deletion is refused with 409. Invite endpoints and the password endpoint are separate tasks in this module.

## Documents
- `SPEC.md` "Users (`/api/users`)" rows `GET /users/me`, `PATCH /users/me`, `GET /users`, `GET /users/{id}`, `PUT /users/{id}`, `DELETE /users/{id}`; the `User` DTO; the paragraph "At least one administrator must remain ...".
- `SPEC.md` "Authentication" bullets 2 and 3 (demotion on next request, gate exemptions include `GET /users/me`).
- `docs/data-model.md` `users`, paragraph "User deletion and changes to `users.admin` preserve at least one administrator ..." (advisory lock, ordering before user-row locks, self-demotion allowed, self-deletion prohibited) and "Demotion changes authorization ... without incrementing `auth_version`".
- `ARCHITECTURE.md` "User authentication and revocation"; ADR 0025.

## Acceptance criteria
- [ ] `GET /api/users/me` (`UngatedUser`) → 200 `User` (works while `must_change_password` is true).
- [ ] `PATCH /api/users/me` (`CurrentUser`) `{ notify_email?: bool }` → 200 `User`; an empty body is a no-op returning the current user; other fields are rejected (`serde(deny_unknown_fields)`, 400).
- [ ] `GET /api/users` (`AdminUser`) → 200 `User[]` ordered by `username`; `GET /api/users/{id}` (`CurrentUser`) → 200 `User` or 404.
- [ ] `PUT /api/users/{id}` (`AdminUser`) `{ username, admin }` (both required) → 200 `User`; 400 on invalid username; 404 unknown id; 409 `username already taken` on the unique violation; 409 `cannot demote the last administrator` when `admin` goes true → false and the locked count of administrators is 1; self-demotion succeeds when another administrator remains. Changing `admin` does not touch `auth_version` or refresh tokens.
- [ ] `DELETE /api/users/{id}` (`AdminUser`) → 204; 409 `cannot delete yourself` when `id` equals the caller; 409 `cannot delete the last administrator` when the target is an administrator and the locked count is 1; 404 unknown id. Refresh and reset tokens cascade; invites, projects, sessions, tasks and comments keep their rows with the user reference set to NULL (schema FKs).
- [ ] Advisory lock: `SELECT pg_advisory_xact_lock($1)` with the constant `ADMIN_MEMBERSHIP_LOCK: i64` (defined once in `UserRepository`) is the first statement of the transaction for every deletion and every admin-flag change, acquired before `lock_for_update` on the target user; the administrator count is read after the lock; the mutation and the check commit together; a rejection rolls back with no field changed.
- [ ] `cargo sqlx prepare` output committed; `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Implementation notes
- Files: `orchestrator/src/routes/users.rs` (new), `orchestrator/src/routes/mod.rs` (nest under `/api/users`), `orchestrator/src/repositories/users.rs` (add `lock_admin_membership(tx)`, `count_admins(tx) -> i64`, `update_profile(tx, id, username, admin) -> Option<User>`, `delete(tx, id) -> bool`, `list() -> Vec<User>`), `orchestrator/.sqlx/`.
- Route order matters in Axum: register `/me` and `/invites` (next task) before `/{id}` so the literal segments win.
- `PUT` sequence: `BEGIN`; `lock_admin_membership`; `lock_for_update(id)` (None → 404); if `current.admin && !body.admin` then `count_admins` (== 1 → 409); validate `username` through `UserError`; `update_profile`; `COMMIT`.
- `DELETE` sequence: reject self before opening the transaction; `BEGIN`; `lock_admin_membership`; `lock_for_update(id)` (None → 404); if `target.admin` then `count_admins` (== 1 → 409); `delete`; `COMMIT`.
- Lock ordering note for other epics: advisory lock → user row → (nothing else here). If the tracker epic later coordinates user deletion with project locks, the advisory lock still comes first.

## Edge cases
- `PUT` with the same username and same admin flag is a no-op 200.
- Two administrators demoting each other concurrently: the second to acquire the advisory lock sees a count of 1 and gets 409 (the concurrency tests task exercises this; make the repository sequence above exact so it holds).
- Deleting a user who is the only administrator while they are `must_change_password` gated is still 409; the gate is irrelevant to the check.
- Deleting a user with active sessions is allowed in v1 (`sessions.created_by` is `SET NULL`); no engine interaction.
- `GET /users/{id}` is readable by any authenticated user (every user sees every user in v1).

## Testing
- `tests/users.rs`, one test per scenario: `me` while gated 200 and while ungated 200; `PATCH /users/me` toggles `notify_email` and persists; unknown field 400; list requires admin (403 for non-admin, 401 unauthenticated); `PUT` rename happy path; duplicate username 409; invalid username 400; demote last admin 409 and the row is unchanged (username edit in the same request must not apply); self-demotion with two admins 200 and the next admin call from that user is 403 `admin required` while `GET /users/me` still works with the same token (auth_version unchanged); `DELETE` self 409; last admin 409; non-admin target 204 and their refresh cookie then answers 401; unknown id 404.
- Command: `cd orchestrator && cargo sqlx prepare && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Database schema, models, repositories and test harness": `UserRepository` basic CRUD and `UserError::InvalidUsername`; the `users` migration FKs as listed in `docs/data-model.md`.