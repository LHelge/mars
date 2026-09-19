---
id: n9zms
title: "Add the tracker event-matrix and history-hygiene test suite: every TaskEvent kind with actor, from/to and reason; no rows on rejected or no-op operations; deleted keeps task_id; touch timestamps"
status: in_progress
priority: P2
created: "2026-09-16T20:47:28.451006585Z"
updated: "2026-09-19T17:49:58.670657033Z"
tags:
  - orchestrator
  - tracker
  - tests
depends_on:
  - "89fh2"
  - "72jr7"
  - kctj2
  - "2sjtz"
  - rujpc
parent: "5h3y4"
attempts: 1
---

## Summary
Add one integration test file that reads the whole `task_events` stream of a project after a scripted sequence of operations and asserts it against an expected list of `(kind, actor kind, task_id, from, to, reason)` tuples, covering all thirteen event kinds, then asserts the negative rules that ADR 0030 and the epic's acceptance criteria require: rejected operations and no-op updates write no rows and do not advance `task_sessions`, `deleted` retains `task_id` and omits `task`, and every event's `task` payload reflects the state after its change. This is the epic-level regression net over the per-task tests.

## Documents
- `SPEC.md` "TaskEvent" (kinds, `actor` shapes, `from`/`to` on `state_changed` and `escalated`, `reason` values on `released` and free text on `escalated`, `deleted` carries the original UUID and omits `task`, `states_changed` with `states`), "Tasks" (no event on no-op; current-state no-op), "MCP tool contracts" first paragraph (reads and rejections write nothing).
- `docs/data-model.md` `task_events` (append-only, `MAX(seq)+1`, `deleted` written in the deleting transaction), `task_sessions` (touch only on actual change).
- `ARCHITECTURE.md` "Task tracker" (all subsections except "Code hand-offs" and "Review approval").
- ADRs 0022, 0030.

## Acceptance criteria
- [ ] `orchestrator/tests/tracker_events.rs` with a helper `events_of(pool, project_id) -> Vec<TaskEvent>` (through `list_task_events_after(project_id, 0, 10_000)` and `TaskEvent::from_row`) and an `expect!` style assertion listing `(seq offset, kind, actor kind, task number or None, from, to, reason)`.
- [ ] Scenario A (user, over REST): create A; create B with `depends_on [A]`; rename state `review` → `qa`; comment on A; add `related` B→A; move A to `done`; move A back to `ready`; release nothing (409, no row); `PUT` A with its current state (no row); delete A. Expected kinds in order: `created`, `created`, `dependency_added`, `blocked`, `states_changed`, `commented`, `dependency_added`, `state_changed(ready→done)`, `unblocked`, `state_changed(done→ready)`, `blocked`, `dependency_removed` ×2 (blocks, related), `unblocked`, `deleted` (task_id = A, no `task` key in the stored payload JSON). Seqs are 1..n with no gaps.
- [ ] Scenario B (sessions, through the tracker functions with sessions inserted by the repository, `max_attempts = 2`): session S1 claims A (`claimed`, actor session), releases with reason (`commented`, `released given_back`); S2 claims (`claimed`), releases (`commented`, `commented` system, `escalated ready→needs_human` with reason starting `attempt limit reached (2/2):`); user releases nothing (409); S3 launched for A via `claim_for_launch` (`claimed`, actor user); `release_leases_for_session(S3, SessionEnded)` (`commented` system, `released session_ended`, actor system); S4 `needs_human` on A already in the human state (`commented`, `updated`, no `released` because S4 does not hold it, no `escalated`).
- [ ] Scenario C (parent closure): parent P with children C1, C2; close C1 (`state_changed`), close C2 (`state_changed` C2, `state_changed` P with actor `system`, `to` = lowest-position terminal, then P's dependants' `unblocked` if any); reopen C1 → P stays closed, `blocked` event on P only.
- [ ] Negative assertions after each rejected call (cycle 409, parent-rule 400, unknown state 400, claim conflict, non-holder release, `PUT` with no changes, `PUT` current state): `COUNT(*) FROM task_events` unchanged and `task_sessions.last_touched_at` unchanged for the session involved; `ready_summaries` and `load_task_detail` also leave both unchanged.
- [ ] Every event with a `task` payload has `task.id == task_id` and, for `state_changed`/`escalated`, `task.state == to`.
- [ ] The file runs green in CI with `--features integration-tests` and takes under 30 seconds on the testcontainers database.

## Implementation notes
- Files: `orchestrator/tests/tracker_events.rs`; shared helpers may go to `orchestrator/tests/common/tracker.rs` (project with default states, sessions, a `Tracker` facade over `TrackerMutation::begin` + domain function + `commit`).
- Drive scenario A over HTTP through `TestApp` so route wiring is covered; scenarios B and C through the domain functions so no MCP transport is needed.
- Assert on the stored JSON (`payload` column) for the `deleted` case, not only the deserialised struct, to prove the `task` key is absent.
- Keep the expected lists literal (no loops generating expectations) so a reviewer can read the contract from the test.

## Edge cases
- Ordering within one mutation is emission order; if a per-task test and this suite disagree, the per-task test's task body wins and this suite is updated in the same commit as the fix.
- Rename of the human or terminal state mid-scenario must not change `to` values already recorded (names are captured at emission time).
- `states_changed` events have `task_id` null: the helper must not filter them out.

## Testing
- This task is the test; `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes with the new file included.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Database schema, models, repositories and test harness": `list_task_events_after`, `SessionRepository` inserts, `TestApp`.
- "Authentication, users, invites and email": `TestApp` login helpers.