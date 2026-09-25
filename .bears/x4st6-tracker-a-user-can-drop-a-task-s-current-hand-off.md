---
id: x4st6
title: "Tracker: a user can drop a task's current hand-off"
status: open
priority: P1
created: "2026-09-25T21:03:46.405990395Z"
updated: "2026-09-25T21:03:46.405990395Z"
tags:
  - orchestrator
  - tracker
  - docs
parent: ny9yq
---

## Summary

Add a way for a user to clear `tasks.current_handoff_id`, so that the task's next launch starts from the project's default branch rather than from a hand-off that is known to be bad. The historical `task_handoffs` rows and their `refs/handoffs/<id>` refs stay; only the pointer is cleared. This is needed on its own for the manual recovery and is the building block for the rollback task.

## Acceptance Criteria

- [ ] `POST /projects/{pid}/tasks/{number}/drop-handoff` with body `{ comment: string }` (a non-empty comment is required, as for every hand-off change) returns 200 with the `Task`. Choose the exact path consistent with the existing task action routes in `SPEC.md` "Tasks" and "Code hand-offs and review"; follow the house style (verbs for actions).
- [ ] One tracker verb in `tracker/`, composed under `TrackerMutation`, used by the route and later by the rollback task. It:
  - clears `current_handoff_id`, and nothing else: state, lease, `attempts`, `rounds` and `closed_at` are unchanged;
  - writes the comment as a user comment;
  - emits `updated` and `commented` task events.

  A task with no current hand-off returns 409 `task has no current hand-off`.
- [ ] The verb takes a list of task ids too, or exposes an inner helper that accepts the mutation, so the rollback can drop hand-offs of many tasks inside one mutation.
- [ ] Not exposed over MCP.
- [ ] A held task is allowed; the holder keeps its lease. Its checkout is unchanged, as for any hand-off change a running session did not make.

## Docs (same commit)

- `SPEC.md`: the endpoint in "Tasks" or "Code hand-offs and review", and the event it emits in "TaskEvent" if the list names the triggers.
- `ARCHITECTURE.md` "Code hand-offs": one sentence saying a user may drop the current hand-off, and what that means for the next launch.
- `docs/data-model.md`: the paragraph that says what leaves `current_handoff_id` unchanged needs the new writer.

## Testing

Integration tests through HTTP (`TestApp`): the happy path, a task with no hand-off (409), an empty comment (400), unauthenticated (401), a non-member (403 or 404, as the tasks routes do), and a launch for the task afterwards using the default branch as base. Tracker tests go through the verb, never through row helpers (CLAUDE.md "Testing expectations").