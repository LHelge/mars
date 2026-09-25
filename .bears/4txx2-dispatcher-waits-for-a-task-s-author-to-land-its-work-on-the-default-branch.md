---
id: "4txx2"
title: Dispatcher waits for a task's author to land its work on the default branch
type: epic
status: open
priority: P1
created: "2026-09-25T21:00:10.184093257Z"
updated: "2026-09-25T21:00:10.184093257Z"
tags:
  - orchestrator
  - frontend
  - dispatcher
  - git
---

## Scope

A conversational session wrote planning documentation and filed tasks against it, but its commits stayed on `session/<sid>`. The dispatcher then launched implementers from the default branch, which did not have those documents. The work they produced, reviewed and auto-merged was built on the wrong base.

The fix is to connect a task with the work of the session that wrote it (`tasks.created_by_session_id`). The dispatcher does not launch a task until that session's commits are on the default branch. People are told why the task is waiting, and ending a session says when its commits have not reached the default branch.

Rejected alternatives, which go in the ADR:
- **Prompt only at session end.** Conversational sessions usually park rather than end, and dispatch happens while the planner is still open.
- **Hold auto-merge while other branches are ahead.** Almost every live session is ahead of the default branch, so this would stall the pipeline. It also acts at the wrong point: the damage is the launch base, not the merge.

## Acceptance Criteria

- [ ] The dispatcher skips a task whose author session has commits not contained in the default branch, and dispatches it once they land.
- [ ] The board shows why such a task is waiting and offers the way to land the work.
- [ ] Ending a session with commits not on the default branch says so in the End confirmation.
- [ ] ARCHITECTURE.md "Dispatcher", SPEC.md "Frontend", an ADR, and the E2E coverage table are updated.