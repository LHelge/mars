---
id: sgwwv
title: "Implement the hand-off tracker transaction: recheck, task_handoffs row with review carry, comment, current pointer, state change, links and events"
status: done
priority: P1
created: "2026-09-16T20:41:54.042318861Z"
updated: "2026-09-19T21:31:35.584737082Z"
tags:
  - orchestrator
  - tracker
depends_on:
  - gv4j2
parent: xjaah
attempts: 1
---

## Summary
Implement the second half of the publication protocol as one function that runs inside the tracker mutation transaction: lock the task row, recheck state, holder and previous current hand-off against the `PreparedHandoff`, then write the `task_comments` row, the `task_handoffs` row (fresh, decided or carried review attribution), `tasks.current_handoff_id`, the state change with lease release and `attempts` reset (through the tracker epic's state-change helper), the `task_sessions` links and the `state_changed` and `commented` events. Everything commits together or not at all.

## Documents
- `docs/data-model.md` `tasks` ("When code is published or forwarded, that same transaction also inserts `task_handoffs` and its required `task_comments` row, updates `current_handoff_id`, upserts the relevant source/calling session links, and emits the comment event. The immutable git ref is prepared first; the transaction rechecks state, holder and the previous current-hand-off id before publishing"), `task_handoffs` (exactly one creation actor; explicit decision sets exactly one reviewer and `reviewed_at`; forwarding preserves attribution even when the prior reviewer was deleted; a new revision never carries an old approval even for the same commit), `task_comments` (author rule), `task_sessions` (upsert on hand-off; a user publishing a hand-off also links its source session), `task_events`, "Tracker mutation transactions".
- `ARCHITECTURE.md` "Task tracker" -> "Code hand-offs" (database transaction locks the project row before the task, rechecks those values and the caller's authority, atomically writes hand-off, comment, pointer, lease release, state change and events), "Review approval" (decision actor and time recorded separately from the forwarding actor; without a decision the existing status and attribution carry forward; a new revision always starts `unreviewed`).
- `SPEC.md` "Code hand-offs and review" (review and creation actors come from authenticated context; `comment_id` points to the comment written with this hand-off; every state event carries the full task including its current hand-off; the accompanying `commented` event carries the comment), "TaskEvent" (`state_changed` with `from`/`to`; `commented` with `comment`; `blocked`/`unblocked`).
- ADRs 0018, 0021, 0028, 0030.

## Acceptance criteria
- [ ] `tracker::handoffs::publish_in_transaction(tx: &mut PgConnection, project_id, prepared: &PreparedHandoff, update: &TaskUpdateRequest, target_state: &TaskState, caller: &HandoffCaller, actor: &TaskEventActor) -> Result<Task>`; documented as "inside `begin_mutation`".
- [ ] Recheck after `find_task_for_update`: `state_id` unchanged since preparation, `lease_holder_session_id` unchanged (and equal to the caller for `HandoffCaller::Session`), `current_handoff_id == prepared.previous_handoff_id`; any difference -> `Error::Conflict("task changed during hand-off publication; re-read it and retry")` and the transaction is rolled back by the caller.
- [ ] Comment: `insert_comment(tx, project_id, NewTaskComment { author_user_id / author_session_id from caller, system: false, body: prepared.comment })`.
- [ ] Hand-off row: `insert_handoff(tx, project_id, NewTaskHandoff { id: prepared.id, task_id, source_session_id, source_branch, commit, comment_id, review_status, reviewed_by_user_id, reviewed_by_session_id, reviewed_at, created_by_user_id / created_by_session_id from caller })` with review fields: `Fresh` -> `unreviewed`, no reviewer, no time; `Decision(d)` -> `d` as status, reviewer = caller (user or session), `reviewed_at = NOW()` (bind the transaction timestamp); `CarriedFrom(prev)` -> copy `review_status`, `reviewed_by_user_id`, `reviewed_by_session_id`, `reviewed_at` verbatim, even if both reviewer ids are null after deletion.
- [ ] State change: call the tracker epic's state-change helper (the one `PUT` uses for a different state) with the other supplied fields of `update` (title, description, priority, labels, parent, assignee) so a hand-off can carry ordinary field updates; it clears the lease, resets `attempts`, sets or clears `closed_at`, recomputes dependants' `blocked` and parent closure, and collects its events. Then `set_task_state_fields(tx, ..., current_handoff_id: Some(prepared.id))` (or pass it into the helper if it accepts it).
- [ ] Links: `touch_task_session(tx, task_id, prepared.source_session_id)` for a revision when the source session still exists; `touch_task_session(tx, task_id, caller session)` for `HandoffCaller::Session` (deduplicated when equal).
- [ ] Events appended in this order through `append_task_events`: the state-change helper's events (`state_changed` with `from`/`to` and `task` built with `handoff` = the new row, plus any `blocked`/`unblocked`/parent events), then `commented` with the comment DTO. No `released` event (the lease clears as part of the state change, not as a release). One `pg_notify('task_events', ...)` from the append primitive.
- [ ] Returns the full `Task` after the change; `.sqlx/` refreshed for any new query.

## Implementation notes
- Files: `orchestrator/src/tracker/handoffs.rs`; possibly a small extension of the tracker epic's state-change helper signature (an `Option<Uuid>` for `current_handoff_id` and a way to build the `Task` DTO after the pointer is set, since the `state_changed` payload must embed the new hand-off). If the helper builds the event payload internally before the pointer is set, set `current_handoff_id` first and then call the helper.
- `NewTaskHandoff::validate` (tracker models) rejects a decided status without a reviewer; a carried decision whose reviewer was deleted needs validation to allow "status != unreviewed with `reviewed_at` set and no reviewer" when the row is a carry. Add a `carried: bool` to the input or a `NewTaskHandoff::carried_from(prev)` constructor that bypasses only the reviewer-presence rule, with a unit test. Note this in the commit message as a model adjustment.
- Lock order inside the transaction: project row (already held by `begin_mutation`) -> task row (`FOR UPDATE`); no session row locks are needed (links are plain upserts).
- The `TaskEventActor` is `{ kind: "user", user_id }` or `{ kind: "session", session_id }` derived from `caller`, never from input.

## Edge cases
- Target state terminal: `closed_at` is set and dependants recompute; the hand-off is still recorded (closing with a final revision is allowed).
- Target state is the human state: the tracker helper decides whether that is an `escalated` event for a hand-off by a user; follow the tracker epic's rule (a user move into the human state is a `state_changed`, not an escalation, unless that epic specifies otherwise) and note the choice in the doc comment.
- Forward with `Decision` by a session: `reviewed_by_session_id` = caller; by a user: `reviewed_by_user_id` = caller. Never both.
- Recheck fails after the ref was pinned: return `Conflict`; the composition task discards the ref.
- Unique violation on `task_handoffs.id` (impossible unless the id was reused): surface `Error::Internal` with an `error!` log.

## Testing
- Integration tests in `orchestrator/tests/handoffs_transaction.rs` on `TestApp` (seed a ready project, states, a task held by a session, a session row; construct a `PreparedHandoff` directly, no git needed for the transaction itself): revision by the holding session writes one comment, one hand-off with `unreviewed`/no reviewer, `current_handoff_id` set, lease cleared, `attempts` 0, state moved, `task_sessions` row for the session, events `state_changed` (with `task.handoff.id`) then `commented`; revision by a user links the source session and uses `created_by_user_id`; forward with `approved` by a session sets `reviewed_by_session_id` and `reviewed_at`; forward without a decision copies status and attribution from the previous row (including after deleting the reviewer user); a new revision after an approved hand-off is `unreviewed` even with the same commit; recheck failure (change `state_id` between preparation and the call) -> `Conflict` and no rows written; a terminal target sets `closed_at` and emits `unblocked` on a dependant.
- `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Documentation
- `docs/data-model.md` `task_handoffs`: one sentence that a carried decision may have no reviewer id when the reviewer was deleted (already implied; make it explicit if the model rule needed relaxing).

## Assumes from other epics
- "Task tracker: states, tasks, leases, dependencies and events": the state-change helper used by `PUT` (lease clear, `attempts` reset, `closed_at`, blocked recompute, parent closure, event collection), the `Task` DTO builder, `TaskEventActor`, `TaskUpdateRequest`.
- "Database schema, models, repositories and test harness": `begin_mutation`, `find_task_for_update`, `insert_comment`, `insert_handoff`, `set_task_state_fields`, `touch_task_session`, `append_task_events`.