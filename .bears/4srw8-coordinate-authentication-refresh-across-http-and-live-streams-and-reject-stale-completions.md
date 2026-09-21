---
id: "4srw8"
title: Coordinate authentication refresh across HTTP and live streams and reject stale completions
status: done
priority: P1
created: "2026-09-21T10:44:27.194573479Z"
updated: "2026-09-21T17:00:58.726131965Z"
tags:
  - frontend
  - technical-review
  - bug
  - auth
parent: "579dz"
attempts: 1
---

Problem: apiClient.ts has a private shared refresh promise, but SessionSocket and TaskStream call refreshAccessToken directly. Concurrent refreshes can submit the same rotating cookie; one succeeds and another receives 401 and signs the user out. A refresh completion can also install authentication after logout or a newer login.

Acceptance: own the single in-flight refresh operation in services/auth.ts so HTTP, WebSocket and SSE callers all share it. Use authentication generation/ownership checks so obsolete successes cannot reinstall a session and obsolete failures cannot sign out a newer session. Preserve transient-failure retry behavior and credential replacement notifications without duplicate reconnects. Add controlled-promise tests for simultaneous HTTP/stream refresh, refresh completion after logout, and completion after a newer login.

References: frontend/src/services/apiClient.ts:112; frontend/src/services/auth.ts:207; frontend/src/session/useSessionSocket.ts:268; frontend/src/tasks/useTaskStream.ts:191; orchestrator/src/auth/credentials.rs refresh rotation. Contract: SPEC.md, "Authentication" and "Frontend", Rules.

Merged from the second review (2026-09-21), same finding, additional detail:
- Natural trigger: a laptop wakes after the 15-minute access token expired; the dead stream's close handler and the window-focus refetch's 401 both refresh in the same tick. The losing 401 also carries the clearing Set-Cookie, which can land after the winner's new cookie and delete it (orchestrator/src/routes/auth.rs:128-147, repositories/refresh_tokens.rs: no reuse grace).
- apiClient.ts:182-184 refreshes even when the token the failed request carried is no longer the current one (another request already refreshed). Compare `token !== getAccessToken()` and just retry; that removes one rotation per burst.
- Moving the single-flight into auth.ts removes most of the reason for the documented apiClient <-> auth import cycle (apiClient.ts:16, auth.ts:17). It is safe today only because it crosses hoisted functions; consider injecting `{ getToken, refresh }` into apiClient to cut it.
- Two follow-ups are separate tasks of this epic and depend on this one: streams torn down on every ordinary refresh, and cross-tab sign-out/token propagation.