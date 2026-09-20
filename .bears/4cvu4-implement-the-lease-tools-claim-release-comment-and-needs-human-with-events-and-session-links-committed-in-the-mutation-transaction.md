---
id: "4cvu4"
title: Implement the lease tools claim, release, comment and needs_human with events and session links committed in the mutation transaction
status: done
priority: P1
created: "2026-09-16T20:45:14.375442189Z"
updated: "2026-09-20T08:19:04.580611529Z"
tags:
  - orchestrator
  - mcp
  - tracker
depends_on:
  - h9u3c
parent: qgj33
attempts: 1
---

## Summary
Implement the four tools that change who holds a task or add to its conversation: `claim` (atomic claim restricted to the profile's served states), `release` (give back with a reason, escalating at `max_attempts`), `comment` (any session in the project) and `needs_human` (escalate to the human state, including the already-in-human-state variant). Each runs inside one tracker mutation transaction that locks the project row, and commits its `TaskEvent` rows and the caller's `task_sessions` link together; rejections write nothing. The shared helpers this task creates (`resolve_task_for_mutation`, `require_holder`, `task_output`) are reused by `update` and `create_task`.

## Documents
- `SPEC.md` "MCP tool contracts" → `claim` (`conflict` "task is not claimable" on zero rows; `conflict` "task is not in a state this profile serves"; returned task includes its hand-off; claiming never resets the checkout), `release` (caller must hold the lease; reason recorded as a comment; at `max_attempts` the task moves to the `human` state with `needs_human_reason` and the output shows that state), `comment` (any session in the project; output `{ comment }`), `needs_human` (moves to the human state, sets `needs_human_reason`, releases the lease, resets `attempts`; requires holding the lease or the task being unheld; already in the human state → record the reason, release any held lease, preserve `attempts`, emit `commented`, `updated` and when applicable `released`, no `escalated` and no email), intro paragraph (successful changes commit events and links together; rejections leave history unchanged).
- `SPEC.md` "TaskEvent" (`claimed`, `released` with reason `given_back`, `escalated` with `from`/`to` and reason, `commented`, `updated`; actor `{ kind: "session", session_id }`).
- `ARCHITECTURE.md` "Task tracker" → "One mutation at a time per project", "The lease is the worker", "Attempts and escalation" (`attempts` counts claims since the last state change; a claim increments, a hand-off resets; release at `max_attempts` escalates with an `escalated` event), "Notification" (every move into the human state sends one email to the assignee or all admins honouring `notify_email`), "MCP design" → "Side effects".
- `docs/data-model.md` `tasks` (the claim statement with `state_id = ANY($4)`, the release paragraph), `task_comments` (exactly one author column; `author_session_id` here), `task_sessions` (upsert on claim, comment, release, escalate; preserve `first_touched_at`), `task_events`, `projects.max_attempts`; ADRs 0009 (claim mechanism), 0016, 0021, 0023, 0030.

## Acceptance criteria
- [ ] `orchestrator/src/mcp/tools/common.rs`: `pub async fn resolve_task_for_mutation(tx: &mut PgConnection, ctx: &SessionContext, arg: &TaskArg) -> McpResult<TaskRow>` (parse, resolve within `ctx.project_id`, `SELECT ... FOR UPDATE` on the task row after the project lock; `not_found` "task not found"); `pub fn require_holder(task: &TaskRow, ctx) -> McpResult<()>` (`conflict` with message `you do not hold this task` when `lease_holder_session_id != Some(ctx.session_id)`); `pub async fn task_output(tx, project_id, task_id) -> McpResult<TaskOutput>` loading the REST `Task` shape (with `handoff`, `depends_on`, `blocks`) inside the transaction before commit.
- [ ] `claim::handle`: `begin_mutation(project_id)`; resolve the task; read served state ids for `ctx.profile.id` inside the transaction; if the task's `state_id` is not served → `conflict` "task is not in a state this profile serves" (rollback); else the tracker service's `claim(tx, project_id, task_id, session_id, &served_state_ids, actor = Session)` which runs the atomic UPDATE, appends `claimed`, upserts the link and notifies; zero rows → `conflict` "task is not claimable" (rollback); commit; output `TaskOutput` with `attempts` incremented and `lease_holder_session_id = ctx.session_id`.
- [ ] `release::handle`: `non_empty("reason")`; `begin_mutation`; resolve; `require_holder`; tracker service `release_by_agent(tx, task, session_id, reason)` which inserts the comment (author session, body = reason), clears the lease, appends `commented` and `released { reason: "given_back" }`, or, when `attempts >= projects.max_attempts`, moves the task to the human state with `needs_human_reason = reason` and appends `escalated { from, to, reason }` plus the tracker's system comment; commit; then (outside the transaction) the tracker's escalation email hook when escalated; output the resulting task.
- [ ] `comment::handle`: `non_empty("body")`; `begin_mutation`; resolve (no holder check); insert the comment with `author_session_id = ctx.session_id`; append `commented { comment }`; upsert the link; commit; output `CommentOutput` with the REST `Comment` shape.
- [ ] `needs_human::handle`: `non_empty("reason")`; `begin_mutation`; resolve; if held by another session → `conflict` "task is held by another session"; if the task is already in the human state → tracker's `record_needs_human_again(tx, task, session_id, reason)` (comment, `needs_human_reason` update, lease release if the caller holds it, events `commented`, `updated`, and `released { reason: "given_back" }` when a lease was cleared; `attempts` untouched; no `escalated`, no email); otherwise tracker's `escalate(tx, task, actor = Session, reason)` (state → human, `needs_human_reason`, lease cleared, `attempts = 0`, `escalated` event with `from`/`to`, comment with the reason and its `commented` event, link upsert); commit; email hook after commit for the escalation case only; output the task.
- [ ] Every rejection (`not_found`, both `conflict` cases, empty strings) rolls back and leaves `task_events`, `task_sessions`, `tasks.updated_at` and `attempts` unchanged (asserted in tests).
- [ ] Nothing is done to the session's checkout on `claim`; the returned `task.handoff` carries the current hand-off so the agent can fetch `refs/handoffs/<id>`.
- [ ] `tests/mcp_lease_tools.rs`: claim happy path (event `claimed` with actor session, link row, `attempts = 1`, hand-off present when the task has one); second session's claim → `conflict` "task is not claimable"; claim of a `backlog` task by a profile serving `ready` → the served-state message; claim of a blocked task → "not claimable"; two concurrent claims from two `McpClient`s (`tokio::join!`) → exactly one success and one conflict, exactly one `claimed` event; release by non-holder → `conflict`; release happy path → lease cleared, comment authored by the session, `released` reason `given_back`, state unchanged; release with `attempts == max_attempts` → state is the human state, `needs_human_reason` set, `escalated` event, one email captured by the mock email client (assignee case and all-admins case with one user opted out); release with empty reason → `invalid_argument`; comment by a non-holder session → 200 with `commented` event and a link row; needs_human on a held-by-other task → `conflict`; on an unheld task → escalation with events and email; on a task already in the human state held by the caller → `commented`, `updated`, `released`, no `escalated`, no email, `attempts` preserved; ADR 0030 audit: rejected calls change no counts.
- [ ] `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Implementation notes
- Files: `orchestrator/src/mcp/tools/common.rs`, `orchestrator/src/mcp/tools/claim.rs`, `release.rs`, `comment.rs`, `needs_human.rs`, `orchestrator/tests/mcp_lease_tools.rs`.
- Locking order (ADR 0021): `begin_mutation` takes the project row first; then the task row; no git lock is involved in these four tools; email is sent after commit, never inside the transaction.
- The tracker epic owns the event payload assembly (`Task` after change, `from`/`to` names, `reason`), the escalation comment text and the email; these handlers only decide which tracker operation to call and translate its errors (`Error::Conflict` → `conflict` with the tracker's message, unless the documented MCP message differs, in which case the handler constructs `McpError` itself).
- Actor for every event is `{ kind: "session", session_id: ctx.session_id }`.

## Edge cases
- `claim` on a task the caller already holds → the atomic UPDATE returns zero rows → "task is not claimable" (the `ready` description already tells the agent to use `get_task` instead).
- `claim` on a task in a terminal or human state → not served → the served-state message.
- `release` when `projects.max_attempts` was lowered below the current `attempts` → escalates (`>=`, not `==`).
- `needs_human` on a task in a terminal state → the tracker's rules decide; expect `conflict` "task is closed" from the tracker service and pass it through.
- Session ended between middleware and commit: the transaction still commits (the middleware check is per request); the reaper releases the lease later.

## Testing
- Integration tests as listed, with the mock email client's captured messages for escalation.
- Command: `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Task tracker: states, tasks, leases, dependencies and events": a tracker service exposing `claim(tx, ...)` over served states, `release_by_agent(tx, ...)` with escalation, `escalate(tx, ...)`, `record_needs_human_again(tx, ...)`, comment insertion with the `commented` event, `upsert_task_session`, the `Task` loader, `projects.max_attempts`, and the escalation email hook; `TaskRepository::resolve`.
- "Authentication, users, invites and email": mock `EmailClient` capture and `users.notify_email`.