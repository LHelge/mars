---
id: tgc55
title: Wire handoff into PUT /projects/{pid}/tasks/{id} and add the REST publication tests with real bare repositories
status: open
priority: P1
created: "2026-09-16T20:42:54.014236664Z"
updated: "2026-09-16T20:42:54.014236664Z"
tags:
  - orchestrator
  - tracker
  - git
  - tests
depends_on:
  - "89kct"
  - gnzhv
parent: xjaah
---

## Summary
Extend the tracker epic's `PUT /projects/{pid}/tasks/{id}` handler so that a body with `handoff` is routed to `HandoffService::update_with_handoff` with `HandoffCaller::User`, and add the integration suite that proves the epic's first acceptance criterion over HTTP: revision publication pins the commit, a tip mismatch returns 409 with the task unchanged, forwarding with and without a decision, a new revision resets review, a stale `handoff_id` is rejected. Without `handoff` the handler behaves exactly as the tracker epic defined.

## Documents
- `SPEC.md` "Tasks" (`PUT /projects/{pid}/tasks/{id}` body with `handoff?: HandoffInput` -> `Task`; `{id}` accepts UUID or number; a user may set any state and is not bound by leases; changing to a different state hands the task off), "Code hand-offs and review" (400 for a missing or unchanged target state and for an empty comment; REST revision requires `source_session_id` belonging to the project; 409 on tip mismatch; 409 on a stale `handoff_id`; new revision resets to `unreviewed`; old approvals stay in history; the example flow review -> merge -> rejection), "TaskEvent".
- `ARCHITECTURE.md` "Task tracker" -> "Code hand-offs", "Review approval".
- `CLAUDE.md` "API conventions", "Testing expectations" (every endpoint: happy path, unauthenticated, forbidden, validation, conflict; git tests use real bare repositories).

## Acceptance criteria
- [ ] In `routes/tasks.rs`, `update_task` deserialises `handoff: Option<HandoffInput>`; when `Some`, it calls `HandoffService::update_with_handoff(pid, task_ref, update, handoff, HandoffCaller::User { user_id })` and returns 200 `Task`; when `None`, the existing path runs unchanged.
- [ ] Statuses: 400 when `state` is absent or equals the current state (`handoff requires a different target state`), 400 empty comment, 400 missing `source_session_id`, 400 source session of another project, 400 short/uppercase commit, 401 without a token, 404 unknown task or project, 409 tip mismatch, 409 stale `handoff_id`, 409 project not `ready`.
- [ ] A successful revision response has `handoff.commit` equal to the requested commit, `handoff.review_status == "unreviewed"`, `lease_holder_session_id == null`, `attempts == 0`, `state` equal to the target; the mirror has `refs/handoffs/<handoff.id>` at that commit; `GET .../tasks/{id}` shows one `handoffs` entry and a comment with `id == handoff.comment_id`.
- [ ] Forward with `review: "approved"` yields a new hand-off id, same `commit` and `source_branch`, `reviewed_by_user_id` = caller, `reviewed_at` set; the previous row is unchanged in history. Forward without `review` after that carries `approved` and the same reviewer. A subsequent revision (even with the same commit) is `unreviewed` with no reviewer, and history has all rows oldest first.
- [ ] The task event stream (read from `task_events` or the SSE endpoint if available) shows `state_changed` with `task.handoff.id` and then `commented` for each publication, and nothing for rejected requests.

## Implementation notes
- Files: `orchestrator/src/routes/tasks.rs` (handler branch), `orchestrator/tests/handoffs_api.rs` (new suite), test helper in `orchestrator/tests/common/` for "ready project with a real mirror, one launched-looking session row with a work clone that has N commits" (reuse the git epic's helper; add a `commit_in_work_clone(path, file, content) -> sha` helper if missing).
- The `Task` DTO in the response comes from the tracker builder; no DTO code here.
- Keep the handler thin: parse, dispatch, map; the service owns ordering.

## Edge cases
- `handoff` plus other fields (`title`, `labels`) in one request: fields apply and the response reflects them.
- `handoff` with `state` naming the human state: allowed; recorded as a state change by the user.
- `handoff` on a task that nobody holds, by a user: allowed (users are not lease-bound), source session may be any project session with a synced branch.
- `handoff` on a task held by session A with `source_session_id` = session B: allowed for a user; the lease on A is cleared by the state change (document in the test name).
- Task referenced by per-project number in the URL: works.

## Testing
- `orchestrator/tests/handoffs_api.rs`, one `#[tokio::test]` per scenario listed above, asserting with `response.assert_status()` and `response.json::<Task>()`, and reading the mirror with `git::refs::resolve(GitRef::Handoff(id))`. Include: "tip mismatch leaves task, lease, attempts and refs unchanged"; "forward with approved then forward without decision carries attribution"; "new revision after approval is unreviewed"; "stale handoff_id after a forward is 409"; "forward works after the source session row is deleted".
- `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Documentation
- `SPEC.md` "Code hand-offs and review": state explicitly that REST users are not lease-bound for hand-offs and may name any project session with a synced branch as the source (implied by "Tasks" today; make it explicit in the hand-off section).

## Assumes from other epics
- "Task tracker: states, tasks, leases, dependencies and events": `routes/tasks.rs` with the `PUT` handler and `TaskUpdateRequest`.
- "Authentication, users, invites and email": JWT extractor and `TestApp` login helpers.
- "Git operations: mirror, clones, integration and REST API": the real-repository test helper and `create_work_clone`.