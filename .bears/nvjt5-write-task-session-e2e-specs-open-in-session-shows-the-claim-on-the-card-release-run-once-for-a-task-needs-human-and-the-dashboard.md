---
id: nvjt5
title: "Write task-session E2E specs: open in session shows the claim on the card, release, run once for a task, needs_human and the dashboard"
status: done
priority: P1
created: "2026-09-16T20:46:53.071282204Z"
updated: "2026-09-20T20:10:16.673792685Z"
tags:
  - frontend
  - tracker
  - sessions
  - tests
depends_on:
  - "2acdq"
  - "7m22f"
parent: "6s8j7"
attempts: 1
---

## Summary
Cover the sentences that join tasks and sessions: launching a session for a task from the drawer ("open in session") claims the task, the card shows the holding session with a link and the session view shows the task in its side panel with the generated first message in the transcript; releasing from the drawer clears the claim; ending the session releases it; "run once" launches an ephemeral profile for the task; a task moved into `needs_human` appears on the dashboard together with running and parked sessions across projects. Escalation email delivery is asserted through the orchestrator log.

## Documents
- `SPEC.md` "User-facing features", "Sessions" (optionally for one task: the session starts holding it and the card links to the session; one click from the drawer), "Task board" (release held tasks, open a task in a new session, live changes), "Dashboard" (running and parked sessions across projects and every task in a human state).
- `SPEC.md` "Sessions" table (`POST /projects/{pid}/sessions` with `task_id` claims the task in the same transaction regardless of served states; 409 if the task is held, blocked or terminal; generated first message `You hold task #12: <title>. Call get_task to read it before starting.` followed by `message`; title defaults to the task's title; for an ephemeral profile the generated message and `message` form the `-p` prompt; `GET /sessions/{id}/tasks`).
- `SPEC.md` "Tasks" (`POST .../release` → `Task`, 409 if nobody holds it; a user release keeps the state and never escalates; `PUT` with `state` `needs_human` moves it; `lease_holder_session_id`, `lease_since`, `attempts`), "TaskEvent" (`claimed`, `released` with `reason: "user" | "session_ended"`, `escalated`).
- `SPEC.md` "Frontend", "Task board" (cards show the holding session with a link, attempts when above one; drawer actions move, release, "open in session" picks a conversational profile defaulting to the first that serves the task's state, "run once" with an ephemeral profile; session view shows the task it was launched for and the tasks it touched in a side panel), "Dashboard" (`GET /sessions?state=running`, `?state=parked`, `GET /tasks?state_kind=human`, 30-second refetch).
- `ARCHITECTURE.md` "Background jobs" (stuck-task reaper releases tasks held by `done`/`failed` sessions), "Session lifecycle" (`done`/`failed` release held tasks).
- `README.md` "Operating notes" (escalations to `needs_human` email the assignee or every admin; logged when `RESEND_API_KEY` is unset).

## Acceptance criteria
- [ ] `frontend/tests/task-sessions.spec.ts`; user, bare repo, ready project per test; `test.setTimeout(180_000)`; sessions ended in `afterEach`.
- [ ] `open in session claims the task`: task `Implement greeting` in `ready`; drawer → "Open in session" → profile picker defaults to `default` (serves `ready`); confirm → navigates to `/sessions/:id`; the session title is `Implement greeting`; the transcript's first user message starts with `You hold task #1: Implement greeting.`; the side panel shows the task; back on the board the card shows the holder link to the session and the drawer's release action is enabled; `getTask` shows `lease_holder_session_id` = session id and `attempts` 1.
- [ ] `a second launch for a held task is refused`: "Open in session" again on the same task → the 409 message; no second session in the sessions tab.
- [ ] `release from the drawer clears the claim without escalating`: click Release → card no longer shows a holder; state still `ready`; `attempts` unchanged; the session keeps running.
- [ ] `ending the session releases its task`: claim again with a new session; End it → within 10 s the card shows no holder and the drawer history/comments show the `session_ended` release (a system comment if the tracker writes one); the drawer's sessions list links to the ended session's transcript.
- [ ] `run once for a task`: ephemeral profile `oneshot`; drawer → "Run once" → session view without composer, transcript starting with the generated task message, ends `done`; the task is released when the session ends.
- [ ] `base override is disclosed`: with no hand-off the launch dialog shows base `main` and no override notice (the hand-off variant lives in the hand-off spec).
- [ ] `needs_human shows on the dashboard and emails`: `off = logOffset()`; assign the task to the user (`assignee_user_id`) and move it to `needs_human` from the drawer; `/` lists it under human-state tasks with a link to `/projects/:id/tasks/1`; the orchestrator log after `off` contains a record with the user's email and subject `Task #1 needs a human: Implement greeting` (helper: `readLoggedText(email, off)` or extend `readLoggedLink` with a `subject` mode); a user with `notify_email: false` receives no such record (second project, second user, assert absence after 5 s).
- [ ] `dashboard lists running and parked sessions across projects`: two projects, one running and one parked stub session; `/` shows both rows with project names and links to `/sessions/:id`; ending one removes it within the 30-second refetch (or trigger a refetch by navigating away and back).

## Implementation notes
- Files: `frontend/tests/task-sessions.spec.ts`; small helper addition in `tests/utils/` for reading a logged email by subject if `readLoggedLink` does not fit.
- The stub answers the generated task message with fixture turn 1; asserting the transcript's first user message text is enough to prove the claim message was delivered.
- Reaper timing: the stuck-task reaper runs every minute, but session end releases immediately in the same flow (`ARCHITECTURE.md` "Session lifecycle": `done` releases held tasks); assert within 10 s and fall back to a 70 s wait with a comment if the implementation relies on the reaper.
- Dashboard refetch is 30 s; navigate away and back to force a fresh query rather than waiting.

## Edge cases
- "Open in session" for a `blocked` or terminal task must be disabled or return the 409 text; assert disabled for a `done` task.
- The `Run once` control is hidden when the project has no ephemeral profile; create `oneshot` first.
- Escalation email records contain the task link `http://localhost:5173/projects/<pid>/tasks/1`; no secret can appear in them.

## Testing
- The spec file; run twice against one stack.
- Command: `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:e2e`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Frontend task board, task detail and hand-off controls": drawer actions "Open in session", "Run once", Release, holder link on cards, sessions list in the drawer; "Frontend foundation": `DashboardPage`; "Frontend project and session views": task side panel.
- "Task tracker": claim on launch, release on end, escalation email through `EmailClient`; "Session lifecycle": `task_id` launch and generated message.