---
id: rz6bj
title: "Add end-to-end launch-for-task tests through published hand-offs: handoff_id and commit base_ref, explicit base override, generated message disclosure"
status: done
priority: P2
created: "2026-09-16T20:44:27.350431730Z"
updated: "2026-09-19T23:34:22.145707670Z"
tags:
  - orchestrator
  - sessions
  - tracker
  - tests
depends_on:
  - tgc55
parent: xjaah
attempts: 1
---

## Summary
The Session lifecycle epic implements launch-for-task, including the hand-off default for `base_ref`, against hand-off rows seeded directly. This task proves the epic's third acceptance criterion with hand-offs that were published through the real protocol: launching for a task after a revision records `handoff_id` and the 40-hex commit as `base_ref` and clones at that commit; an explicit `base_ref` overrides it with `handoff_id` null and the disclosure sentence in the generated message; the message discloses id, branch, commit, review status and comment. Any wiring gap found (for example the hand-off comment lookup) is fixed here.

## Documents
- `SPEC.md` "Sessions" (`POST /projects/{pid}/sessions` with `task_id`; if `base_ref` is omitted and the task has a hand-off, selection and claim happen atomically, `handoff_id` records that hand-off and `base_ref` its full commit id; otherwise `handoff_id` is null; the generated message includes the current hand-off id, source branch, commit, review status and comment, including when an explicit base overrides it; `Session.handoff_id`).
- `ARCHITECTURE.md` "Task tracker" -> "Launching a session for a task" (second paragraph: pinned commit selected under the same locked task row; explicit `base_ref` overrides and confers no approval; an already-running session that calls `claim` gets the hand-off through the returned task but its checkout is unchanged).
- `ARCHITECTURE.md` "Git model" -> "Session clone" (commit-id bases are accessible through alternates; supported bases include commit ids present in the project repository).
- `docs/data-model.md` `sessions.handoff_id`, `sessions.base_ref`, `task_handoffs` ("Session launch selects and records the current hand-off in the same transaction as claiming the task, and persists the chosen commit in `sessions.base_ref`").
- ADR 0018.

## Acceptance criteria
- [ ] Test: publish revision A (through `PUT .../tasks/{id}`), forward it with `approved`, then `POST /projects/{pid}/sessions {profile_id, task_id}` -> 201 with `handoff_id` = the forwarded (current) hand-off id and `base_ref` = A's commit; the launched work clone (mock engine, real git) has `HEAD` at A and branch `session/<new sid>`; the task is held by the new session with `attempts == 1`.
- [ ] Test: same with `base_ref: "main"` -> `handoff_id == null`, `base_ref == "main"`, and the first `user_message` event contains the hand-off paragraph plus the override sentence naming `refs/handoffs/<id>`.
- [ ] Test: the generated message's review status reflects the current row (`approved` after a forward, `unreviewed` after a new revision) and the comment body is the hand-off's comment, not the latest task comment.
- [ ] Test: a task whose current hand-off was superseded between reading the task and launching (publish a new revision first) launches from the new current hand-off, never the stale one (selection happens under the task row lock).
- [ ] Test: after the session ends (`POST /sessions/{id}/end`), the lease is released and the task's `handoff` is unchanged (ending a session does not touch the hand-off).
- [ ] Any deviation found in the launcher's hand-off handling is fixed in `orchestrator/src/session/` in this task, with the test that exposed it.

## Implementation notes
- File: `orchestrator/tests/handoffs_launch.rs` (new), using the mock engine from `TestApp` and the real-repository helper; the launcher runs the real git clone against the mirror.
- Read the generated message from the session's `events` (`user_message` with `user_id: null`) or from the mock engine's recorded stdin/prompt, whichever the sessions epic exposes.
- The ephemeral variant (message in the `-p` prompt) needs one assertion on the recorded launch command.

## Edge cases
- Hand-off present but its commit missing from the mirror cannot happen after publication (refs retain objects and gc is off); no test.
- `base_ref` equal to the hand-off commit given explicitly: `handoff_id` is still null (an explicit base is an override by definition); assert.

## Testing
- The scenarios above, one `#[tokio::test]` each.
- `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Documentation
- none unless a gap is found; then the sessions section of `SPEC.md` is corrected in the same commit.

## Assumes from other epics
- "Session lifecycle: launcher, owner, recovery and sessions API": launch-for-task (`handoff_id` selection, generated message, `MARS_TASK_ID`), `POST /sessions/{id}/end`, the mock-engine launch recording.
- "Task tracker: states, tasks, leases, dependencies and events": `claim_for_launch`, `release_leases_for_session`.