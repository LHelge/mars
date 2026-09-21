---
id: vtssn
title: "Move tracker tests onto the verb interface: delete the test-side mutation envelope helpers and assert lock and notify behaviour only at the TrackerMutation seam"
status: in_progress
priority: P2
created: "2026-09-17T20:02:01.328838073Z"
updated: "2026-09-21T02:24:58.272771753Z"
tags:
  - orchestrator
  - tracker
  - architecture
  - tests
  - docs
depends_on:
  - tepsh
  - cuw5s
  - cws3a
parent: kg8cx
attempts: 1
---

## Summary
The tracker's verbs are the test surface. `tests/repositories_tasks_core.rs` and `tests/repositories_tasks_rows.rs` currently re-implement the mutation envelope in helpers (`in_mutation`, `insert_state`, `append`, `publish_handoff`) and assert on `StateFields` bags while naming claim, terminal move and reopen in comments. Once `change_state`, the lease verbs and `TrackerMutation` exist, rewrite those tests to drive the verbs, keep the lock-behaviour tests (per-project serialisation, single notify on commit, none on rollback) at the `TrackerMutation` seam, and delete the helpers.

## Documents
- `ARCHITECTURE.md` "Task tracker" (the verbs and their rules), "Event delivery".
- `CLAUDE.md` "Testing expectations".
- ADRs 0021, 0028.

## Acceptance criteria
- [ ] No file under `orchestrator/tests/` calls `TaskRepository::begin_mutation`, `set_task_state_fields`, `append_task_events` or `touch_task_session` directly.
- [ ] `the_state_fields_move_together_and_stay_in_scope` and its siblings are replaced by tests named after the rules they prove (`a_holders_move_clears_the_lease_and_resets_attempts`, `entering_a_terminal_state_closes_and_unblocks_dependants`, `assigning_the_current_state_preserves_lease_attempts_and_closure`), each calling the verb.
- [ ] `mutations_serialise_per_project_and_not_across_projects`, `the_project_lock_serialises_task_numbering` and `a_batch_notifies_once_on_commit_and_never_on_rollback` survive, driven through `TrackerMutation::begin` and a verb.
- [ ] Row-level scope tests (`WHERE id = $1 AND project_id = $2`) that cannot be expressed through a verb move into `#[cfg(test)]` modules next to the repository, not integration tests.
- [ ] The hand-off arrangement helper `publish_handoff` is replaced by the code hand-offs epic's publication function if it has landed, otherwise by the smallest verb-level arrangement and a comment naming the task that replaces it.

## Implementation notes
- Files: `orchestrator/tests/repositories_tasks_core.rs`, `orchestrator/tests/repositories_tasks_rows.rs`, possibly renamed to `tests/tracker_*.rs` to sit beside the tracker epic's `tracker_state.rs` and `tracker_leases.rs`; `orchestrator/src/repositories/tasks/*.rs` (`#[cfg(test)]` modules).
- Do not duplicate what `tracker_state.rs` and `tracker_leases.rs` already assert; delete rather than move when a scenario is already covered there.

## Edge cases
- The tracker epic's `n9zms` event-matrix suite covers "every event kind with actor"; this task does not re-assert payloads.

## Testing
- `cd orchestrator && cargo fmt && cargo clippy --all-targets -- -D warnings && cargo clippy --all-targets --features integration-tests -- -D warnings && cargo test --features integration-tests` passes.

## Documentation
- `CLAUDE.md` "Testing expectations": add "Tracker tests drive the verbs in `tracker/` through `TrackerMutation`; they never call the row helpers in `repositories/tasks/` directly."

## Assumes from other epics
- "Task tracker": `cuw5s` (`change_state`), `cws3a` (leases), `n9zms` (event matrix).