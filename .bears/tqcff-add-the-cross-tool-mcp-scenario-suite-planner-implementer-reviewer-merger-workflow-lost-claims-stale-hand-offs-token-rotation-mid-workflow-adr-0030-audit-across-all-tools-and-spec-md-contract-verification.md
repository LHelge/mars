---
id: tqcff
title: "Add the cross-tool MCP scenario suite: planner-implementer-reviewer-merger workflow, lost claims, stale hand-offs, token rotation mid-workflow, ADR 0030 audit across all tools, and SPEC.md contract verification"
status: in_progress
priority: P2
created: "2026-09-16T20:47:49.319413872Z"
updated: "2026-09-20T09:32:19.383030913Z"
tags:
  - orchestrator
  - mcp
  - tests
  - docs
depends_on:
  - y2nd8
  - "4cvu4"
  - pg6ga
  - "88zm4"
  - x458j
parent: qgj33
attempts: 1
---

## Summary
The per-tool tasks each test their own contract; this task proves the epic's acceptance criteria as a whole by driving a complete multi-agent workflow through the in-process `rmcp` server with four sessions of four profiles, and by auditing tracker side effects across every tool in one run (ADR 0030). It also walks `SPEC.md` "MCP tool contracts" and `ARCHITECTURE.md` "MCP design" sentence by sentence against the implementation and writes back any drift found, so the documents and the code agree when the epic closes.

## Documents
- Epic `qgj33` acceptance criteria (every tool through the in-process server with a `TestApp` bearer token; `conflict` on lost claims and stale hand-offs; `forbidden` for gated git tools; `invalid_argument` for unknown states, ambiguous provenance and cycles; 401/403 middleware; descriptions byte for byte; replaced token fails after relaunch).
- `SPEC.md` "MCP tool contracts" (all subsections), "Code hand-offs and review" (the worked example: implementer publishes commit A to `review`; reviewer forwards to `merge` with `approved`; merge uses A even after the branch advanced to B).
- `ARCHITECTURE.md` "MCP design" (all paragraphs), "Task tracker" → "State is a queue" (planner serves `backlog` and hands to `ready`; implementer serves `ready` and hands to `review`; reviewer serves `review` and hands to `merge` or back to `ready`; merger serves `merge` and closes).
- ADRs 0029, 0030.

## Acceptance criteria
- [ ] `tests/mcp_workflow.rs` (TestApp, real bare repositories via the git epic's helper, default task states): profiles `planner` (serves `backlog`, no git tools), `implementer` (serves `ready`), `reviewer` (serves `review`), `merger` (serves `merge`, `mcp_tools = ["list_session_branches", "merge", "push"]`); one seeded session per profile with a work clone for the implementer. Scenario: planner `create_task` epic in `backlog` → `claim` → `create_task` two children with `parent` (provenance inferred, no `discovered_from` edge because origin equals parent) → `update` each child to `ready` (planner holds them? no: children are unheld and planner created them, so use the creator path for `title` and then `claim` + `update` state) → `release` the epic; implementer `ready` lists the two children in priority order → `claim` first → commits A in the work clone → `update` to `review` with a revision hand-off → `ready` again shows only the second child; reviewer `ready` → `claim` → `get_task` shows the hand-off → `update` to `merge` with `forward` + `review: "approved"`; merger `ready` → `claim` → `merge` task form → `update` to `done`; assert the epic stays open (second child open), `main` contains commit A, and after the second child is closed the same way the epic closes automatically with a `state_changed` event with actor `system`.
- [ ] Lost claim: two implementer sessions call `claim` on the same task concurrently; exactly one `{ task }`, one `conflict` "task is not claimable"; exactly one `claimed` event.
- [ ] Stale hand-off: implementer publishes revision A, then revision B (moving the task back to `ready` via the reviewer's `changes_requested` forward and re-claiming); the reviewer's forward of A's `handoff_id` → `conflict`; merger's `merge` with A's id → `conflict`; with B's approved id → success.
- [ ] Gating: implementer calling `merge` → `forbidden`; the same session after the profile's `mcp_tools` gains `merge` (direct `UPDATE agent_profiles`) → allowed on the next call without reconnecting.
- [ ] Token rotation mid-workflow: after the implementer's claim, `rotate_token(implementer_session)` (relaunch path); the old client's next call fails with HTTP 401 at the transport; a new client with the new token continues and still holds the task.
- [ ] Ended session: mark the reviewer session `done` directly; its next call → HTTP 403.
- [ ] ADR 0030 audit across the run: a helper `tracker_fingerprint(pool, project_id) -> (task_events_count, task_sessions rows with timestamps, tasks updated_at map)` is captured before and after every read-only call (`ready`, `get_task`, `list_session_branches`) and every rejected call in the scenario, and asserted unchanged; after every successful mutation it is asserted changed for the directly changed task only, with `task_sessions.first_touched_at` preserved on repeat touches.
- [ ] Contract walk: a checklist in the test file's module doc lists every normative sentence of `SPEC.md` "MCP tool contracts" with the test that covers it (`tests/mcp_*.rs` names); any sentence without coverage gets a test here; any place where the implementation's message or behaviour differs from the document is fixed in code or, where the code is right and the document vague, written back into `SPEC.md`/`ARCHITECTURE.md` in the same commit (list the changes in the commit body).
- [ ] `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes; the suite runs in under two minutes on CI.

## Implementation notes
- Files: `orchestrator/tests/mcp_workflow.rs`, `orchestrator/tests/common/mcp.rs` (add `tracker_fingerprint` and a `WorkflowFixture` builder creating the four profiles and sessions), possibly `SPEC.md`/`ARCHITECTURE.md` write-backs.
- Reuse `McpClient`, `seed_mcp_session` and the git epic's repository helper; do not duplicate fixture code from earlier tests, move shared parts into `common/mcp.rs`.
- Commits in the implementer's work clone are made by shelling out to `git` in the test (git is never mocked).
- Keep each scenario as one `#[tokio::test]` so a failure names the step; share setup through the fixture builder.

## Edge cases
- The epic's automatic closure uses the terminal state with the lowest position (`done`); assert by name.
- The reviewer forwarding with `changes_requested` moves the task to `ready` with the hand-off intact and `review_status = changes_requested`; the implementer's next revision is `unreviewed`.
- Concurrency test must use two distinct `McpClient` connections (distinct MCP sessions), not two calls on one client.

## Testing
- This task is the test suite; command: `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- `SPEC.md` "MCP tool contracts" and `ARCHITECTURE.md` "MCP design": only drift corrections found by the contract walk; none expected beyond wording.

## Assumes from other epics
- "Session lifecycle: launcher, owner, recovery and sessions API": `rotate_token`.
- "Task tracker: states, tasks, leases, dependencies and events": automatic parent closure with actor `system`.
- "Code hand-offs and review": forward with `changes_requested`, `HandoffVerifier`.
- "Git operations: mirror, clones, integration and REST API": real-repository test helper.