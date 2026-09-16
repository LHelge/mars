---
id: pg6ga
title: "Implement the update tool: holder and creator authority, state hand-off, current-state no-op, parent and blocks-dependency edits, and revision/forward hand-offs with the source session derived from the caller"
status: open
priority: P1
created: "2026-09-16T20:46:39.044440108Z"
updated: "2026-09-16T20:46:39.044440108Z"
tags:
  - orchestrator
  - mcp
  - tracker
  - git
depends_on:
  - "4cvu4"
parent: qgj33
---

## Summary
Implement `update`, the agent's hand-off tool. It applies field changes (`title`, `description`, `priority`, `labels`, `parent`, `add_depends_on`, `remove_depends_on`), moves the task to a different state (clearing the lease, resetting `attempts`, closing or reopening, recomputing dependants), treats the current state as a no-op, and optionally publishes a revision hand-off or forwards the current one through the hand-off epic's publication protocol (git lock and ref pin first, then the tracker transaction). The source session of a revision is always the caller; a supplied `source_session_id` is rejected. An update with no effective change writes nothing.

## Documents
- `SPEC.md` "MCP tool contracts" → `update` (full paragraph: caller must hold the lease except for `title`, `description`, `labels`, `add_depends_on`, `remove_depends_on` on tasks the caller created that nobody holds; unknown state → `invalid_argument` with the valid names; changing state releases the lease and resets `attempts`; terminal sets `closed_at` and recomputes `blocked`; current state preserves lease and counters; one-level parent rules → `invalid_argument`; hand-off inputs obey "Code hand-offs and review"; revision publication uses the calling session and needs no git-tool permission; `add_depends_on` creates `blocks`, cycle → `invalid_argument`; `remove_depends_on` removes only `blocks`).
- `SPEC.md` "Code hand-offs and review" (`HandoffInput` shapes; `handoff` requires a different target state and a non-empty comment; MCP omits `source_session_id`, a supplied one is rejected; `commit` is a full object id and sync must produce that exact tip or `conflict`; forwarding requires `handoff_id` to equal the current hand-off, else `conflict`; a new revision resets review to `unreviewed`; review actor from authenticated context), "Tasks" (hand-off semantics of `PUT`, no-op rule, one-level parent, `priority` 0–3), "TaskEvent" (`state_changed`, `updated`, `commented`, `dependency_added`, `dependency_removed`, `blocked`, `unblocked`).
- `ARCHITECTURE.md` "Task tracker" → "Code hand-offs" (publication protocol and lock order), "Review approval", "The lease is the worker", "Blocked is stored", "Parents"; "Git model" → "Serialization"; ADRs 0018, 0021, 0023, 0030.
- `docs/data-model.md` `tasks` (hand-off paragraph), `task_handoffs`, `task_dependencies` (identity includes `kind`; cycle check on `blocks` only), `task_sessions`.

## Acceptance criteria
- [ ] Validation before any lock: at least one field present, else `invalid_argument` `update requires at least one field`; `priority` via `validate_priority`; `handoff.validate()` (rejects `source_session_id`, empty comment, malformed commit); `handoff` present without `state` → `invalid_argument` `handoff requires a different target state`.
- [ ] Path A (no `handoff`): `begin_mutation`; `resolve_task_for_mutation`; authority: holder, or (`created_by_session_id == ctx.session_id` and `lease_holder_session_id IS NULL` and only the five creator-editable fields are present), else `conflict` `you do not hold this task`; `state` resolved by name within the project → unknown → `invalid_argument` `unknown state "<name>"; valid states: <names in position order, comma separated>`; the tracker service's `apply_update(tx, task, TaskPatch, actor = Session)` performs field updates, the state hand-off or no-op, parent re-nesting (one-level violations → `invalid_argument` with the tracker's message; parent not found → `not_found`), `blocks` edge additions (`add_depends_on` targets resolved by `TaskArg` in this project, unknown → `not_found`; self or cycle → `invalid_argument` `dependency would create a cycle`; existing edge → no event) and removals (missing edge → no event; other kinds untouched), emits the events, upserts the caller's link only when something changed; commit; output `TaskOutput`.
- [ ] `handoff` with `state` equal to the current state → `invalid_argument` `handoff requires a different target state` (checked after locking, before any git work is done only if the check can be made cheaply beforehand; the hand-off service re-checks under the lock).
- [ ] Path B (`handoff` present): delegate to the hand-off epic's `HandoffService::publish(project_id, task_ref, HandoffActor::Session(ctx.session_id), target_state_name, HandoffRequest::{Revision { commit, comment } | Forward { handoff_id, comment, review }}, TaskPatch)` which takes the project git lock, validates state/holder/current hand-off, syncs the caller's branch (revision) or reuses the current commit (forward), pins `refs/handoffs/<id>`, releases the git lock, then runs the tracker transaction that rechecks and writes the hand-off row, its comment, `current_handoff_id`, the state change, lease release, the patch, session links (caller and source) and events. The handler never takes the git lock itself and never holds a tracker transaction across git work. Errors: tip mismatch or unsynced commit → `conflict` (`commit <sha> is not the tip of your session branch; commit and try again` from the service); stale `handoff_id` → `conflict`; non-holder → `conflict` `you do not hold this task`.
- [ ] No-op: an update that supplies only values equal to the current ones (including `state` = current state) commits nothing: no `task_events` row, no `task_sessions` upsert, `updated_at` unchanged; output is the current task.
- [ ] Every rejection rolls back with no side effects; a hand-off rejection leaves the task, lease and `current_handoff_id` unchanged (an orphan `refs/handoffs/*` ref after a crash is the cleanup job's concern, not this task's).
- [ ] `tests/mcp_update.rs`: holder changes `title`+`priority` → `updated` event, link touched; non-holder non-creator → `conflict`; creator on an unheld task editing `labels` → OK, editing `state` → `conflict`; unknown state → `invalid_argument` listing all states in order; state `ready` → `review` by the holder → `state_changed` with `from`/`to`, lease cleared, `attempts = 0`; move to `done` → `closed_at` set and a dependant's `unblocked` event; move from `done` back to `ready` → `closed_at` cleared; current state with another field → `updated` only, lease and `attempts` preserved; `parent` set to a task that has a parent → `invalid_argument`; `parent: null` clears; `add_depends_on` forming a cycle → `invalid_argument`; `remove_depends_on` removes `blocks` but leaves a `discovered_from` edge for the same pair; `add_depends_on` with `"#7"`; identical values → no event and `updated_at` unchanged; revision hand-off to `review` with the work clone's HEAD commit → `task_handoffs` row with `source_session_id = caller`, `review_status = unreviewed`, `refs/handoffs/<id>` resolves to the commit, `current_handoff_id` set, lease cleared, events `commented` and `state_changed`; revision with a commit that is not the branch tip → `conflict` and the task unchanged; revision with `source_session_id` supplied → `invalid_argument` before any git work (no fetch-back event); forward by a reviewer session with `review: "approved"` → new row at the same commit, `reviewed_by_session_id = reviewer`, `source_session_id` = original; forward with a stale `handoff_id` → `conflict`; `handoff` without `state` or with the current state → `invalid_argument`; a profile with empty `mcp_tools` can publish a revision (no git permission needed).
- [ ] `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Implementation notes
- Files: `orchestrator/src/mcp/tools/update.rs`, `orchestrator/tests/mcp_update.rs`.
- Build one `TaskPatch { title, description, priority, labels, parent: Option<Option<Uuid>>, add_blocks: Vec<Uuid>, remove_blocks: Vec<Uuid>, state_id: Option<Uuid> }` for both paths so the same tracker code applies field changes; resolve `TaskArg`s to UUIDs inside the locked transaction for path A and let the hand-off service resolve them inside its transaction for path B (pass `TaskArg`-parsed `TaskRef`s, not pre-resolved ids, to avoid a check outside the lock).
- The valid-state list in the `invalid_argument` message is read inside the transaction from `task_states` ordered by `position`.
- Lock order: path B is git lock → (release) → project row → task row; path A is project row → task row. Never call `GitService` while holding a `begin_mutation` transaction.

## Edge cases
- `labels: []` clears labels (an effective change if labels were non-empty).
- `parent` pointing at the task itself → `invalid_argument`.
- Holder updating a task whose state was renamed concurrently: the name lookup happens under the project lock, so a rename either precedes it or follows it.
- Revision hand-off when the caller's work clone has uncommitted changes: only the committed tip counts; the sync ignores the work tree.
- Forward with `review` by the session that produced the revision: allowed by the documents (no self-review rule); do not add one.

## Testing
- Integration tests as listed; hand-off tests need the git epic's real-repository helper and the caller's work clone with commits.
- Command: `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Task tracker: states, tasks, leases, dependencies and events": `apply_update(tx, ...)` with `TaskPatch`, state resolution by name, parent and cycle validation messages, no-change detection, `dependency_added`/`dependency_removed`/`blocked`/`unblocked` emission.
- "Code hand-offs and review": `HandoffService::publish(...)` accepting a session actor, a `TaskPatch` and the `Revision`/`Forward` request, with the `conflict` messages for tip mismatch and stale forwards.
- "Git operations: mirror, clones, integration and REST API": the real-repository test helper.