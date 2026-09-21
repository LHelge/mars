---
id: "75puv"
title: Remove repositories/tasks/test_support.rs once the tracker tests run through the verbs
status: in_progress
priority: P2
created: "2026-09-19T13:44:22.859331846Z"
updated: "2026-09-21T03:15:54.432692528Z"
tags:
  - orchestrator
  - tracker
  - architecture
  - tests
depends_on:
  - vtssn
parent: kg8cx
attempts: 1
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

## Widened after `vtssn` merged (2026-09-21)
- `vtssn` moved `repositories_tasks_{core,rows}.rs` to `tests/tracker_states.rs` and `tests/tracker_tasks.rs`, which no longer use `TaskRepositoryTestExt`. The callers left on `main` are: `tests/common/handoffs.rs` (`Fixture::claim`, `Fixture::set_fields`, `Fixture::current_handoff`), `cron_stuck_tasks.rs`, `events_listener.rs`, `handoffs_transaction.rs`, `mcp_create_task.rs`, `mcp_lease_tools.rs`, `mcp_ready_get.rs`, `mcp_update.rs`, `realtime_acceptance.rs`, `sessions_api.rs`, `tasks_api.rs`, `tracker_dto.rs`, `tracker_escalation.rs`, `tracker_graph.rs`, `tracker_leases.rs`, `tracker_provenance.rs`, `tracker_state.rs`.
- `vtssn` could not move the row-level guards into the crate, because `tests/common/db.rs` resolves its state directory with `env!("CARGO_TARGET_TMPDIR")`, which cargo sets for integration test binaries only. These are therefore unasserted on `main` and this task restores them as in-crate unit tests: `set_task_state_fields` refusing a state of another project, a hand-off of another task and an unknown task id; `append_task_events` refusing an event about another project's task (the `deleted` exception is still covered through `delete_task` in `tests/tasks_api.rs`).
- [ ] The in-crate tests reuse the one-server-per-run harness instead of a second one: `tests/common/db.rs` is reachable from the library's `#[cfg(test)]` code (for instance a `#[cfg(test)] #[path = "../tests/common/db.rs"] mod` in `src/`), with the state directory resolved without `env!("CARGO_TARGET_TMPDIR")` when it is unset (`option_env!`, falling back to `tmp/` beside the profile directory of `std::env::current_exe()`, which is the same `target/tmp`). A lib test process and an integration test process of one run share the same server.