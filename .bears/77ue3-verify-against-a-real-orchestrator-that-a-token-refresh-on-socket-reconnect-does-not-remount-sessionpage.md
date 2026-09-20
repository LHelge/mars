---
id: "77ue3"
title: Verify against a real orchestrator that a token refresh on socket reconnect does not remount SessionPage
status: open
priority: P2
created: "2026-09-20T08:49:57.461269981Z"
updated: "2026-09-20T18:50:44.361190492Z"
tags:
  - frontend
  - sessions
  - auth
  - e2e
depends_on:
  - "2acdq"
  - hez8r
parent: "6s8j7"
---

## Summary
Reported by the dyr6h (TerminalView) implementer from a Playwright run against a **mocked** API and WebSocket: when the session socket dropped, `SessionSocket.refreshThenReconnect()` rotated the token and the app appeared to re-bootstrap (`GET /users/me` and the session query ran again) with `SessionPage` unmounting and remounting. If real, every reconnect would discard the composer's typed text, reopen the side panel's state and hand the operator a fresh terminal instead of the documented `Terminal disconnected — Reconnect` state.

The coordinator could not find a code path for it: `services/auth.ts#installSession` sets a non-null `user` from the refresh response, so `ProtectedRoute` should not fall back to `LoadingState`, and `AuthBootstrap` only reloads the user when it is null. The observation may be an artefact of the mock's refresh response (for example one without a `user`). This task settles it against the real thing.

## Documents
- `SPEC.md` "Authentication" (stream rules: refresh, then reopen with a fresh token and the last cursor; `AuthResponse` shape of `POST /auth/refresh`), "Frontend" rules (`ProtectedRoute`, authenticated startup loads `GET /users/me`).

## Acceptance criteria
- [ ] A Playwright scenario against the real orchestrator (`--features integration-tests`, stub session image) opens a session, types text into the composer without sending, forces the socket closed (or lets the access token expire), and asserts after the reconnect: the composer still holds the text, no second `GET /api/sessions/{id}` bootstrap navigation occurred, and an open Terminal panel shows `Terminal disconnected` with `Reconnect`.
- [ ] If the remount is real: fix its cause (most likely auth state briefly publishing `user: null`, or a `key`/route element changing identity on token change) with a unit test beside the fix, and say in the commit which it was. If it is not real: the scenario stays as the regression test and the task closes with a note.
- [ ] The frontend quality chain passes.

## Out of scope
Changing the refresh contract.
