---
id: thes7
title: "Add the TrackerMutation context: project-locked transaction, event collection, task_sessions touch and commit with notify"
status: done
priority: P0
created: "2026-09-16T20:40:33.723830560Z"
updated: "2026-09-19T09:33:18.837382368Z"
tags:
  - orchestrator
  - tracker
  - realtime
depends_on:
  - "4qqjs"
parent: "5h3y4"
attempts: 1
---

## Summary
Provide the one code path every tracker writer goes through: `TrackerMutation`, a struct that owns the project-locked transaction from `TaskRepository::begin_mutation`, records the acting `TaskActor`, collects `TaskEvent`s and `task_sessions` touches while domain helpers run, and on `commit()` appends the events (with `pg_notify('task_events', '<project_id>:<seq>')` inside the same transaction), upserts the session links and commits everything together. Rolling back publishes nothing. Later tasks (states, state changes, claims, dependencies, comments, deletion) are written as functions taking `&mut TrackerMutation`.

## Documents
- `ARCHITECTURE.md` "Task tracker" → "One mutation at a time per project" (lock project row first, validate after, helpers share the transaction, nothing broadcast before commit, keep transactions short: no engine, email, model or git work inside).
- `docs/data-model.md` "Tracker mutation transactions", `task_events` (append rule, notify, `deleted` keeps `task_id`), `task_sessions` (upsert on create/claim/change/comment/release/escalate/hand-off; preserve `first_touched_at`, advance `last_touched_at` only for an actual change), "Notifications (LISTEN/NOTIFY channels)".
- `SPEC.md` "MCP tool contracts" first paragraph (successful changes commit events and the calling session's link together; rejected and no-op operations write nothing).
- ADRs 0021, 0028, 0030.

## Acceptance criteria
- [ ] `orchestrator/src/tracker/mutation.rs` defines `pub struct TrackerMutation<'a> { tx: Transaction<'a, Postgres>, project_id: Uuid, project: Project, actor: TaskActor, events: Vec<NewTaskEvent>, touches: Vec<(Uuid /* task */, Uuid /* session */)>, escalations: Vec<Escalation> }` with `TrackerMutation::begin(pool: &PgPool, project_id, actor) -> Result<TrackerMutation>` (calls `TaskRepository::begin_mutation`, loads the locked project row so `max_attempts` is available; `NotFound` when the project is missing).
- [ ] `conn(&mut self) -> &mut PgConnection` exposes the transaction to repository helpers; `actor()`, `project()` and `project_id()` accessors exist.
- [ ] `emit(&mut self, kind, task_id: Option<Uuid>, payload: TaskEventPayload)` pushes a `NewTaskEvent` with `ts = Utc::now()`; convenience builders `emit_task(kind, &TaskDto)` (payload `{actor, task}`), `emit_state(kind, &TaskDto, from, to, reason)`, `emit_comment(&TaskDto, &CommentDto)`, `emit_deleted(task_id)` (payload `{actor}` only), `emit_states_changed(&[TaskState])` (`task_id = None`) fill `actor` from the mutation.
- [ ] `touch(&mut self, task_id, session_id)` records a `task_sessions` upsert; `touch_actor(&mut self, task_id)` records it only when the actor is a session (no-op for user and system actors).
- [ ] `record_escalation(&mut self, Escalation { project_id, task_id, task_number, task_title, assignee_user_id: Option<Uuid>, reason })` queues an email notification to be sent by the caller after commit; `commit()` returns `MutationOutcome { seqs: Vec<i64>, escalations: Vec<Escalation> }`.
- [ ] `commit(self) -> Result<MutationOutcome>`: if `events` is empty it still commits (row changes without events are legal for reads-with-locks) but issues no notify and performs no touches; otherwise it calls `TaskRepository::append_task_events` once with the whole batch (one notify with the highest seq), runs every recorded touch through `touch_task_session`, then commits. Dropping a `TrackerMutation` without `commit` rolls back.
- [ ] A helper `TrackerMutation::no_change(self)` explicitly rolls back and returns `Ok(())` for no-op operations so intent is visible at call sites.
- [ ] `tracker/mod.rs` documents the locking order in its module doc: "git lock (outside) → project row (`begin_mutation`) → session rows → task rows; never call engine, email, git or model code while a `TrackerMutation` is open".

## Implementation notes
- Files: `orchestrator/src/tracker/mutation.rs`, `orchestrator/src/tracker/mod.rs`, `orchestrator/src/tracker/escalation.rs` (only the `Escalation` struct here; sending is a later task).
- `Escalation` carries everything the email needs so the sender does not reopen the transaction: project name, task number and title, reason, assignee id.
- The order of `events` is the emission order; `append_task_events` allocates successive `seq` values so callers must emit in the documented order (e.g. `commented` before `state_changed` on a release with a comment, `state_changed` before dependants' `unblocked`).
- `TaskDto` payloads are loaded through `load_task_dto_in(conn, ...)` after the row change so the event carries the task after the change.
- Do not add a second `pg_notify`; `append_task_events` already issues it inside the transaction (ADR 0028).

## Edge cases
- `begin` on a project whose row is locked by another mutation waits; it never times out on its own (the pool's acquire timeout applies before the lock).
- Two `touch` calls for the same `(task, session)` pair in one mutation upsert once.
- `commit` failing on `append_task_events` (sequence collision) surfaces `Error::Internal` and rolls back the row changes with it.
- A mutation whose actor is `system` never produces touches even if `touch_actor` is called.

## Testing
- Integration tests in `orchestrator/tests/tracker_mutation.rs` on the test pool: `begin` + `emit_task` + `commit` writes one `task_events` row with the actor JSON and a `PgListener` on `task_events` receives exactly one `<project_id>:<seq>` after commit; dropping without commit leaves no row and no notification; two events in one mutation get consecutive seqs and one notification carrying the higher seq; `touch_actor` with a session actor creates the `task_sessions` row and a second mutation advances only `last_touched_at`; `touch_actor` with a user actor creates nothing; `no_change` leaves the table untouched; two concurrent `begin` calls on the same project serialise (the second returns only after the first commits) while one on another project proceeds immediately.
- `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Database schema, models, repositories and test harness": `TaskRepository::begin_mutation`, `append_task_events`, `touch_task_session`, `ProjectRepository` row loading, `Project` row type with `max_attempts` and `name`.