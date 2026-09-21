---
id: u9fvv
title: User and profile deletion can deadlock with a live session's writer the way tracker mutations did (kb48s)
status: open
priority: P3
created: "2026-09-21T17:35:09.418102714Z"
updated: "2026-09-21T17:35:09.418102714Z"
tags:
  - orchestrator
  - sessions
  - auth
  - projects
depends_on:
  - kb48s
---

## Summary
Discovered while fixing kb48s. A session-only transaction that updates its `sessions` row twice makes Postgres re-run that row's foreign-key checks, taking `FOR KEY SHARE` on the project, profile, user, task and hand-off the session names (ADR 0041). kb48s removed the cycle for tracker mutations (`FOR NO KEY UPDATE`) and for task deletion (sessions locked first). Two holders outside the tracker still take a referenced row exclusively and then wait for a session row:

- **User deletion**: `users` row `FOR UPDATE` / `DELETE`, then `sessions.created_by` is `SET NULL` — waits for the session row while a live session's writer waits for `FOR KEY SHARE` on the user.
- **Profile deletion**: the `ON DELETE RESTRICT` check reads `sessions` `FOR KEY SHARE` while holding the profile row; with a live session the right answer is the 409, not a possible `40P01`.

Neither has been observed; both follow from the mechanism and are rare (a deletion racing a `result` line or a state transition of a session that references the row).

## Documents
- `ARCHITECTURE.md`, "Task tracker", "Tracker locks never block a foreign-key check"; ADR 0041, "Consequences".

## Acceptance criteria
- [ ] Each path either locks the referencing session rows before taking its own row exclusively (as `TaskRepository::delete_task` does) or refuses before it would wait, and the rule is stated in `ARCHITECTURE.md`.
- [ ] A deterministic test per path in the style of `tests/tracker_mutation.rs`, `deleting_a_task_waits_for_its_session_instead_of_deadlocking_with_it`, failing with `40P01` before the fix.
- [ ] The ADR 0041 consequence naming this gap is removed.