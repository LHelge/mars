---
id: ntepg
title: Resolve repeated socket authentication rejection into an actionable state
status: open
priority: P2
created: "2026-09-21T10:44:36.634530929Z"
updated: "2026-09-21T10:50:13.545375192Z"
tags:
  - frontend
  - technical-review
  - bug
  - session
  - auth
depends_on:
  - "4srw8"
parent: "579dz"
---

Problem: SessionSocket.handleClose returns after a second authentication close inside AUTH_RETRY_WINDOW_MS. It leaves status 'reconnecting' but schedules no retry, triggers no sign-out and surfaces no error; the claimed foundation redirect in the comment never happens.

Acceptance: explicitly resolve this branch into the appropriate authentication recovery or a visible terminal connection error with a recovery action. Avoid endless refresh loops and avoid claiming a reconnect is underway when no attempt is pending. Account for valid authentication with a resource-specific refusal when choosing behavior. Update the existing test that merely asserts giving up so it asserts the user-visible outcome, stream cleanup and absence of retry storms.

References: frontend/src/session/useSessionSocket.ts:254 and frontend/src/session/useSessionSocket.test.ts repeated auth rejection test. Contract: SPEC.md, "Authentication", "WebSocket: session stream", and "Frontend", session state.

Merged from the second review (2026-09-21), the opposite failure on the same close path:
- For every non-auth close, handleClose -> refreshThenReconnect -> scheduleRetry -> connect loops with no bound (useSessionSocket.ts:244-298). If the session was deleted elsewhere the upgrade is refused and never opens, so the loop runs at the 30 s backoff cap indefinitely, rotating the refresh token on every cycle and never telling the user. Acceptance addition: after N consecutive attempts that never reached `open`, read the session once over REST and stop with a visible state on 404/403; do not refresh the token on closes that are not authentication closes (this also shrinks the race in 4srw8).
- A message sent over a socket that dies before acknowledgement is tracked in the transcript/composer task of this epic, not here.