---
id: md2zq
title: "Add launch-for-task: claim in the session insert transaction, hand-off base selection, MARS_TASK_ID, generated task message, lease release on failure and GET /sessions/{id}/tasks"
status: open
priority: P1
created: "2026-09-16T20:35:04.556129627Z"
updated: "2026-09-16T20:35:04.556129627Z"
tags:
  - orchestrator
  - sessions
  - tracker
depends_on:
  - tjccc
  - "9wxhs"
parent: s52qg
---

## Summary
Extend `POST /projects/{pid}/sessions` and the launcher with the optional `task_id`: the session row insert and the task claim commit in one tracker-locked transaction, `base_ref` defaults to the task's current hand-off commit (recording `handoff_id`), the title defaults to the task title, the container receives `MARS_TASK_ID`, the generated task message is the first input delivered (or the head of the ephemeral `-p` prompt), a launch that fails in `creating` releases the task, and `GET /sessions/{id}/tasks` lists the tasks a session touched. The claim statement and lease release come from the Task tracker epic; this task calls them.

## Documents
- `SPEC.md` "Sessions" (`task_id` on `POST`: 409 if the task is held, blocked or in a terminal state; the paragraph after the DTO: claim in the same transaction regardless of served states; first input is the generated message `You hold task #12: <title>. Call get_task to read it before starting.` followed by `message`; hand-off selection and claim atomic, `handoff_id` and `base_ref` = full commit id; `handoff_id` null with an explicit base or no hand-off; the generated message includes the current hand-off id, source branch, commit, review status and comment even when an explicit base overrides it; ephemeral: generated message and `message` joined into the `-p` prompt; title defaults to the task title), `GET /sessions/{id}/tasks` → `Task[]` touched by this session; "Tasks" (`Task` shape; `lease_holder_session_id`, `attempts`), "TaskEvent" (`claimed`, `released` with reason `session_ended`).
- `ARCHITECTURE.md` "Task tracker" → "Launching a session for a task" (both paragraphs: unheld, unblocked, non-terminal; human state qualifies; failure in `creating` releases when the session becomes `failed`; hand-off pinned commit selected under the same locked task row), "One mutation at a time per project" (project row lock before session row locks; git lock before the DB project lock; never wait for the git lock inside a tracker transaction), "Session container specification" (`MARS_TASK_ID` only when launched for a task), "Launch sequence" (generated task message flushed first).
- `docs/data-model.md` `sessions.task_id`, `sessions.handoff_id`, `tasks` (claim statement with `state_id = ANY(every non-terminal state)` for a UI launch), `task_sessions` (upsert when a session is launched for a task), `task_handoffs`.
- ADRs 0016, 0018, 0021.

## Acceptance criteria
- [ ] `POST /projects/{pid}/sessions` with `task_id` (UUID or per-project number, as elsewhere): resolve the task in this project (404 if absent); if `base_ref` is given, resolve it under the project git lock first and release the lock; then open one transaction: lock the project row (`SELECT ... FOR UPDATE`), lock the task row, re-check unheld, not `blocked`, non-terminal (409 `task is not claimable` otherwise); if `base_ref` was omitted and `tasks.current_handoff_id` is set, use `handoff.commit` as `base_ref` and record `handoff_id`; otherwise `base_ref` = explicit or `project.default_branch` and `handoff_id` null; `title` = explicit or `task.title`; insert the session row (with `task_id`, `handoff_id`) and run the tracker's `claim_for_launch(tx, task, session_id, actor = user)` which executes the atomic claim over all non-terminal states, writes the `claimed` `TaskEvent`, upserts `task_sessions` and issues the `task_events` notify; commit; then launch. Zero rows from the claim → rollback and 409.
- [ ] Generated message text: `You hold task #<number>: <title>. Call get_task to read it before starting.` and, when the task has a current hand-off, a second paragraph: `Current hand-off <handoff id> from session <source_session_id> on branch <source_branch> at commit <commit> (review: <review_status>). Hand-off comment: <comment body>`; when an explicit `base_ref` overrides the hand-off, append: `Your checkout starts from <base_ref>, not from the hand-off commit; fetch refs/handoffs/<id> before continuing that work.`
- [ ] Conversational: the launcher enqueues the generated message as a `QueuedInput { user_id: None, client_id: None }` before the user's `message`, so the owner records it as the first `user_message` (`user_id: null`) and writes it first after `init`.
- [ ] Ephemeral: `LaunchContext.prompt` = generated message + `"\n\n"` + `message` (or the generated message alone); nothing is queued.
- [ ] `MARS_TASK_ID=<task uuid>` is in the container env exactly when `session.task_id` is set, after `MARS_PROJECT_ID` and before the secrets.
- [ ] The `on_session_ended` / `on_session_failed` hooks call the tracker's `release_leases_for_session(session_id, reason = "session_ended")`, so a launch failing in `creating`, an `end`, an ephemeral `done`, and recovery's `creating → failed` all release the claim with a `released` event and a system comment as the tracker epic specifies.
- [ ] `GET /api/sessions/{id}/tasks` → 200 `Task[]` from `task_sessions` ordered by `last_touched_at DESC`; 404 unknown session. If the tracker epic already mounted this route, keep exactly one implementation.
- [ ] Ephemeral profile with `task_id` and no `message` is valid (400 only when both are missing).

## Implementation notes
- Files: `orchestrator/src/routes/sessions.rs`, `orchestrator/src/session/launcher.rs` (`LaunchMode::Fresh { token, first_message, task_message }`), `orchestrator/src/session/task_message.rs` (new: `pub fn generated_task_message(task: &Task, handoff: Option<&Handoff>, base_override: Option<&str>) -> String`), `orchestrator/src/prelude/state.rs` (install the tracker release in the hooks at startup).
- Lock order is fixed: project git lock (only for explicit `base_ref` resolution, released before the DB transaction) → project row → task row → session insert. Never resolve a ref or call the engine while the project row is locked.
- The hand-off's `comment` body is read through `task_comments` by `handoff.comment_id` inside the same transaction.
- `task_id` accepts a per-project number: reuse the tracker epic's task-reference parser.

## Edge cases
- The task's current hand-off is deleted between selection and launch: the FK `ON DELETE SET NULL` leaves `handoff_id` null while `base_ref` keeps the commit; the launcher resolves the commit id directly.
- `base_ref` omitted, hand-off present but its commit is missing from the mirror (should not happen; refs/handoffs are retained): the launcher fails the session with the base-ref error and the lease is released by the failure hook.
- A task in the project's `human` state qualifies for launch.
- A blocked task or one with a lease → 409 even when the caller is an admin.
- Session deleted later: `task_sessions` rows cascade; `tasks.lease_holder_session_id` is set null by the FK, but the service's `delete` only runs on `done`/`failed` sessions whose leases were already released.

## Testing
- Extend `orchestrator/tests/sessions_api.rs` (TestApp with a ready project, task states and a task): launch with `task_id` → 201, task has `lease_holder_session_id` = session, `attempts` = 1, a `claimed` event, a `task_sessions` row, `Session.task_id` set, title = task title; by per-project number too; 409 for a held task, a blocked task, a terminal task; task in `needs_human` succeeds; task with a current hand-off and no `base_ref` → `handoff_id` set and `base_ref` = the 40-hex commit, and the launched clone is at that commit; explicit `base_ref` → `handoff_id` null and the override sentence in the generated message; the first `user_message` event has `user_id: null` and the exact generated text, the mock stdin receives it before the user's message; ephemeral launch: the recorded command's `-p` prompt equals generated + blank line + message; `MARS_TASK_ID` present in the recorded env only for task launches; a launch that fails (image pull failure in the mock) releases the lease with a `released` event, reason `session_ended`; `GET /sessions/{id}/tasks` returns the task.
- `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Documentation
- `SPEC.md` "Sessions": record the exact generated-message format for the hand-off paragraph and the base-override sentence (the document names the content but not the wording).

## Assumes from other epics
- "Task tracker: states, tasks, leases, dependencies and events": `claim_for_launch(tx, ...)` (claim over all non-terminal states, `claimed` event, `task_sessions` upsert, notify), `release_leases_for_session(session_id, reason)`, the task-reference parser (UUID or number), `TaskRepository::list_touched_by_session`, and the `Task`/`Handoff` types.
- "Code hand-offs and review": `task_handoffs` rows and `refs/handoffs/<id>` retention (only read here).