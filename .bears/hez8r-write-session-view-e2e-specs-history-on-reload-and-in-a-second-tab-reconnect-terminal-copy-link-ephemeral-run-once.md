---
id: hez8r
title: "Write session view E2E specs: history on reload and in a second tab, reconnect, terminal, copy link, ephemeral run-once"
status: open
priority: P2
created: "2026-09-16T20:45:46.180711422Z"
updated: "2026-09-16T20:45:46.180711422Z"
tags:
  - frontend
  - sessions
  - realtime
  - tests
depends_on:
  - "2acdq"
parent: "6s8j7"
---

## Summary
Cover the durability and escape-hatch sentences of the "Sessions" and "Agent profiles" paragraphs: a session keeps running when nobody watches and anyone opening it later sees the full history (reload, second tab, second user), the WebSocket resumes from the last `seq` without duplicates after a forced disconnect, the terminal opens into the running container as `agent` in `/session/work`, `Copy link` on the session header, the task side panel, and an ephemeral profile run once from the project page ending in `done` with no composer. Session lifecycle assertions stay in the lifecycle spec; this file is about views and streams.

## Documents
- `SPEC.md` "User-facing features", "Sessions" (sessions run when nobody is watching; anyone opening later sees everything; terminal into a running container), "Agent profiles" (ephemeral profile runs one prompt, shows transcript, result and cost, has no composer).
- `SPEC.md` "WebSocket: session stream" (`?after=<seq>` replay, dedupe on `seq`, `terminal_open` requires `running`, terminal is `/bin/bash -l` as `agent`, binary frames, `terminal_closed` with `exit_code`; pings every 30 s), "Sessions" table (`GET /sessions/{id}/events?before=&limit=` → `{events, has_more}`; `POST /projects/{pid}/sessions` 400 for an ephemeral profile with neither `task_id` nor `message`; `/input` 409 for ephemeral).
- `SPEC.md` "Frontend": "Session state" (store folds events, `lastSeq`, reconnect with fresh token, older history through REST on scroll-up), "Copy links" (`/sessions/{id}`, `Link copied`), "Composer" (absent for ephemeral, disabled when `done`/`failed`).
- `SPEC.md` "Authentication" (streams accept `?token=`; on close the client refreshes and reopens with the last cursor).
- `ARCHITECTURE.md` "Session container specification" (working directory `/session/work`, user `1000:1000`, `HOME=/session/home`, environment `MARS_SESSION_ID`, `MARS_PROJECT_ID`).

## Acceptance criteria
- [ ] `frontend/tests/session-view.spec.ts`; `test.setTimeout(180_000)`; sessions ended in `afterEach`.
- [ ] `full history after reload and in a second tab`: launch with a message, send `next` (two turns rendered); `page.reload()` → the same messages in the same order, no duplicates (count assistant text rows and tool cards); open the session in a second context as another user → identical transcript; send `more` from context A → context B shows the new turn live.
- [ ] `reconnect resumes without duplicates`: with a transcript of two turns, force the socket closed with `page.context().setOffline(true)` for 3 s then `setOffline(false)`; the header shows `reconnecting` then `live`; send a message; the reply renders once and no earlier row is duplicated.
- [ ] `older history loads on scroll-up` (only if the frontend paginates below a threshold reachable with the fixture; otherwise `test.skip` with the threshold noted): send enough messages after fixture exhaustion (`Stub reply to:` lines) to exceed the initial window, reload, scroll to top, older rows appear once.
- [ ] `terminal into a running container`: open the terminal tab on a `running` session; type `pwd; id -u; echo $MARS_SESSION_ID\n`; the xterm output contains `/session/work`, `1000` and the session id; type `exit\n` → `terminal_closed` renders with exit code 0; on a `parked` session the terminal control is disabled or shows the "session must be running" message.
- [ ] `copy link`: `Copy link` on the session header writes `http://localhost:5173/sessions/<id>` and shows `Link copied`; opening that URL unauthenticated goes to `/login` and, after login, back to the session.
- [ ] `ephemeral run once from the project page`: create profile `oneshot` (kind `ephemeral`, partial messages default off) in the profiles tab; on the sessions tab choose it and enter message `summarise`; the session view shows the transcript, the `result` row and cost, no composer at all; the session ends in `done` by itself once the stub exits after replaying every turn (`-p` mode); `POST /sessions/{id}/input` through the helper API returns 409. Launching `oneshot` with neither task nor message shows the 400 error.
- [ ] `metadata header`: state badge, branch `session/<id>`, container id (short), `cli_session_id`, `base_ref` `main`, and the launching user are visible on a running session.

## Implementation notes
- Files: `frontend/tests/session-view.spec.ts`.
- Terminal typing: focus the xterm textarea (`.xterm-helper-textarea`) and use `page.keyboard.type`; read output from the xterm rows (`.xterm-rows`) or the accessibility tree; allow 5 s for the exec to attach.
- The `setOffline` route only affects the page's network, not the orchestrator, so the session keeps running; that is the point of the test.
- Duplicate detection: count rows by a stable test id (`data-testid="message-<id>"` if the session epic exposes one, else `getByRole("article")`) before and after.
- Cross-user access is expected (no per-project authorisation in v1, `SPEC.md` "Non-goals").

## Edge cases
- `terminal_open` on a `parked` session is refused by the server; the UI should not offer it; assert the disabled control rather than the socket error.
- Scroll-up pagination depends on the frontend's `limit` (`n ≤ 500`); check `useSessionSocket` for the initial `after`/history strategy before writing the test.
- Chromium's clipboard needs `grantPermissions(["clipboard-read","clipboard-write"])`.

## Testing
- The spec file; run twice against one stack.
- Command: `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:e2e`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Frontend project and session views": `TerminalView` over xterm.js, `useSessionSocket` reconnect, `Copy link`, ephemeral launch control, metadata header.
- "Real-time delivery": WebSocket replay with `after`, terminal exec multiplexing; "Session lifecycle": ephemeral `-p` run ending in `done`.