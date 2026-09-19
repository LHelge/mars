---
id: cuw5s
title: "Implement the state-change core: hand-off semantics, current-state no-op, reopen, dependant recompute and automatic parent closure"
status: in_progress
priority: P1
created: "2026-09-16T20:42:19.808273182Z"
updated: "2026-09-19T10:33:55.344497344Z"
tags:
  - orchestrator
  - tracker
depends_on:
  - h8qkt
parent: "5h3y4"
attempts: 1
---

## Summary
Implement `tracker::state::change_state`, the single function that moves a task between states for users (`PUT`), agents (MCP `update`), the reaper and the system. It applies the hand-off rules (clear lease, reset `attempts`, set or clear `closed_at`), the current-state no-op, reopening, the `blocked` recompute on dependants and parent, and automatic parent closure into the lowest-position terminal state with actor `system`. The Code hand-offs epic later wraps this function with publication; the escalation paths call it with the human state as target.

## Documents
- `ARCHITECTURE.md` "Task tracker" → "The lease is the worker" (hand-off = different state; assigning the current state preserves lease, attempts and closure; a state change by the holder clears the lease in the same transaction), "Parents" (one level; when the last non-terminal child of a non-terminal parent enters a terminal state the same transaction moves the parent to the terminal state with the lowest position, clears its lease, writes `state_changed` with actor `system`; never reopened automatically), "Blocked is stored", "Attempts and escalation" (a hand-off resets `attempts`).
- `SPEC.md` "Tasks" (changing to a different state hands off: clears lease, resets `attempts`; terminal sets `closed_at` and unblocks dependants; non-terminal on a closed task reopens; current state is a no-op preserving lease, `attempts`, `closed_at`, emitting no state-change or escalation event; parent closed by system with `state_changed` actor `system`), "TaskEvent" (`state_changed` with `from`/`to` state names; `escalated` likewise plus `reason`; `blocked`/`unblocked`).
- `docs/data-model.md` `tasks` ("A **hand-off** changes to a different state..." paragraph and "A **parent** is blocked by its open children..." paragraph; `closed_at` set on entering terminal, cleared on leaving; `attempts` reset on every state change; plain moves leave `current_handoff_id` unchanged).
- ADRs 0016, 0021, 0023.

## Acceptance criteria
- [ ] `orchestrator/src/tracker/state.rs` exposes `pub async fn change_state(m: &mut TrackerMutation, task: &Task, target: &TaskState, opts: StateChangeOptions) -> Result<StateChangeResult>` where `StateChangeOptions { event: StateEventKind::StateChanged | StateEventKind::Escalated { reason: String }, needs_human_reason: Option<String> }` and `StateChangeResult { changed: bool, task: TaskDto }`.
- [ ] Same state (`task.state_id == target.id`): return `changed: false` with the unchanged task and emit nothing; lease, `attempts`, `closed_at` untouched (this is the no-op rule; callers still apply other field changes).
- [ ] Different state: one `UPDATE` sets `state_id = target`, `lease_holder_session_id = NULL`, `lease_since = NULL`, `attempts = 0`, `closed_at = CASE target.kind terminal → NOW() (keep existing if already set? no: entering terminal from non-terminal always sets NOW()) | otherwise NULL`, `needs_human_reason = opts.needs_human_reason` when `Some` (unchanged when `None`), `updated_at = NOW()`, scoped by `id AND project_id`; `current_handoff_id` is never touched here.
- [ ] After the update emit `state_changed` (or `escalated` with `reason`) with `from = old state name`, `to = target name` and the full task; then, if the terminal-ness changed (entered or left a terminal state), call `graph::recompute_blocked` on `graph::affected_by_state_change(task)` so dependants and the parent flip with `blocked`/`unblocked` events after the state event.
- [ ] Parent auto-closure: after a move into a terminal state, if the task has a parent whose state is non-terminal and the parent has no remaining non-terminal children, move the parent with a nested `change_state` call whose actor is forced to `TaskActor::System` (the mutation gains `with_actor(TaskActor, |m| ...)` or the emit helpers take an explicit actor) into the project's terminal state with the lowest `position`; the parent's lease is cleared by the same rule; the parent's own dependants are then recomputed (recursion depth is bounded to one level because nesting is one level).
- [ ] Moving a child out of a terminal state never reopens a terminal parent; the parent merely becomes `blocked` again through the recompute (which, for a terminal parent, is harmless).
- [ ] `pub async fn resolve_state(m, name: &str) -> Result<TaskState>` returns `Error::BadRequest(format!("unknown state \"{name}\"; valid states are: {list in position order}"))` for unknown names, reused by every route and tool.
- [ ] `touch_actor(task.id)` is recorded on every effective change.

## Implementation notes
- Files: `orchestrator/src/tracker/state.rs`; the generic `UPDATE` may be the repository's `set_task_state_fields` from the schema epic, extended if it lacks a field.
- Event order inside one call: `state_changed`/`escalated` for the task → `blocked`/`unblocked` for its dependants/parent → parent's `state_changed` (system) → parent's dependants' `blocked`/`unblocked`.
- `from`/`to` are names, so load the old state's name before the update (the task row only has `state_id`).
- Callers that also write a comment (release with reason, `needs_human`) emit `commented` before calling `change_state` so the comment precedes the state event in the stream.
- Escalation email is not sent here; the escalation tasks call `m.record_escalation` after `change_state` returns.

## Edge cases
- Target is the human state with `event = StateChanged` (a user drags a card into `needs_human` over REST): emit `state_changed`, no email, `needs_human_reason` unchanged unless given. Only the escalation paths use `Escalated`.
- Terminal → terminal (`done` → `cancelled`): `closed_at` is refreshed to `NOW()`, lease cleared, no `blocked` recompute needed (terminal-ness unchanged) and no parent closure (the parent already closed or was reopened by a user).
- A task with a lease moved by a user: the lease is cleared even though the user is not the holder (users are not bound by leases).
- The parent has other children still open: no closure; the parent's `blocked` stays true.
- A parent already in a terminal state when its last child closes: nothing happens.
- Reopening a task (`done` → `ready`) whose dependants were unblocked: they flip back to `blocked` with events.

## Testing
- Integration tests in `orchestrator/tests/tracker_state.rs` on the test pool (direct `TrackerMutation` use, no HTTP): move `ready` → `review` clears a lease and resets `attempts` from 2 to 0, emits one `state_changed` with `from: "ready", to: "review"`; same-state call returns `changed: false`, no event, lease intact; `ready` → `done` sets `closed_at`, emits `unblocked` for a dependant; `done` → `ready` clears `closed_at` and re-blocks the dependant; parent with two children: closing the first emits no parent event, closing the second emits the parent's `state_changed` with `actor: {kind: "system"}`, `to: "done"` (lowest-position terminal even when `cancelled` was renamed/reordered), parent lease cleared; reopening a child leaves the parent `done` and `blocked`; unknown state name → the exact `BadRequest` message listing valid names.
- `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Database schema, models, repositories and test harness": `set_task_state_fields`, `list_children`, `find_state`, `list_states`.