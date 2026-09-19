---
id: "75puv"
title: Remove repositories/tasks/test_support.rs once the tracker tests run through the verbs
status: open
priority: P2
created: "2026-09-19T13:44:22.859331846Z"
updated: "2026-09-19T13:44:22.859331846Z"
tags:
  - orchestrator
  - tracker
  - architecture
  - tests
depends_on:
  - vtssn
parent: kg8cx
---

## Summary
`tepsh` made `StateFields`, `set_task_state_fields`, `touch_task_session` and `append_task_events` `pub(crate)`, and had to leave one public door for the integration tests (a separate crate): `orchestrator/src/repositories/tasks/test_support.rs`, a `pub mod` exporting `TaskRepositoryTestExt` and `StateFields`. It cannot be gated on the `integration-tests` feature because CI lints `--all-targets` without it. Every method still demands a `Locked<'_>`, but the module is public API of the library that exists only for tests. Its own module doc says it goes away with the task that moves the tests onto the tracker verbs (`vtssn`); the verbs it was waiting for now exist (`tracker::state::change_state`, `tracker::leases::{claim_for_profile, claim_for_launch, release_by_user, release_by_agent}`).

Filed while merging `tepsh` during the work for `md2zq`.

## Documents
- `ARCHITECTURE.md` "Task tracker" → "One mutation at a time per project" (tracker writes take the mutation's locked connection).

## Acceptance criteria
- [ ] Callers of `TaskRepositoryTestExt` in `tests/` (`repositories_tasks_rows.rs`, `repositories_tasks_core.rs`, `tracker_dto.rs`, `tracker_graph.rs`, `tracker_state.rs`, `tracker_leases.rs`, `tracker_escalation.rs`, `sessions_api.rs` and any later ones) set up leases, attempts, terminal states and current hand-offs through the tracker verbs, or through a fixture in `tests/common/` built on them.
- [ ] The assertions that genuinely test the repository statements (the sequence allocator with malformed batches, `first_touched_at` preservation) move to unit tests inside the crate (`#[cfg(test)]` next to the statement, on the test pool helper) where `pub(crate)` is reachable.
- [ ] `orchestrator/src/repositories/tasks/test_support.rs` is deleted and `grep -rn test_support orchestrator` returns nothing.
- [ ] Both clippy invocations and the full test suite pass.