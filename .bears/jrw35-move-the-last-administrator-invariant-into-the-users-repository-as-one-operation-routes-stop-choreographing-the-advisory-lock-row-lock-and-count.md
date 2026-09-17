---
id: jrw35
title: Move the last-administrator invariant into the users repository as one operation; routes stop choreographing the advisory lock, row lock and count
status: open
priority: P2
created: "2026-09-17T20:02:59.380928728Z"
updated: "2026-09-17T20:02:59.380928728Z"
tags:
  - orchestrator
  - auth
  - architecture
depends_on:
  - "7erm7"
parent: wju32
---

## Summary
`docs/data-model.md` calls "at least one administrator remains" a repository invariant, but `repositories/users.rs` explicitly leaves it to callers and `routes/users.rs` executes it twice (`replace`, `remove`) as an order-dependent sequence: `lock_admin_membership` → `lock_user` → `count_admins` → mutate. Give `UserRepository` one operation per mutation that can violate the invariant (`update_role`/`replace` and `delete`), which takes the advisory lock, the row lock and the count inside, and returns the documented 409. The routes then only translate.

## Documents
- `SPEC.md` "Users" (409 when demoting or deleting the last administrator; self-deletion prohibited; check and mutation are one transaction serialised with other deletions and role changes).
- `docs/data-model.md` `users` (the advisory-lock paragraph: acquire before user-row or project locks, hold through commit, re-read the count after waiting).
- ADR 0025.

## Acceptance criteria
- [ ] `UserRepository::replace(id, NewUserFields) -> Result<User>` and `UserRepository::delete(id, acting_user_id) -> Result<()>` (names may follow the existing ones) own the transaction: `lock_admin_membership` → `lock_user` → `count_admins` → mutate → commit, and return `Error::Conflict("cannot remove the last administrator")` (keep the current message text) or the self-deletion conflict.
- [ ] `lock_admin_membership` and `count_admins` become private to `repositories/users.rs`; `grep -rn "lock_admin_membership\|count_admins" orchestrator/src/routes` finds nothing.
- [ ] The repository's module doc replaces "this is left to the routes" with the invariant statement from `docs/data-model.md`.
- [ ] `tests/users_last_admin_race.rs` keeps passing unchanged (it drives HTTP), and a repository-level race test proves two concurrent demotions of the final two administrators leave one.
- [ ] `cargo sqlx prepare` run and `.sqlx/` committed if query text changed.

## Implementation notes
- Files: `orchestrator/src/repositories/users.rs`, `orchestrator/src/routes/users.rs`, `orchestrator/tests/users_last_admin_race.rs`, `orchestrator/tests/repositories_users.rs`.
- Lock order stays: advisory lock first, then the user row, never a project lock inside (no tracker coordination here; session and lease release on user deletion belongs to the tracker epic's deletion hooks and runs in the tracker's own mutation after this commit, as `ARCHITECTURE.md` "Task tracker" describes).
- The pattern to follow is `apply_password_change`, the one policy-owning method the file already has.

## Edge cases
- Demoting yourself while another administrator exists is allowed; the repository receives the acting user's id only for the self-deletion rule.
- A `replace` that changes the username but not `admin` still takes the advisory lock (cheap, and it keeps the rule "every role-affecting mutation serialises").

## Testing
- `cd orchestrator && cargo fmt && cargo clippy --all-targets -- -D warnings && cargo clippy --all-targets --features integration-tests -- -D warnings && cargo test --features integration-tests` passes.

## Documentation
- None: `docs/data-model.md` already states the invariant as a repository invariant; the code now matches. Adjust the `routes/users.rs` module doc so it no longer describes the four-step sequence.