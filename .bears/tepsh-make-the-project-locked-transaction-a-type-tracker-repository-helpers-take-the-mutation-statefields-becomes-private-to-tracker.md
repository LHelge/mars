---
id: tepsh
title: "Make the project-locked transaction a type: tracker repository helpers take the mutation, StateFields becomes private to tracker/"
status: open
priority: P1
created: "2026-09-17T20:00:15.388889008Z"
updated: "2026-09-17T20:00:15.388889008Z"
tags:
  - orchestrator
  - tracker
  - architecture
  - docs
depends_on:
  - thes7
parent: kg8cx
---

## Summary
Replace the 22 "call only inside a `begin_mutation` transaction" doc warnings in `repositories/tasks/*.rs` with a type that only `TrackerMutation` can produce, so that every tracker write is under the project lock by construction. Restrict `StateFields` and `set_task_state_fields` to `tracker/`, so the coupling between state kind, lease, `attempts` and `closed_at` is decided in `change_state` and nowhere else.

## Documents
- `ARCHITECTURE.md` "Task tracker" → "One mutation at a time per project" (every writer locks the project row first; helpers share the transaction).
- `docs/data-model.md` "Tracker mutation transactions".
- `CLAUDE.md` "Backend conventions" (repository helpers accept the caller's transaction).
- ADRs 0021, 0028.

## Acceptance criteria
- [ ] `tracker/mutation.rs` exposes a connection token (for example `pub struct Locked<'a>(&'a mut PgConnection)` constructed only by `TrackerMutation::conn()`, or the helpers take `&mut TrackerMutation` directly; pick one and use it everywhere). Every `TaskRepository` method that writes a tracker table, and every read that the documents say must happen under the lock (`find_task_for_update`, `task_in_project` when used for a write, state-configuration reads before a write), takes that token instead of `&mut PgConnection`.
- [ ] `TaskRepository::begin_mutation` is `pub(crate)` and called only from `TrackerMutation::begin`.
- [ ] `StateFields` and `set_task_state_fields` are `pub(crate)` and referenced only from `tracker/` (assert with a grep in the task's own review, not a test).
- [ ] `grep -rn "inside a .*begin_mutation" orchestrator/src` returns nothing; the module doc of `repositories/tasks/mod.rs` states the rule once, in terms of the token type.
- [ ] Lock-free reads (board lists, detail loaders, `ready_summaries`) keep taking `&PgPool` or `&mut PgConnection`; the token is for writes and locked reads only.
- [ ] `cargo sqlx prepare` run if any query text changed; `.sqlx/` committed.

## Implementation notes
- Files: `orchestrator/src/repositories/tasks/{mod,rows,states,events,handoffs,dependencies,links,comments}.rs`, `orchestrator/src/tracker/mutation.rs`, `orchestrator/src/tracker/mod.rs`.
- `append_task_events` and `touch_task_session` are called from `TrackerMutation::commit` only; make them `pub(crate)` too.
- `SessionRepository::append_events` and `set_state` are not tracker writes; leave them. Combined tracker/session operations (launch-for-task, lease release on session end) obtain the session connection from the mutation's token, which preserves the documented lock order.
- Do not change any SQL; this task changes signatures and visibility.

## Edge cases
- The hand-off publication path (code hand-offs epic) validates under the git lock first and then opens the mutation; it needs the token for the database half only.
- Deletion cascades that touch several projects: the mutation acquires project rows in UUID order (`ARCHITECTURE.md`); the token type does not need to encode which project, one token per mutation is enough.

## Testing
- Existing repository tests compile against the new signatures by going through `TrackerMutation::begin` (the follow-up task moves them onto the verbs; here they may still call helpers, but only with the token).
- A compile-fail doc test or a `#[cfg(test)]` comment is not required; the visibility and the type are the proof.
- `cd orchestrator && cargo fmt && cargo clippy --all-targets -- -D warnings && cargo clippy --all-targets --features integration-tests -- -D warnings && cargo test --features integration-tests` passes.

## Documentation
- `ARCHITECTURE.md` "Task tracker" → "One mutation at a time per project": add one sentence that tracker repository helpers accept the mutation's locked connection rather than a bare connection, so a write outside the project lock does not compile.
- `CLAUDE.md` "Backend conventions": qualify "Repository helpers accept the caller's transaction" with "tracker helpers accept the `TrackerMutation`'s locked connection".

## Assumes from other epics
- "Task tracker": `thes7` (`TrackerMutation`) exists; `h8qkt`, `cuw5s`, `cws3a` and later tasks are written against the token from the start.